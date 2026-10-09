use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};
use transmog_capture::{CaptureLimits, CaptureRecordKind, read_indexed_frame};
use transmog_saz::{ArchiveBody, SazArchive};

use crate::body_store::SavedBodySource;

/// Clones independent cursors over one pinned, read-only source file. The lock
/// covers only seek/read pairs; separate body streams never share positions.
#[derive(Clone, Debug)]
pub(crate) struct SourceReader {
    file: Arc<Mutex<File>>,
    position: u64,
    length: u64,
}
impl SourceReader {
    pub(crate) fn new(file: File) -> io::Result<Self> {
        let length = file.metadata()?.len();
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            position: 0,
            length,
        })
    }
}
impl Read for SourceReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("Trace source reader failed"))?;
        file.seek(SeekFrom::Start(self.position))?;
        let remaining =
            usize::try_from(self.length.saturating_sub(self.position)).unwrap_or(usize::MAX);
        let length = remaining.min(buffer.len());
        let count = file.read(&mut buffer[..length])?;
        self.position = self.position.saturating_add(count as u64);
        Ok(count)
    }
}
impl Seek for SourceReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(offset) => self.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.position)
    }
}

#[derive(Debug)]
pub(crate) struct SazBodySource {
    archive: SazArchive<SourceReader>,
    body: ArchiveBody,
    length: Arc<AtomicU64>,
}
impl SazBodySource {
    pub(crate) fn new(
        archive: SazArchive<SourceReader>,
        body: ArchiveBody,
        measured_length: Option<u64>,
    ) -> Self {
        let length = Arc::new(AtomicU64::new(measured_length.unwrap_or(if body.chunked {
            u64::MAX
        } else {
            body.wire_bytes
        })));
        Self {
            archive,
            body,
            length,
        }
    }
}
impl SavedBodySource for SazBodySource {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut archive = self.archive.clone();
        let body = self.body.clone();
        let length = self.length.clone();
        std::thread::Builder::new()
            .name("transmog-saz-body".into())
            .spawn(move || {
                let mut writer = ChunkWriter {
                    sender: sender.clone(),
                };
                let outcome = archive
                    .copy_body(&body, &mut writer, || false)
                    .map_err(|_| io::Error::other("Saved body is incomplete, changed or corrupt"));
                if let Ok(bytes) = outcome {
                    length.store(bytes, Ordering::Release);
                }
                let _ = sender.send(Chunk::End(outcome.map(|_| ())));
            })?;
        Ok(Box::new(ChunkReader {
            receiver,
            current: io::Cursor::new(Vec::new()),
            finished: false,
        }))
    }
    fn length(&self) -> Option<u64> {
        let length = self.length.load(Ordering::Acquire);
        (length != u64::MAX).then_some(length)
    }
}

enum Chunk {
    Bytes(Vec<u8>),
    End(io::Result<()>),
}
struct ChunkWriter {
    sender: mpsc::SyncSender<Chunk>,
}
impl Write for ChunkWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        for bytes in buffer.chunks(16 * 1024) {
            self.sender
                .send(Chunk::Bytes(bytes.to_vec()))
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        }
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct ChunkReader {
    receiver: mpsc::Receiver<Chunk>,
    current: io::Cursor<Vec<u8>>,
    finished: bool,
}
impl Read for ChunkReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let read = self.current.read(buffer)?;
            if read > 0 || self.finished {
                return Ok(read);
            }
            match self
                .receiver
                .recv()
                .map_err(|_| io::Error::other("Saved body reader stopped"))?
            {
                Chunk::Bytes(bytes) => self.current = io::Cursor::new(bytes),
                Chunk::End(result) => {
                    self.finished = true;
                    result?;
                    return Ok(0);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NativeBodyPiece {
    pub offset: u64,
    pub frame_bytes: u64,
    pub digest: [u8; 32],
}
#[derive(Debug)]
pub(crate) struct NativeBodySource {
    pub reader: SourceReader,
    pub pieces: Arc<[NativeBodyPiece]>,
    pub bytes: u64,
}
impl SavedBodySource for NativeBodySource {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        Ok(Box::new(NativeBodyReader {
            reader: self.reader.clone(),
            pieces: self.pieces.clone(),
            next: 0,
            current: io::Cursor::new(Vec::new()),
        }))
    }
    fn length(&self) -> Option<u64> {
        Some(self.bytes)
    }
}
struct NativeBodyReader {
    reader: SourceReader,
    pieces: Arc<[NativeBodyPiece]>,
    next: usize,
    current: io::Cursor<Vec<u8>>,
}
impl Read for NativeBodyReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(buffer)?;
            if count > 0 || self.next == self.pieces.len() {
                return Ok(count);
            }
            let piece = &self.pieces[self.next];
            self.reader.seek(SeekFrom::Start(piece.offset))?;
            let limits = CaptureLimits {
                max_file_bytes: 4 * 1024 * 1024 * 1024,
                max_record_bytes: 8 * 1024 * 1024,
                max_records: 1,
            };
            let record = read_indexed_frame(&mut self.reader, piece.frame_bytes, limits)
                .map_err(|_| io::Error::other("Saved native frame changed or is corrupt"))?;
            let CaptureRecordKind::BodySegment {
                bytes: Some(bytes), ..
            } = record.kind
            else {
                return Err(io::Error::other("Saved body frame is unavailable"));
            };
            if <[u8; 32]>::from(Sha256::digest(&bytes)) != piece.digest {
                return Err(io::Error::other("Saved body bytes changed since import"));
            }
            self.current = io::Cursor::new(bytes);
            self.next += 1;
        }
    }
}

impl fmt::Debug for ChunkReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SavedBodyReader")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_cursors_keep_the_indexed_file_boundary_when_a_source_grows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        std::fs::write(&path, b"abcdef").unwrap();
        let mut first = SourceReader::new(File::open(&path).unwrap()).unwrap();
        let mut second = first.clone();
        second.seek(SeekFrom::Start(3)).unwrap();
        let mut bytes = [0; 2];
        first.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ab");
        second.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"de");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"new")
            .unwrap();
        let mut tail = Vec::new();
        first.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"cdef");
        assert!(second.seek(SeekFrom::Current(-100)).is_err());
    }
}
