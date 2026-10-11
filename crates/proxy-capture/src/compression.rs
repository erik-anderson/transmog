//! Independent compression jobs with bounded, per-body adaptive sampling.
use std::io;

/// Maximum independently decompressible body chunk.
pub const BODY_CHUNK_BYTES: usize = 128 * 1024;
const PROBE_BYTES: u64 = 64 * 1024;
const REPROBE_BYTES: u64 = 8 * 1024 * 1024;

/// Compression decisions for one body boundary, reset for each new message.
#[derive(Default, Debug)]
pub struct BodyCompression {
    sampled: u64,
    saved: u64,
    bypass: bool,
    skipped: u64,
    misses: u8,
}

/// An independently encoded chunk. `codec == 0` means raw; `1` means zstd.
#[derive(Debug)]
pub struct CompressedChunk {
    /// Encoding discriminator; encrypted along with the chunk descriptor.
    pub codec: u8,
    /// Exact expanded length.
    pub expanded: usize,
    /// Raw bytes or a complete independent zstd frame.
    pub bytes: Vec<u8>,
}

/// Reusable compressor owned by one capture worker.
pub struct ChunkCompressor {
    context: zstd::bulk::Compressor<'static>,
    scratch: Vec<u8>,
    attempted: u64,
}

impl ChunkCompressor {
    /// Creates a compressor with a reusable native context.
    /// # Errors
    /// Returns a zstd initialization failure.
    pub fn new(level: i32) -> io::Result<Self> {
        Ok(Self {
            context: zstd::bulk::Compressor::new(level)?,
            scratch: Vec::new(),
            attempted: 0,
        })
    }

    /// Number of source bytes passed to zstd, including unsuccessful probes.
    pub fn attempted_bytes(&self) -> u64 {
        self.attempted
    }

    fn compress(&mut self, input: &[u8]) -> io::Result<usize> {
        self.scratch
            .resize(zstd::zstd_safe::compress_bound(input.len()), 0);
        self.attempted = self.attempted.saturating_add(input.len() as u64);
        self.context
            .compress_to_buffer(input, self.scratch.as_mut_slice())
    }

    /// Compresses metadata independently; tiny or ineffective results remain raw.
    /// # Errors
    /// Returns a native compression failure.
    pub fn metadata(&mut self, input: &[u8]) -> io::Result<CompressedChunk> {
        let size = if input.len() >= 256 {
            self.compress(input)?
        } else {
            input.len()
        };
        Ok(self.result(input, size))
    }

    fn result(&self, input: &[u8], size: usize) -> CompressedChunk {
        let saved = input.len().saturating_sub(size);
        let useful = saved >= 64 && saved >= input.len().div_ceil(50);
        CompressedChunk {
            codec: u8::from(useful),
            expanded: input.len(),
            bytes: if useful {
                self.scratch[..size].to_vec()
            } else {
                input.to_vec()
            },
        }
    }

    /// Compresses a body chunk or bypasses zstd after ineffective sampling.
    /// The caller bounds chunks and keeps one state per body representation.
    /// # Errors
    /// Returns a native compression failure or an oversized chunk.
    pub fn body(
        &mut self,
        input: &[u8],
        state: &mut BodyCompression,
    ) -> io::Result<CompressedChunk> {
        if input.len() > BODY_CHUNK_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "body compression chunk exceeds 128 KiB",
            ));
        }
        if state.bypass {
            state.skipped = state.skipped.saturating_add(input.len() as u64);
            if state.skipped < REPROBE_BYTES || input.len() < 1024 {
                return Ok(CompressedChunk {
                    codec: 0,
                    expanded: input.len(),
                    bytes: input.to_vec(),
                });
            }
            state.skipped = 0;
            let probe = &input[..input.len().min(64 * 1024)];
            let size = self.compress(probe)?;
            if probe.len().saturating_sub(size) < probe.len().div_ceil(25) {
                return Ok(CompressedChunk {
                    codec: 0,
                    expanded: input.len(),
                    bytes: input.to_vec(),
                });
            }
            state.bypass = false;
            state.misses = 0;
        }
        let size = if input.len() >= 256 {
            self.compress(input)?
        } else {
            input.len()
        };
        let result = self.result(input, size);
        state.observe(input.len(), &result);
        Ok(result)
    }
}

impl BodyCompression {
    pub(crate) fn parallel_worthwhile(&self) -> bool {
        self.sampled >= PROBE_BYTES && !self.bypass
    }

    pub(crate) fn observe(&mut self, length: usize, result: &CompressedChunk) {
        let state = self;
        state.sampled = state.sampled.saturating_add(length as u64);
        state.saved = state
            .saved
            .saturating_add(length.saturating_sub(result.bytes.len()) as u64);
        state.misses = if result.codec == 0 {
            state.misses.saturating_add(1)
        } else {
            0
        };
        if state.sampled >= PROBE_BYTES
            && (state.saved < state.sampled.div_ceil(50) || state.misses >= 2)
        {
            state.bypass = true;
            state.skipped = 0;
        }
    }
}

/// A small bounded pool. Adaptive bypass decisions stay on the ordering thread.
pub(crate) struct CompressionPool {
    sender: Option<std::sync::mpsc::SyncSender<Job>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}
type Job = (
    Vec<u8>,
    std::sync::mpsc::SyncSender<io::Result<CompressedChunk>>,
);
impl CompressionPool {
    pub(crate) fn new(count: usize) -> io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Job>(8);
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        let mut pool = Self {
            sender: Some(sender),
            workers: Vec::new(),
        };
        for index in 0..count {
            let receiver = receiver.clone();
            let mut compressor = ChunkCompressor::new(1)?;
            pool.workers.push(
                std::thread::Builder::new()
                    .name(format!("tmcap-zstd-{index}"))
                    .spawn(move || {
                        loop {
                            let job = receiver
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .recv();
                            let Ok((bytes, reply)) = job else {
                                break;
                            };
                            let _ = reply.send(compressor.metadata(&bytes));
                        }
                    })?,
            );
        }
        Ok(pool)
    }
    pub(crate) fn submit(
        &self,
        bytes: Vec<u8>,
    ) -> io::Result<std::sync::mpsc::Receiver<io::Result<CompressedChunk>>> {
        let (reply, result) = std::sync::mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .unwrap()
            .send((bytes, reply))
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(result)
    }
}
impl Drop for CompressionPool {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn random() -> Vec<u8> {
        let mut seed = 123_456_789_u64;
        (0..BODY_CHUNK_BYTES)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed.to_le_bytes()[0]
            })
            .collect()
    }
    #[test]
    fn incompressible_body_bypasses_work_and_recovers_when_content_changes() {
        let input = random();
        let mut state = BodyCompression::default();
        let mut codec = ChunkCompressor::new(1).unwrap();
        for _ in 0..128 {
            let output = codec.body(&input, &mut state).unwrap();
            assert_eq!(output.codec, 0);
            assert_eq!(output.bytes, input);
        }
        assert!(codec.attempted_bytes() < 512 * 1024);
        let input = vec![b'a'; BODY_CHUNK_BYTES];
        let mut resumed = false;
        for _ in 0..65 {
            resumed |= codec.body(&input, &mut state).unwrap().codec == 1;
        }
        assert!(resumed);
    }
    #[test]
    fn chunks_are_independent_and_body_states_are_isolated() {
        let mut codec = ChunkCompressor::new(1).unwrap();
        let mut binary = BodyCompression::default();
        codec.body(&random(), &mut binary).unwrap();
        let mut text = BodyCompression::default();
        let source = b"example text with repetitions\n".repeat(4096);
        let output = codec.body(&source, &mut text).unwrap();
        assert_eq!(output.codec, 1);
        assert_eq!(
            zstd::bulk::decompress(&output.bytes, output.expanded).unwrap(),
            source
        );
    }
}
