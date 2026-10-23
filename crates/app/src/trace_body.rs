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
use transmog_capture::{CaptureLimits, CaptureRecordKind, read_indexed_frame_with_codec};
use transmog_saz::{ArchiveBody, SazArchive};

use crate::body_store::SavedBodySource;

/// Clones independent cursors over one pinned, read-only source file. The lock
/// covers only seek/read pairs; separate body streams never share positions.
#[derive(Clone, Debug)]
pub(crate) struct SourceReader {
    file: Arc<Mutex<SourceFile>>,
    position: u64,
    length: u64,
    modified: Option<std::time::SystemTime>,
    digest: Arc<Mutex<Option<String>>>,
}
#[derive(Debug)]
enum SourceFile {
    File(File),
    Package(transmog_portable::PackageMember),
}
impl Read for SourceFile {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(f) => f.read(bytes),
            Self::Package(p) => p.read(bytes),
        }
    }
}
impl Seek for SourceFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        match self {
            Self::File(f) => f.seek(from),
            Self::Package(p) => p.seek(from),
        }
    }
}
impl SourceFile {
    fn metadata(&self) -> io::Result<std::fs::Metadata> {
        match self {
            Self::File(f) => f.metadata(),
            Self::Package(_) => Err(io::Error::other(
                "Package object has no standalone metadata",
            )),
        }
    }
}
impl SourceReader {
    pub(crate) const fn length(&self) -> u64 {
        self.length
    }

    pub(crate) fn new(file: File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        let length = metadata.len();
        let modified = metadata.modified().ok();
        Ok(Self {
            file: Arc::new(Mutex::new(SourceFile::File(file))),
            position: 0,
            length,
            modified,
            digest: Arc::default(),
        })
    }
    pub(crate) fn package(
        member: transmog_portable::PackageMember,
        length: u64,
        sha: String,
    ) -> Self {
        Self {
            file: Arc::new(Mutex::new(SourceFile::Package(member))),
            position: 0,
            length,
            modified: None,
            digest: Arc::new(Mutex::new(Some(sha))),
        }
    }
    pub(crate) fn artifact_digest(
        &self,
        canceled: &std::sync::atomic::AtomicBool,
    ) -> io::Result<String> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("Trace source reader failed"))?;
        if matches!(&*file, SourceFile::Package(_)) {
            if canceled.load(Ordering::Acquire) {
                return Err(io::Error::other("Case export canceled"));
            }
            return self
                .digest
                .lock()
                .map_err(|_| io::Error::other("Source identity cache failed"))?
                .clone()
                .ok_or_else(|| io::Error::other("Package identity is unavailable"));
        }
        let before = file.metadata()?;
        if self.modified.is_none()
            || before.len() != self.length
            || before.modified().ok() != self.modified
        {
            return Err(io::Error::other("Original trace changed since indexing"));
        }
        if let Some(digest) = self
            .digest
            .lock()
            .map_err(|_| io::Error::other("Source identity cache failed"))?
            .clone()
        {
            return Ok(digest);
        }
        file.rewind()?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 128 * 1024];
        let mut total = 0_u64;
        loop {
            if canceled.load(Ordering::Acquire) {
                return Err(io::Error::other("Case export canceled"));
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("Trace length overflow"))?;
            if total > self.length {
                return Err(io::Error::other("Original trace grew while hashing"));
            }
            hash.update(&buffer[..count]);
        }
        let after = file.metadata()?;
        if total != self.length
            || after.len() != self.length
            || after.modified().ok() != self.modified
        {
            return Err(io::Error::other("Original trace changed while hashing"));
        }
        let digest = hex::encode(hash.finalize());
        *self
            .digest
            .lock()
            .map_err(|_| io::Error::other("Source identity cache failed"))? = Some(digest.clone());
        Ok(digest)
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
    range_cache: Mutex<Option<Arc<tempfile::NamedTempFile>>>,
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
            range_cache: Mutex::new(None),
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
    fn prepare_ranges(&self) -> io::Result<()> {
        let mut cache = self
            .range_cache
            .lock()
            .map_err(|_| io::Error::other("Saved archive byte cache failed"))?;
        if cache.is_some() {
            return Ok(());
        }
        let mut file = tempfile::NamedTempFile::new()?;
        let bytes = self
            .archive
            .clone()
            .copy_body(&self.body, &mut file, || false)
            .map_err(|_| io::Error::other("Saved body is incomplete, changed or corrupt"))?;
        file.flush()?;
        self.length.store(bytes, Ordering::Release);
        *cache = Some(Arc::new(file));
        Ok(())
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        if offset > 0 {
            self.prepare_ranges()?;
        }
        let cache = self
            .range_cache
            .lock()
            .map_err(|_| io::Error::other("Saved archive byte cache failed"))?
            .clone();
        if let Some(cache) = cache {
            let mut source = SourceReader::new(cache.reopen()?)?;
            let length = self.length().ok_or(io::ErrorKind::UnexpectedEof)?;
            if offset > length {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            source.seek(SeekFrom::Start(offset))?;
            // Retain ownership of the temporary cache while this cursor is open.
            return Ok(Box::new(CachedArchiveReader {
                reader: source.take(length - offset),
                _cache: cache,
            }));
        }
        self.open()
    }
}

struct CachedArchiveReader {
    reader: io::Take<SourceReader>,
    _cache: Arc<tempfile::NamedTempFile>,
}
impl Read for CachedArchiveReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
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
    pub body_offset: u64,
    pub bytes: u64,
    pub offset: u64,
    pub index: u64,
    pub codec: transmog_capture::FrameCodec,
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
            skip: 0,
            current: io::Cursor::new(Vec::new()),
        }))
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        if offset > self.bytes {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let next = self
            .pieces
            .partition_point(|piece| piece.body_offset + piece.bytes <= offset);
        let skip = self
            .pieces
            .get(next)
            .map_or(0, |piece| offset - piece.body_offset);
        Ok(Box::new(NativeBodyReader {
            reader: self.reader.clone(),
            pieces: self.pieces.clone(),
            next,
            skip,
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
    skip: u64,
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
                max_file_bytes: u64::MAX,
                max_record_bytes: 8 * 1024 * 1024,
                max_records: 1,
            };
            let record = read_indexed_frame_with_codec(
                &mut self.reader,
                piece.frame_bytes,
                limits,
                &piece.codec,
                piece.index,
            )
            .map_err(|_| io::Error::other("Saved native frame changed or is corrupt"))?;
            let CaptureRecordKind::BodySegment {
                bytes: Some(bytes), ..
            } = record.kind
            else {
                return Err(io::Error::other("Saved body frame is unavailable"));
            };
            if bytes.len() as u64 != piece.bytes
                || <[u8; 32]>::from(Sha256::digest(&bytes)) != piece.digest
            {
                return Err(io::Error::other("Saved body bytes changed since import"));
            }
            self.current = io::Cursor::new(bytes);
            self.current.set_position(self.skip);
            self.skip = 0;
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

    fn chunked_saz(path: &std::path::Path, compression: zip::CompressionMethod) -> SazBodySource {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, bytes) in [
            (
                "raw/1_c.txt",
                "GET https://example.invalid/ HTTP/1.1\r\n\r\n",
            ),
            (
                "raw/1_s.txt",
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            ),
        ] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(compression),
            )
            .unwrap();
            zip.write_all(bytes.as_bytes()).unwrap();
        }
        std::fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
        let mut archive = SazArchive::open(
            SourceReader::new(File::open(path).unwrap()).unwrap(),
            transmog_saz::SazImportLimits::default(),
        )
        .unwrap();
        let mut index = archive.index(|_, _| {}, || false).unwrap();
        let body = index.sessions.remove(0).response.unwrap().body;
        SazBodySource::new(archive, body, None)
    }

    #[test]
    fn compressed_saz_ranges_measure_once_and_release_the_cache_after_the_last_cursor() {
        let root = tempfile::tempdir().unwrap();
        let source = chunked_saz(
            &root.path().join("trace.saz"),
            zip::CompressionMethod::Deflated,
        );
        assert_eq!(source.length(), None);
        source.prepare_ranges().unwrap();
        assert_eq!(source.length(), Some(5));
        let cache = source.range_cache.lock().unwrap().as_ref().unwrap().clone();
        let cache_path = cache.path().to_owned();
        source.prepare_ranges().unwrap();
        assert!(Arc::ptr_eq(
            &cache,
            source.range_cache.lock().unwrap().as_ref().unwrap()
        ));
        let mut cursor = source.open_at(1).unwrap();
        let mut bytes = Vec::new();
        cursor.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"ello");
        drop(cache);
        drop(source);
        assert!(cache_path.exists());
        drop(cursor);
        assert!(!cache_path.exists());
    }

    #[test]
    fn corrupt_saz_cannot_publish_a_partial_range_cache() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("corrupt.saz");
        let source = chunked_saz(&path, zip::CompressionMethod::Stored);
        let mut bytes = std::fs::read(&path).unwrap();
        let offset = bytes
            .windows(5)
            .position(|bytes| bytes == b"hello")
            .unwrap();
        bytes[offset] ^= 1;
        std::fs::write(path, bytes).unwrap();
        assert!(source.prepare_ranges().is_err());
        assert!(source.range_cache.lock().unwrap().is_none());
        assert_eq!(source.length(), None);
    }

    #[test]
    fn native_tail_ranges_skip_earlier_frames_and_validate_only_the_loaded_payload() {
        use transmog_capture::{CaptureReader, CaptureRecord, CaptureWriter};
        use transmog_core::observe::ExchangeBoundary;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("indexed.tmcap");
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        // Corruption is detected per encrypted storage block. Use an earlier
        // body spanning distinct blocks so the selected tail is independent.
        let mut seed = 123_456_789_u32;
        let earlier: Vec<u8> = (0..32768)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed.to_le_bytes()[0]
            })
            .collect();
        for (index, bytes) in [earlier.clone(), vec![9; 57]].into_iter().enumerate() {
            writer
                .append(&CaptureRecord {
                    sequence: index as u64 + 1,
                    exchange_id: 1,
                    kind: CaptureRecordKind::BodySegment {
                        boundary: ExchangeBoundary::ClientResponse,
                        byte_count: bytes.len(),
                        bytes: Some(bytes),
                        truncated: false,
                    },
                })
                .unwrap();
        }
        writer.seal().unwrap();
        let bytes = writer.into_inner().unwrap();
        let mut reader =
            CaptureReader::new(io::Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        let first = reader.read_next().unwrap().unwrap();
        let second = reader.read_next().unwrap().unwrap();
        let pieces = [(&first, 0, earlier), (&second, 32768, vec![9; 57])]
            .into_iter()
            .map(|(frame, body_offset, body)| NativeBodyPiece {
                body_offset,
                bytes: body.len() as u64,
                offset: frame.offset,
                index: frame.index,
                codec: frame.codec.clone(),
                frame_bytes: frame.frame_bytes,
                digest: Sha256::digest(&body).into(),
            })
            .collect::<Vec<_>>();
        let mut corrupt = bytes;
        corrupt[usize::try_from(first.offset + 64).unwrap()] ^= 255;
        std::fs::write(&path, corrupt).unwrap();
        let source = NativeBodySource {
            reader: SourceReader::new(File::open(path).unwrap()).unwrap(),
            pieces: Arc::from(pieces),
            bytes: 32825,
        };
        let mut tail = Vec::new();
        source
            .open_at(32769)
            .unwrap()
            .read_to_end(&mut tail)
            .unwrap();
        assert_eq!(tail, vec![9; 56]);
        assert!(source.open().unwrap().read_to_end(&mut Vec::new()).is_err());
        assert!(source.open_at(32826).is_err());
    }

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

#[derive(Debug)]
pub(crate) struct NativeStreamSource {
    pub reader: SourceReader,
    pub offset: u64,
    pub frame_bytes: u64,
    pub codec: transmog_capture::FrameCodec,
    pub index: u64,
    pub exchange_id: u128,
    pub sequence: u64,
}
impl transmog_core::stream::StreamContentSource for NativeStreamSource {
    fn read(&self) -> Result<transmog_core::stream::StreamMessage, String> {
        let mut reader = self.reader.clone();
        reader
            .seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        let record = read_indexed_frame_with_codec(
            &mut reader,
            self.frame_bytes,
            CaptureLimits {
                max_file_bytes: u64::MAX,
                max_record_bytes: 8 * 1024 * 1024,
                max_records: usize::MAX,
            },
            &self.codec,
            self.index,
        )
        .map_err(|e| e.to_string())?;
        if record.exchange_id != self.exchange_id || record.sequence != self.sequence {
            return Err("Saved stream message identity changed".into());
        }
        match record.kind {
            CaptureRecordKind::StreamMessage(message) => Ok(message),
            _ => Err("Saved stream message is unavailable".into()),
        }
    }
}
