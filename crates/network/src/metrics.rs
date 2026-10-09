//! Physical socket payloads, write waits and read-only kernel TCP statistics.
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use transmog_core::performance::{
    PerformanceRecorder, SocketIoObservation, TcpObservation, TransportObservation,
};

/// Shared physical connection evidence, independent of HTTP streams.
#[derive(Debug, Default)]
pub struct ConnectionMetrics {
    read: AtomicU64,
    written: AtomicU64,
    first_read: Mutex<Option<Instant>>,
    state: Mutex<SocketState>,
}
#[derive(Debug, Default)]
struct SocketState {
    tcp: Option<(Instant, TcpObservation)>,
    last_query: Option<Instant>,
    pending_write: Option<Instant>,
    write_wait: Duration,
    write_waits: u64,
    last_write: Option<Instant>,
    last_flush: Option<Instant>,
}
impl ConnectionMetrics {
    fn state(&self) -> std::sync::MutexGuard<'_, SocketState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Total socket payload bytes read, including TLS and shared streams.
    pub fn bytes_read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }
    /// Total socket payload bytes accepted by the socket, excluding TCP/IP framing.
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
    /// Projects cached kernel statistics and local socket observations onto a request.
    /// Kernel queries happen only while the owning socket is borrowed and alive.
    pub fn observe(&self, observation: &mut TransportObservation, recorder: &PerformanceRecorder) {
        let state = self.state();
        if let Some((at, tcp)) = &state.tcp {
            observation.tcp = Some(tcp.clone());
            observation.tcp_sampled_offset_micros = Some(recorder.offset_micros(*at));
        }
        let wait = state.write_wait.saturating_add(
            state
                .pending_write
                .map_or(Duration::ZERO, |at| at.elapsed()),
        );
        observation.socket_io = Some(SocketIoObservation {
            write_wait_micros: u64::try_from(wait.as_micros()).unwrap_or(u64::MAX),
            write_waits: state.write_waits,
            last_write_offset_micros: state.last_write.map(|at| recorder.offset_micros(at)),
            last_flush_offset_micros: state.last_flush.map(|at| recorder.offset_micros(at)),
        });
    }
    fn read(&self, count: usize) {
        if count == 0 {
            return;
        }
        self.read.fetch_add(count as u64, Ordering::Relaxed);
        self.first_read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(Instant::now);
    }
    fn write_result(&self, pending: bool, count: usize, flushed: bool) {
        let now = Instant::now();
        let mut state = self.state();
        if pending {
            if state.pending_write.is_none() {
                state.pending_write = Some(now);
                state.write_waits = state.write_waits.saturating_add(1);
            }
        } else if let Some(began) = state.pending_write.take() {
            state.write_wait = state
                .write_wait
                .saturating_add(now.saturating_duration_since(began));
        }
        if count > 0 {
            self.written.fetch_add(count as u64, Ordering::Relaxed);
            state.last_write = Some(now);
        }
        if flushed {
            state.last_flush = Some(now);
        }
    }
}

/// Transparent Tokio I/O wrapper preserving backpressure and vectored writes.
#[derive(Debug)]
pub struct MeteredIo<T> {
    inner: T,
    metrics: Arc<ConnectionMetrics>,
    sampler: Option<fn(&T) -> Option<TcpObservation>>,
}
impl<T> MeteredIo<T> {
    /// Wraps any I/O object with shared byte and write-wait counters.
    pub fn new(inner: T, metrics: Arc<ConnectionMetrics>) -> Self {
        Self {
            inner,
            metrics,
            sampler: None,
        }
    }
    /// Borrows the underlying socket for connection metadata.
    pub fn get_ref(&self) -> &T {
        &self.inner
    }
    fn sample(&self, force: bool) {
        let Some(sampler) = self.sampler else {
            return;
        };
        let now = Instant::now();
        let mut state = self.metrics.state();
        if !force
            && state
                .last_query
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(25))
        {
            return;
        }
        state.last_query = Some(now);
        if let Some(info) = sampler(&self.inner) {
            state.tcp = Some((now, info));
        }
    }
}
impl MeteredIo<tokio::net::TcpStream> {
    /// Wraps a TCP socket and samples supported kernel statistics, at most every
    /// 25ms during I/O, plus setup and closure. Never retains native handles.
    pub fn new_tcp(inner: tokio::net::TcpStream, metrics: Arc<ConnectionMetrics>) -> Self {
        let result = Self {
            inner,
            metrics,
            sampler: Some(tcp_sample),
        };
        result.sample(true);
        result
    }
}
fn tcp_sample(socket: &tokio::net::TcpStream) -> Option<TcpObservation> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsSocket;
        transmog_socket_info::sample(socket.as_socket())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsFd;
        transmog_socket_info::sample(socket.as_fd())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = socket;
        None
    }
}
impl<T> Drop for MeteredIo<T> {
    fn drop(&mut self) {
        self.sample(true);
        self.metrics.write_result(false, 0, false);
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
        self.sample(false);
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
        self.metrics.write_result(
            result.is_pending(),
            match &result {
                Poll::Ready(Ok(count)) => *count,
                _ => 0,
            },
            false,
        );
        self.sample(false);
        result
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write_vectored(cx, buffers);
        self.metrics.write_result(
            result.is_pending(),
            match &result {
                Poll::Ready(Ok(count)) => *count,
                _ => 0,
            },
            false,
        );
        self.sample(false);
        result
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        self.metrics.write_result(
            result.is_pending(),
            0,
            matches!(result, Poll::Ready(Ok(()))),
        );
        self.sample(false);
        result
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
    async fn physical_write_waits_include_pending_and_finish_on_acceptance() {
        let (socket, mut peer) = tokio::io::duplex(1);
        let metrics = Arc::new(ConnectionMetrics::default());
        let mut io = MeteredIo::new(socket, metrics.clone());
        io.write_all(b"a").await.unwrap();
        let recorder = PerformanceRecorder::new(std::time::SystemTime::now(), Instant::now());
        let mut observation = TransportObservation::default();
        let mut write = Box::pin(io.write_all(b"b"));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(write.as_mut(), cx).is_pending());
            Poll::Ready(())
        })
        .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        metrics.observe(&mut observation, &recorder);
        let pending = observation.socket_io.as_ref().unwrap();
        assert_eq!(pending.write_waits, 1);
        assert!(pending.write_wait_micros >= 1000);
        let mut bytes = [0];
        peer.read_exact(&mut bytes).await.unwrap();
        write.await.unwrap();
        io.flush().await.unwrap();
        metrics.observe(&mut observation, &recorder);
        let completed = observation.socket_io.unwrap();
        assert_eq!(completed.write_waits, 1);
        assert!(completed.last_write_offset_micros.is_some());
        assert!(completed.last_flush_offset_micros.is_some());
        assert_eq!(metrics.bytes_written(), 2);
    }
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
