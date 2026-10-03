use std::{
    collections::VecDeque,
    io,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll},
};

use async_compression::tokio::{
    bufread::{
        BrotliDecoder as AsyncBrotliDecoder, DeflateDecoder as AsyncDeflateDecoder,
        GzipDecoder as AsyncGzipDecoder, ZlibDecoder as AsyncZlibDecoder,
        ZstdDecoder as AsyncZstdDecoder,
    },
    write::{
        BrotliEncoder as AsyncBrotliEncoder, GzipEncoder as AsyncGzipEncoder,
        ZlibEncoder as AsyncZlibEncoder, ZstdEncoder as AsyncZstdEncoder,
    },
};
use async_compression::zstd::DParameter;
use bytes::Bytes;
use rustymiddle_core::{BodyFrame, HeaderBlock};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::{ContentBudget, ContentCoding, ContentLimitError, ContentLimits};

const MINIMUM_ZSTD_WINDOW_BYTES: usize = 1 << 10;

/// Compatibility policy for the historically ambiguous HTTP `deflate` coding.
///
/// RFC 9110 defines `deflate` as a zlib-wrapped stream. Some legacy senders
/// incorrectly emit a raw DEFLATE stream instead.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DeflateCompatibility {
    /// Accept only the standards-conformant zlib wrapper.
    #[default]
    StrictZlib,
    /// Accept zlib, or select raw DEFLATE when the first two bytes cannot be a
    /// valid zlib header.
    AllowRaw,
}

/// Optional decoder interoperability policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContentDecoderOptions {
    deflate_compatibility: DeflateCompatibility,
}

impl ContentDecoderOptions {
    /// Creates strict decoder options.
    pub const fn new() -> Self {
        Self {
            deflate_compatibility: DeflateCompatibility::StrictZlib,
        }
    }

    /// Selects the HTTP `deflate` compatibility policy.
    #[must_use]
    pub const fn with_deflate_compatibility(mut self, compatibility: DeflateCompatibility) -> Self {
        self.deflate_compatibility = compatibility;
        self
    }

    /// Returns the configured HTTP `deflate` compatibility policy.
    pub const fn deflate_compatibility(self) -> DeflateCompatibility {
        self.deflate_compatibility
    }
}

/// A bounded incremental decoder for one HTTP content-coding layer.
///
/// Input and output use canonical [`BodyFrame`] values. Data frame boundaries
/// are not representation boundaries and may change during decoding. A
/// trailers frame is held until [`finish`](Self::finish) has validated the
/// compressed stream, then emitted after all decoded data.
#[derive(Debug)]
pub struct ContentDecoder {
    coding: ContentCoding,
    engine: DecoderEngine,
    budget: ContentBudget,
    pending: Vec<u8>,
    deflate_probe: Option<Vec<u8>>,
    stream_complete: bool,
    trailers: Option<HeaderBlock>,
    terminal: bool,
}

impl ContentDecoder {
    /// Creates a decoder for a supported coding.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the codec cannot honor the configured
    /// resource limits.
    pub fn new(coding: ContentCoding, limits: ContentLimits) -> Result<Self, ContentCodecError> {
        Self::with_options(coding, limits, ContentDecoderOptions::default())
    }

    /// Creates a decoder with explicit interoperability options.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the codec cannot honor the configured
    /// resource limits or interoperability policy.
    pub fn with_options(
        coding: ContentCoding,
        limits: ContentLimits,
        options: ContentDecoderOptions,
    ) -> Result<Self, ContentCodecError> {
        let engine = match coding {
            ContentCoding::Gzip => {
                let mut decoder = AsyncGzipDecoder::new(ChunkReader::new());
                decoder.multiple_members(true);
                DecoderEngine::Gzip(decoder)
            }
            ContentCoding::Brotli => {
                DecoderEngine::Brotli(AsyncBrotliDecoder::new(ChunkReader::new()))
            }
            ContentCoding::Deflate => {
                DecoderEngine::Zlib(AsyncZlibDecoder::new(ChunkReader::new()))
            }
            ContentCoding::Zstd => {
                let configured = limits.max_decoder_window_bytes().get();
                if configured < MINIMUM_ZSTD_WINDOW_BYTES {
                    return Err(ContentCodecError::DecoderWindowTooSmall {
                        coding,
                        minimum: MINIMUM_ZSTD_WINDOW_BYTES,
                        configured,
                    });
                }
                let mut decoder = AsyncZstdDecoder::with_params(
                    ChunkReader::new(),
                    &[DParameter::window_log_max(configured.ilog2())],
                );
                decoder.multiple_members(true);
                DecoderEngine::Zstd(decoder)
            }
        };
        let deflate_probe = (coding == ContentCoding::Deflate
            && options.deflate_compatibility() == DeflateCompatibility::AllowRaw)
            .then(Vec::new);
        Ok(Self {
            coding,
            engine,
            budget: ContentBudget::new(limits),
            pending: Vec::new(),
            deflate_probe,
            stream_complete: false,
            trailers: None,
            terminal: false,
        })
    }

    /// Accepts one encoded body frame and returns decoded data made available
    /// by that input.
    ///
    /// A trailers frame produces no immediate output. Call [`finish`](Self::finish)
    /// to validate the codec footer and release the terminal trailers.
    ///
    /// # Errors
    ///
    /// Fails closed on malformed input, a configured limit, invalid frame
    /// ordering, or use after a terminal success or failure.
    pub fn on_frame(&mut self, frame: BodyFrame) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.ensure_active()?;
        let result = match frame {
            BodyFrame::Data(bytes) => self.decode_data(&bytes),
            BodyFrame::Trailers(trailers) => {
                if self.trailers.replace(trailers).is_some() {
                    Err(ContentCodecError::DuplicateTrailers)
                } else {
                    Ok(Vec::new())
                }
            }
        };
        if result.is_err() {
            self.terminal = true;
        }
        result
    }

    /// Completes and validates the compressed stream.
    ///
    /// Any final decoded data precedes the optional terminal trailers frame.
    /// Completion is one-shot, including after an error.
    ///
    /// # Errors
    ///
    /// Returns a typed truncation, corruption, limit, allocation, sequencing,
    /// or terminal-state failure.
    pub fn finish(&mut self) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.ensure_active()?;
        self.terminal = true;
        if !self.stream_complete {
            if let Some(probe) = self.deflate_probe.take() {
                self.engine.push(Bytes::from(probe));
            }
            self.engine.finish_input();
            if !self.pump()? {
                return Err(ContentCodecError::Codec {
                    coding: self.coding,
                });
            }
            self.stream_complete = true;
        }
        if self.engine.has_buffered_input() {
            return Err(ContentCodecError::InvalidData {
                coding: self.coding,
            });
        }
        let mut output = self.drain_data();
        if let Some(trailers) = self.trailers.take() {
            output.push(BodyFrame::Trailers(trailers));
        }
        Ok(output)
    }

    fn decode_data(&mut self, bytes: &Bytes) -> Result<Vec<BodyFrame>, ContentCodecError> {
        if self.trailers.is_some() {
            return Err(ContentCodecError::DataAfterTrailers);
        }
        if self.stream_complete && !bytes.is_empty() {
            return Err(ContentCodecError::InvalidData {
                coding: self.coding,
            });
        }
        self.budget.record_encoded(bytes.len())?;
        if !self.queue_encoded(bytes) {
            return Ok(Vec::new());
        }
        if self.pump()? {
            self.stream_complete = true;
            if self.engine.has_buffered_input() {
                return Err(ContentCodecError::InvalidData {
                    coding: self.coding,
                });
            }
        }
        Ok(self.drain_data())
    }

    fn queue_encoded(&mut self, bytes: &Bytes) -> bool {
        let Some(mut probe) = self.deflate_probe.take() else {
            self.engine.push(bytes.clone());
            return true;
        };
        if probe.len().saturating_add(bytes.len()) < 2 {
            probe.extend_from_slice(bytes);
            self.deflate_probe = Some(probe);
            return false;
        }

        let first = probe.first().copied().unwrap_or(bytes[0]);
        let second = if probe.is_empty() { bytes[1] } else { bytes[0] };
        if !is_zlib_header(first, second) {
            self.engine.select_raw_deflate();
        }
        if !probe.is_empty() {
            self.engine.push(Bytes::from(probe));
        }
        self.engine.push(bytes.clone());
        true
    }

    fn pump(&mut self) -> Result<bool, ContentCodecError> {
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        loop {
            let mut bytes = [0_u8; 8 * 1024];
            match self.engine.poll_read(&mut context, &mut bytes) {
                Poll::Ready(Ok(0)) => return Ok(true),
                Poll::Ready(Ok(written)) => {
                    self.budget.record_decoded(written)?;
                    if self.pending.try_reserve(written).is_err() {
                        return Err(ContentCodecError::ResourceExhausted {
                            coding: self.coding,
                        });
                    }
                    self.pending.extend_from_slice(&bytes[..written]);
                }
                Poll::Ready(Err(error)) => {
                    return Err(map_decode_io_error(self.coding, &error));
                }
                Poll::Pending => return Ok(false),
            }
        }
    }

    fn drain_data(&mut self) -> Vec<BodyFrame> {
        if self.pending.is_empty() {
            Vec::new()
        } else {
            vec![BodyFrame::Data(Bytes::from(std::mem::take(
                &mut self.pending,
            )))]
        }
    }

    fn ensure_active(&self) -> Result<(), ContentCodecError> {
        if self.terminal {
            Err(ContentCodecError::AlreadyFinished {
                coding: self.coding,
            })
        } else {
            Ok(())
        }
    }
}

/// A bounded incremental encoder for one HTTP content-coding layer.
///
/// Decoded data is accepted as canonical [`BodyFrame`] values. Trailers are
/// held until [`finish`](Self::finish) writes the codec footer, so trailers can
/// never precede encoded bytes.
#[derive(Debug)]
pub struct ContentEncoder {
    coding: ContentCoding,
    engine: EncoderEngine,
    capture: SharedCapture,
    limits: ContentLimits,
    input_bytes: usize,
    trailers: Option<HeaderBlock>,
    terminal: bool,
}

impl ContentEncoder {
    /// Creates an encoder for a supported coding.
    ///
    /// # Errors
    ///
    /// Returns a typed error when codec initialization cannot honor the
    /// configured policy.
    pub fn new(coding: ContentCoding, limits: ContentLimits) -> Result<Self, ContentCodecError> {
        let (writer, capture) = CaptureWriter::new(limits);
        let engine = match coding {
            ContentCoding::Gzip => EncoderEngine::Gzip(AsyncGzipEncoder::new(writer)),
            ContentCoding::Brotli => {
                EncoderEngine::Brotli(Box::new(AsyncBrotliEncoder::new(writer)))
            }
            ContentCoding::Deflate => EncoderEngine::Zlib(AsyncZlibEncoder::new(writer)),
            ContentCoding::Zstd => EncoderEngine::Zstd(AsyncZstdEncoder::new(writer)),
        };
        Ok(Self {
            coding,
            engine,
            capture,
            limits,
            input_bytes: 0,
            trailers: None,
            terminal: false,
        })
    }

    /// Accepts one decoded body frame and returns encoded data made available
    /// by that input.
    ///
    /// A trailers frame produces no immediate output. Call [`finish`](Self::finish)
    /// to write the codec footer and release terminal trailers.
    ///
    /// # Errors
    ///
    /// Fails closed on a configured limit, invalid frame ordering, codec
    /// failure, or use after a terminal success or failure.
    pub async fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.ensure_active()?;
        let result = match frame {
            BodyFrame::Data(bytes) => self.encode_data(&bytes).await,
            BodyFrame::Trailers(trailers) => {
                if self.trailers.replace(trailers).is_some() {
                    Err(ContentCodecError::DuplicateTrailers)
                } else {
                    Ok(Vec::new())
                }
            }
        };
        if result.is_err() {
            self.terminal = true;
        }
        result
    }

    /// Completes the compressed stream and emits optional terminal trailers.
    ///
    /// Completion is one-shot, including after an error.
    ///
    /// # Errors
    ///
    /// Returns a typed limit, allocation, codec, sequencing, or terminal-state
    /// failure.
    pub async fn finish(&mut self) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.ensure_active()?;
        self.terminal = true;
        if self.engine.shutdown().await.is_err() {
            return Err(map_encode_io_error(self.coding, &self.capture));
        }
        let mut output = self.capture.drain_data(self.coding)?;
        if let Some(trailers) = self.trailers.take() {
            output.push(BodyFrame::Trailers(trailers));
        }
        Ok(output)
    }

    async fn encode_data(&mut self, bytes: &Bytes) -> Result<Vec<BodyFrame>, ContentCodecError> {
        if self.trailers.is_some() {
            return Err(ContentCodecError::DataAfterTrailers);
        }
        self.record_input(bytes.len())?;
        if self.engine.write_all(bytes).await.is_err() {
            return Err(map_encode_io_error(self.coding, &self.capture));
        }
        self.capture.drain_data(self.coding)
    }

    fn record_input(&mut self, bytes: usize) -> Result<(), ContentCodecError> {
        let attempted = self.input_bytes.saturating_add(bytes);
        let limit = self.limits.max_decoded_bytes().get();
        if attempted > limit {
            return Err(ContentLimitError::DecodedBytes { limit, attempted }.into());
        }
        self.input_bytes = attempted;
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), ContentCodecError> {
        if self.terminal {
            Err(ContentCodecError::AlreadyFinished {
                coding: self.coding,
            })
        } else {
            Ok(())
        }
    }
}

/// A content codec failed without exposing dependency-specific error types.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContentCodecError {
    /// A finite content resource limit was exceeded.
    #[error(transparent)]
    Limit(#[from] ContentLimitError),
    /// A codec's configured history-window bound is below its supported minimum.
    #[error(
        "{coding:?} decoder window limit {configured} bytes is below the minimum {minimum} bytes"
    )]
    DecoderWindowTooSmall {
        /// Codec requiring the history window.
        coding: ContentCoding,
        /// Smallest supported bound in bytes.
        minimum: usize,
        /// Configured bound in bytes.
        configured: usize,
    },
    /// Data appeared after terminal trailers.
    #[error("content codec received data after trailers")]
    DataAfterTrailers,
    /// More than one trailers frame appeared.
    #[error("content codec received duplicate trailers")]
    DuplicateTrailers,
    /// A completed or failed codec was used again.
    #[error("{coding:?} content codec is already terminal")]
    AlreadyFinished {
        /// Codec associated with the terminal state.
        coding: ContentCoding,
    },
    /// The compressed stream ended before a complete representation arrived.
    #[error("truncated {coding:?} content")]
    Truncated {
        /// Codec which detected truncation.
        coding: ContentCoding,
    },
    /// The compressed representation was malformed or failed validation.
    #[error("invalid {coding:?} content")]
    InvalidData {
        /// Codec which rejected the input.
        coding: ContentCoding,
    },
    /// A bounded output allocation could not be satisfied.
    #[error("unable to allocate bounded {coding:?} codec output")]
    ResourceExhausted {
        /// Codec whose output allocation failed.
        coding: ContentCoding,
    },
    /// The codec engine failed for another reason.
    #[error("{coding:?} content codec failed")]
    Codec {
        /// Codec which failed.
        coding: ContentCoding,
    },
}

#[derive(Debug)]
enum DecoderEngine {
    Gzip(AsyncGzipDecoder<ChunkReader>),
    Brotli(AsyncBrotliDecoder<ChunkReader>),
    Zlib(AsyncZlibDecoder<ChunkReader>),
    RawDeflate(AsyncDeflateDecoder<ChunkReader>),
    Zstd(AsyncZstdDecoder<ChunkReader>),
}

impl DecoderEngine {
    fn push(&mut self, bytes: Bytes) {
        match self {
            Self::Gzip(engine) => engine.get_mut().push(bytes),
            Self::Brotli(engine) => engine.get_mut().push(bytes),
            Self::Zlib(engine) => engine.get_mut().push(bytes),
            Self::RawDeflate(engine) => engine.get_mut().push(bytes),
            Self::Zstd(engine) => engine.get_mut().push(bytes),
        }
    }

    fn finish_input(&mut self) {
        match self {
            Self::Gzip(engine) => engine.get_mut().finish(),
            Self::Brotli(engine) => engine.get_mut().finish(),
            Self::Zlib(engine) => engine.get_mut().finish(),
            Self::RawDeflate(engine) => engine.get_mut().finish(),
            Self::Zstd(engine) => engine.get_mut().finish(),
        }
    }

    fn poll_read(
        &mut self,
        context: &mut Context<'_>,
        output: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut read = ReadBuf::new(output);
        let result = match self {
            Self::Gzip(engine) => Pin::new(engine).poll_read(context, &mut read),
            Self::Brotli(engine) => Pin::new(engine).poll_read(context, &mut read),
            Self::Zlib(engine) => Pin::new(engine).poll_read(context, &mut read),
            Self::RawDeflate(engine) => Pin::new(engine).poll_read(context, &mut read),
            Self::Zstd(engine) => Pin::new(engine).poll_read(context, &mut read),
        };
        match result {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read.filled().len())),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn has_buffered_input(&self) -> bool {
        match self {
            Self::Gzip(engine) => engine.get_ref().has_remaining(),
            Self::Brotli(engine) => engine.get_ref().has_remaining(),
            Self::Zlib(engine) => engine.get_ref().has_remaining(),
            Self::RawDeflate(engine) => engine.get_ref().has_remaining(),
            Self::Zstd(engine) => engine.get_ref().has_remaining(),
        }
    }

    fn select_raw_deflate(&mut self) {
        *self = Self::RawDeflate(AsyncDeflateDecoder::new(ChunkReader::new()));
    }
}

#[derive(Debug)]
struct ChunkReader {
    chunks: VecDeque<Bytes>,
    offset: usize,
    finished: bool,
}

impl ChunkReader {
    const fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            offset: 0,
            finished: false,
        }
    }

    fn push(&mut self, bytes: Bytes) {
        if !bytes.is_empty() {
            self.chunks.push_back(bytes);
        }
    }

    const fn finish(&mut self) {
        self.finished = true;
    }

    fn has_remaining(&self) -> bool {
        self.chunks
            .front()
            .is_some_and(|chunk| self.offset < chunk.len())
            || self.chunks.len() > 1
    }
}

impl AsyncRead for ChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.as_mut().poll_fill_buf(context) {
            Poll::Ready(Ok(available)) => {
                let written = available.len().min(output.remaining());
                output.put_slice(&available[..written]);
                self.consume(written);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncBufRead for ChunkReader {
    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<io::Result<&[u8]>> {
        while self
            .chunks
            .front()
            .is_some_and(|chunk| self.offset == chunk.len())
        {
            self.chunks.pop_front();
            self.offset = 0;
        }
        if self.chunks.is_empty() && !self.finished {
            return Poll::Pending;
        }
        let this = self.get_mut();
        Poll::Ready(Ok(this
            .chunks
            .front()
            .map_or(&[], |chunk| &chunk[this.offset..])))
    }

    fn consume(mut self: Pin<&mut Self>, amount: usize) {
        let available = self
            .chunks
            .front()
            .map_or(0, |chunk| chunk.len().saturating_sub(self.offset));
        self.offset = self.offset.saturating_add(amount.min(available));
    }
}

#[derive(Debug)]
enum EncoderEngine {
    Gzip(AsyncGzipEncoder<CaptureWriter>),
    Brotli(Box<AsyncBrotliEncoder<CaptureWriter>>),
    Zlib(AsyncZlibEncoder<CaptureWriter>),
    Zstd(AsyncZstdEncoder<CaptureWriter>),
}

impl EncoderEngine {
    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Gzip(engine) => engine.write_all(bytes).await,
            Self::Brotli(engine) => engine.write_all(bytes).await,
            Self::Zlib(engine) => engine.write_all(bytes).await,
            Self::Zstd(engine) => engine.write_all(bytes).await,
        }
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            Self::Gzip(engine) => engine.shutdown().await,
            Self::Brotli(engine) => engine.shutdown().await,
            Self::Zlib(engine) => engine.shutdown().await,
            Self::Zstd(engine) => engine.shutdown().await,
        }
    }
}

#[derive(Debug)]
enum CaptureFailure {
    Limit(ContentLimitError),
    Allocation,
}

#[derive(Debug)]
struct CaptureState {
    budget: ContentBudget,
    pending: Vec<u8>,
    failure: Option<CaptureFailure>,
}

type SharedCapture = Arc<Mutex<CaptureState>>;

#[derive(Clone, Debug)]
struct CaptureWriter {
    state: SharedCapture,
}

impl CaptureWriter {
    fn new(limits: ContentLimits) -> (Self, SharedCapture) {
        let state = Arc::new(Mutex::new(CaptureState {
            budget: ContentBudget::new(limits),
            pending: Vec::new(),
            failure: None,
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
    }
}

impl AsyncWrite for CaptureWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = lock_capture(&self.state);
        if state.failure.is_some() {
            return Poll::Ready(Err(io::Error::other("content capture already failed")));
        }
        let result = state.budget.record_output(bytes.len());
        if let Err(error) = result {
            state.failure = Some(CaptureFailure::Limit(error));
            return Poll::Ready(Err(io::Error::other("content limit exceeded")));
        }
        if state.pending.try_reserve(bytes.len()).is_err() {
            state.failure = Some(CaptureFailure::Allocation);
            return Poll::Ready(Err(io::Error::other("content output allocation failed")));
        }
        state.pending.extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

trait CaptureAccess {
    fn drain_data(&self, coding: ContentCoding) -> Result<Vec<BodyFrame>, ContentCodecError>;
}

impl CaptureAccess for SharedCapture {
    fn drain_data(&self, coding: ContentCoding) -> Result<Vec<BodyFrame>, ContentCodecError> {
        let mut state = lock_capture(self);
        if state.failure.is_some() {
            return Err(capture_failure(coding, &mut state));
        }
        if state.pending.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![BodyFrame::Data(Bytes::from(std::mem::take(
                &mut state.pending,
            )))])
        }
    }
}

fn lock_capture(capture: &SharedCapture) -> MutexGuard<'_, CaptureState> {
    capture
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn capture_failure(coding: ContentCoding, state: &mut CaptureState) -> ContentCodecError {
    match state.failure.take() {
        Some(CaptureFailure::Limit(error)) => error.into(),
        Some(CaptureFailure::Allocation) => ContentCodecError::ResourceExhausted { coding },
        None => ContentCodecError::Codec { coding },
    }
}

fn map_decode_io_error(coding: ContentCoding, error: &io::Error) -> ContentCodecError {
    match error.kind() {
        io::ErrorKind::UnexpectedEof => ContentCodecError::Truncated { coding },
        _ => ContentCodecError::InvalidData { coding },
    }
}

fn map_encode_io_error(coding: ContentCoding, capture: &SharedCapture) -> ContentCodecError {
    let mut state = lock_capture(capture);
    if state.failure.is_some() {
        capture_failure(coding, &mut state)
    } else {
        ContentCodecError::Codec { coding }
    }
}

const fn is_zlib_header(compression_method: u8, flags: u8) -> bool {
    compression_method & 0x0f == 8
        && compression_method >> 4 <= 7
        && u16::from_be_bytes([compression_method, flags]).is_multiple_of(31)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use rustymiddle_core::HeaderField;

    use super::*;

    fn limits(
        encoded: usize,
        decoded: usize,
        output: usize,
        ratio: usize,
        slack: usize,
    ) -> ContentLimits {
        limits_with_window(encoded, decoded, output, 16 * 1024 * 1024, ratio, slack)
    }

    fn limits_with_window(
        encoded: usize,
        decoded: usize,
        output: usize,
        decoder_window: usize,
        ratio: usize,
        slack: usize,
    ) -> ContentLimits {
        ContentLimits::new(
            NonZeroUsize::new(encoded).unwrap(),
            NonZeroUsize::new(decoded).unwrap(),
            NonZeroUsize::new(output).unwrap(),
            NonZeroUsize::new(decoder_window).unwrap(),
            NonZeroUsize::new(ratio).unwrap(),
            slack,
            NonZeroUsize::new(4).unwrap(),
        )
    }

    fn generous_limits() -> ContentLimits {
        limits(1024 * 1024, 1024 * 1024, 1024 * 1024, 1000, 1024 * 1024)
    }

    const fn enabled_codings() -> [ContentCoding; 4] {
        [
            ContentCoding::Gzip,
            ContentCoding::Brotli,
            ContentCoding::Deflate,
            ContentCoding::Zstd,
        ]
    }

    fn append_frames(data: &mut Vec<u8>, frames: Vec<BodyFrame>) -> Option<HeaderBlock> {
        let mut trailers = None;
        for frame in frames {
            match frame {
                BodyFrame::Data(bytes) => data.extend_from_slice(&bytes),
                BodyFrame::Trailers(value) => {
                    assert!(trailers.replace(value).is_none());
                }
            }
        }
        trailers
    }

    async fn encode_with_chunks(coding: ContentCoding, data: &[u8], chunk_size: usize) -> Vec<u8> {
        let mut codec = ContentEncoder::new(coding, generous_limits()).unwrap();
        let mut output = Vec::new();
        for chunk in data.chunks(chunk_size) {
            let frames = codec
                .on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk)))
                .await
                .unwrap();
            assert!(append_frames(&mut output, frames).is_none());
        }
        let frames = codec.finish().await.unwrap();
        assert!(append_frames(&mut output, frames).is_none());
        output
    }

    fn decode_with_chunks(
        coding: ContentCoding,
        data: &[u8],
        chunk_size: usize,
        codec_limits: ContentLimits,
    ) -> Result<Vec<u8>, ContentCodecError> {
        decode_with_options(
            coding,
            data,
            chunk_size,
            codec_limits,
            ContentDecoderOptions::default(),
        )
    }

    fn decode_with_options(
        coding: ContentCoding,
        data: &[u8],
        chunk_size: usize,
        codec_limits: ContentLimits,
        options: ContentDecoderOptions,
    ) -> Result<Vec<u8>, ContentCodecError> {
        let mut codec = ContentDecoder::with_options(coding, codec_limits, options).unwrap();
        let mut output = Vec::new();
        for chunk in data.chunks(chunk_size) {
            let frames = codec.on_frame(BodyFrame::Data(Bytes::copy_from_slice(chunk)))?;
            assert!(append_frames(&mut output, frames).is_none());
        }
        let frames = codec.finish()?;
        assert!(append_frames(&mut output, frames).is_none());
        Ok(output)
    }

    #[tokio::test]
    async fn enabled_codings_round_trip_across_single_byte_boundaries() {
        let input = b"streaming content across deliberately tiny frames";
        for coding in enabled_codings() {
            let encoded = encode_with_chunks(coding, input, 1).await;
            let decoded = decode_with_chunks(coding, &encoded, 1, generous_limits()).unwrap();
            assert_eq!(decoded, input, "{coding:?}");
        }
    }

    #[tokio::test]
    async fn fixed_vectors_decode_independently_of_our_encoder() {
        const GZIP_HELLO: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9,
            0xc9, 0x07, 0x00, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00,
        ];
        const BROTLI_HELLO: &[u8] = &[0x0b, 0x02, 0x80, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x03];
        const ZLIB_HELLO: &[u8] = &[
            0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];
        const ZSTD_HELLO: &[u8] = &[
            0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x58, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f,
        ];

        for (coding, encoded) in [
            (ContentCoding::Gzip, GZIP_HELLO),
            (ContentCoding::Brotli, BROTLI_HELLO),
            (ContentCoding::Deflate, ZLIB_HELLO),
            (ContentCoding::Zstd, ZSTD_HELLO),
        ] {
            assert_eq!(
                decode_with_chunks(coding, encoded, 1, generous_limits()).unwrap(),
                b"hello",
                "{coding:?}"
            );
        }
    }

    #[tokio::test]
    async fn output_is_deterministic_across_input_chunk_boundaries() {
        let input = vec![b'a'; 32 * 1024];
        for coding in enabled_codings() {
            let whole = encode_with_chunks(coding, &input, input.len()).await;
            let fragmented = encode_with_chunks(coding, &input, 37).await;
            assert_eq!(fragmented, whole, "{coding:?}");
        }
    }

    #[tokio::test]
    async fn empty_representations_are_valid_for_enabled_codings() {
        for coding in enabled_codings() {
            let encoded = encode_with_chunks(coding, b"", 1).await;
            assert!(!encoded.is_empty());
            assert_eq!(
                decode_with_chunks(coding, &encoded, 1, generous_limits()).unwrap(),
                b""
            );
        }
    }

    #[tokio::test]
    async fn terminal_trailers_follow_all_codec_output() {
        let trailers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("x-checkpoint", "complete").unwrap(),
        ]);
        for coding in enabled_codings() {
            let mut encoder = ContentEncoder::new(coding, generous_limits()).unwrap();
            let initial = encoder
                .on_frame(BodyFrame::Data(Bytes::from_static(b"body")))
                .await
                .unwrap();
            assert!(
                initial
                    .iter()
                    .all(|frame| matches!(frame, BodyFrame::Data(_)))
            );
            assert!(
                encoder
                    .on_frame(BodyFrame::Trailers(trailers.clone()))
                    .await
                    .unwrap()
                    .is_empty()
            );
            let final_frames = encoder.finish().await.unwrap();
            assert!(
                matches!(final_frames.last(), Some(BodyFrame::Trailers(value)) if value == &trailers)
            );
            assert!(
                final_frames[..final_frames.len() - 1]
                    .iter()
                    .all(|frame| matches!(frame, BodyFrame::Data(_)))
            );
        }
    }

    #[tokio::test]
    async fn malformed_and_truncated_streams_fail_closed() {
        for coding in enabled_codings() {
            let malformed = vec![0xff; 32];
            let malformed_error =
                decode_with_chunks(coding, &malformed, 3, generous_limits()).unwrap_err();
            assert!(
                matches!(
                    malformed_error,
                    ContentCodecError::InvalidData { .. }
                        | ContentCodecError::Truncated { .. }
                        | ContentCodecError::Codec { .. }
                ),
                "{coding:?}: {malformed_error:?}"
            );

            let encoded = encode_with_chunks(coding, b"complete source body", 4).await;
            let truncated = &encoded[..encoded.len() - 1];
            let truncated_error =
                decode_with_chunks(coding, truncated, 2, generous_limits()).unwrap_err();
            assert!(
                matches!(
                    truncated_error,
                    ContentCodecError::InvalidData { .. }
                        | ContentCodecError::Truncated { .. }
                        | ContentCodecError::Codec { .. }
                ),
                "{coding:?}: {truncated_error:?}"
            );
        }
    }

    #[tokio::test]
    async fn gzip_checksum_corruption_is_rejected() {
        let mut encoded = encode_with_chunks(ContentCoding::Gzip, b"checksum body", 3).await;
        let last = encoded.len() - 1;
        encoded[last] ^= 0x80;
        assert!(decode_with_chunks(ContentCoding::Gzip, &encoded, 1, generous_limits()).is_err());
    }

    #[tokio::test]
    async fn every_concatenated_gzip_member_is_decoded_and_validated() {
        let first = encode_with_chunks(ContentCoding::Gzip, b"first", 5).await;
        let second = encode_with_chunks(ContentCoding::Gzip, b"second", 6).await;
        let concatenated = [first.clone(), second.clone()].concat();
        assert_eq!(
            decode_with_chunks(ContentCoding::Gzip, &concatenated, 1, generous_limits()).unwrap(),
            b"firstsecond"
        );

        let mut trailing_junk = concatenated;
        trailing_junk.extend_from_slice(b"not another gzip member");
        assert!(
            decode_with_chunks(
                ContentCoding::Gzip,
                &trailing_junk,
                trailing_junk.len(),
                generous_limits()
            )
            .is_err()
        );

        let mut corrupt_second = second;
        let checksum_byte = corrupt_second.len() - 8;
        corrupt_second[checksum_byte] ^= 0x80;
        assert!(
            decode_with_chunks(
                ContentCoding::Gzip,
                &[first, corrupt_second].concat(),
                1,
                generous_limits()
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn bytes_after_a_complete_brotli_stream_are_rejected() {
        let mut encoded = encode_with_chunks(ContentCoding::Brotli, b"complete", 3).await;
        encoded.extend_from_slice(b"trailing junk");
        assert!(
            decode_with_chunks(
                ContentCoding::Brotli,
                &encoded,
                encoded.len(),
                generous_limits()
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn zstd_concatenated_frames_and_decoder_window_bound_are_enforced() {
        let first = encode_with_chunks(ContentCoding::Zstd, b"first", 5).await;
        let second = encode_with_chunks(ContentCoding::Zstd, b"second", 6).await;
        let concatenated = [first, second].concat();
        assert_eq!(
            decode_with_chunks(ContentCoding::Zstd, &concatenated, 1, generous_limits()).unwrap(),
            b"firstsecond"
        );

        let source = vec![b'z'; 64 * 1024];
        let encoded = encode_with_chunks(ContentCoding::Zstd, &source, source.len()).await;
        let small_window = limits_with_window(
            encoded.len(),
            source.len(),
            source.len(),
            1024,
            1000,
            source.len(),
        );
        assert!(matches!(
            decode_with_chunks(ContentCoding::Zstd, &encoded, encoded.len(), small_window),
            Err(ContentCodecError::InvalidData {
                coding: ContentCoding::Zstd
            })
        ));
    }

    #[test]
    fn raw_deflate_compatibility_is_explicit_and_header_driven() {
        const RAW_DEFLATE_HELLO: &[u8] = &[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
        const ZLIB_HELLO: &[u8] = &[
            0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];

        assert!(
            decode_with_chunks(
                ContentCoding::Deflate,
                RAW_DEFLATE_HELLO,
                1,
                generous_limits()
            )
            .is_err()
        );

        let compatibility =
            ContentDecoderOptions::new().with_deflate_compatibility(DeflateCompatibility::AllowRaw);
        assert_eq!(
            decode_with_options(
                ContentCoding::Deflate,
                RAW_DEFLATE_HELLO,
                1,
                generous_limits(),
                compatibility
            )
            .unwrap(),
            b"hello"
        );
        assert_eq!(
            decode_with_options(
                ContentCoding::Deflate,
                ZLIB_HELLO,
                1,
                generous_limits(),
                compatibility
            )
            .unwrap(),
            b"hello"
        );
    }

    #[tokio::test]
    async fn encoded_decoded_ratio_and_output_limits_are_typed() {
        for coding in enabled_codings() {
            let encoded = encode_with_chunks(coding, &vec![b'z'; 8192], 8192).await;

            let encoded_error = decode_with_chunks(
                coding,
                &encoded,
                encoded.len(),
                limits(encoded.len() - 1, 16 * 1024, 16 * 1024, 1000, 16 * 1024),
            )
            .unwrap_err();
            assert!(matches!(
                encoded_error,
                ContentCodecError::Limit(ContentLimitError::EncodedBytes { .. })
            ));

            let decoded_error = decode_with_chunks(
                coding,
                &encoded,
                encoded.len(),
                limits(encoded.len(), 8191, 16 * 1024, 1000, 16 * 1024),
            )
            .unwrap_err();
            assert!(matches!(
                decoded_error,
                ContentCodecError::Limit(ContentLimitError::DecodedBytes { .. })
            ));

            let ratio_error = decode_with_chunks(
                coding,
                &encoded,
                encoded.len(),
                limits(encoded.len(), 16 * 1024, 16 * 1024, 2, 16),
            )
            .unwrap_err();
            assert!(matches!(
                ratio_error,
                ContentCodecError::Limit(ContentLimitError::ExpansionRatio { .. })
            ));
        }

        let mut encoder =
            ContentEncoder::new(ContentCoding::Brotli, limits(1024, 1024, 1, 100, 1024)).unwrap();
        let first = encoder
            .on_frame(BodyFrame::Data(Bytes::from_static(b"output limit")))
            .await;
        let output_error = match first {
            Err(error) => error,
            Ok(_) => encoder.finish().await.unwrap_err(),
        };
        assert!(matches!(
            output_error,
            ContentCodecError::Limit(ContentLimitError::OutputBytes { .. })
        ));

        let mut encoder =
            ContentEncoder::new(ContentCoding::Gzip, limits(1024, 4, 1024, 100, 1024)).unwrap();
        assert!(matches!(
            encoder
                .on_frame(BodyFrame::Data(Bytes::from_static(b"12345")))
                .await,
            Err(ContentCodecError::Limit(ContentLimitError::DecodedBytes {
                limit: 4,
                attempted: 5
            }))
        ));
    }

    #[tokio::test]
    async fn sequencing_and_terminal_state_are_enforced() {
        let trailers = HeaderBlock::new();
        let mut decoder = ContentDecoder::new(ContentCoding::Gzip, generous_limits()).unwrap();
        decoder
            .on_frame(BodyFrame::Trailers(trailers.clone()))
            .unwrap();
        assert_eq!(
            decoder.on_frame(BodyFrame::Data(Bytes::from_static(b"late"))),
            Err(ContentCodecError::DataAfterTrailers)
        );
        assert!(matches!(
            decoder.finish(),
            Err(ContentCodecError::AlreadyFinished { .. })
        ));

        let mut encoder = ContentEncoder::new(ContentCoding::Brotli, generous_limits()).unwrap();
        encoder
            .on_frame(BodyFrame::Trailers(trailers.clone()))
            .await
            .unwrap();
        assert_eq!(
            encoder.on_frame(BodyFrame::Trailers(trailers)).await,
            Err(ContentCodecError::DuplicateTrailers)
        );

        let mut encoder = ContentEncoder::new(ContentCoding::Gzip, generous_limits()).unwrap();
        encoder.finish().await.unwrap();
        assert!(matches!(
            encoder.finish().await,
            Err(ContentCodecError::AlreadyFinished { .. })
        ));
    }

    #[test]
    fn invalid_codec_configuration_is_typed() {
        let too_small = limits_with_window(1024, 1024, 1024, 1023, 100, 1024);
        assert!(matches!(
            ContentDecoder::new(ContentCoding::Zstd, too_small),
            Err(ContentCodecError::DecoderWindowTooSmall {
                coding: ContentCoding::Zstd,
                minimum: 1024,
                configured: 1023
            })
        ));
    }

    #[test]
    fn codec_state_can_move_with_an_owned_async_body_task() {
        fn assert_send<T: Send>() {}
        assert_send::<ContentDecoder>();
        assert_send::<ContentEncoder>();
    }
}
