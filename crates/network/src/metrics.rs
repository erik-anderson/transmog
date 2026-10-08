//! Socket payload counters, before TLS and independent of HTTP stream framing.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Instant,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Shared physical socket counters. These measure socket payloads, including TLS
/// records; they do not count TCP/IP headers or kernel retransmissions.
#[derive(Debug, Default)]
pub struct ConnectionMetrics {
    read: AtomicU64,
    written: AtomicU64,
    first_read: Mutex<Option<Instant>>,
}
impl ConnectionMetrics {
    /// Total socket payload bytes read so far, shared across all HTTP streams.
    pub fn bytes_read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }
    /// Total socket payload bytes written so far, shared across all HTTP streams.
    pub fn bytes_written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
    /// First successful nonempty read, if any.
    pub fn first_read_at(&self) -> Option<Instant> {
        *self
            .first_read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn read(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.read.fetch_add(count as u64, Ordering::Relaxed);
        let mut first = self
            .first_read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if first.is_none() {
            *first = Some(Instant::now());
        }
    }
    fn written(&self, count: usize) {
        self.written.fetch_add(count as u64, Ordering::Relaxed);
    }
}

/// Transparent Tokio I/O wrapper preserving backpressure and vectored writes.
#[derive(Debug)]
pub struct MeteredIo<T> {
    inner: T,
    metrics: Arc<ConnectionMetrics>,
}
impl<T> MeteredIo<T> {
    /// Wraps one physical socket with shared counters.
    pub fn new(inner: T, metrics: Arc<ConnectionMetrics>) -> Self {
        Self { inner, metrics }
    }
    /// Borrows the underlying socket for connection metadata.
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
}
impl<T: AsyncRead + Unpin> AsyncRead for MeteredIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Ok(()))) {
            self.metrics.read(buffer.filled().len() - before);
        }
        result
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for MeteredIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, bytes);
        if let Poll::Ready(Ok(count)) = result {
            self.metrics.written(count);
        }
        result
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write_vectored(cx, buffers);
        if let Poll::Ready(Ok(count)) = result {
            self.metrics.written(count);
        }
        result
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn counts_only_completed_payload_reads_and_partial_writes() {
        let (socket, mut peer) = tokio::io::duplex(4);
        let metrics = Arc::new(ConnectionMetrics::default());
        let mut io = MeteredIo::new(socket, metrics.clone());
        assert_eq!(metrics.first_read_at(), None);
        assert_eq!(io.write(b"abcdef").await.unwrap(), 4);
        assert_eq!(metrics.bytes_written(), 4);
        let mut sent = [0; 4];
        peer.read_exact(&mut sent).await.unwrap();
        assert_eq!(&sent, b"abcd");
        peer.write_all(b"xyz").await.unwrap();
        let mut read = [0; 3];
        io.read_exact(&mut read).await.unwrap();
        assert_eq!(metrics.bytes_read(), 3);
        let first = metrics.first_read_at();
        assert!(first.is_some());
        peer.shutdown().await.unwrap();
        assert_eq!(io.read(&mut read).await.unwrap(), 0);
        assert_eq!(metrics.bytes_read(), 3);
        assert_eq!(metrics.first_read_at(), first);
    }
}
