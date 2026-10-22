//! Chromium-style AIA fetches. HTTP is deliberately independent of upstream TLS.
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    runtime::{Handle, Runtime},
    sync::Semaphore,
    task::JoinHandle,
};
pub use transmog_tls_native::{IssuerRequest, IssuerSource};

const MAX_RESPONSE: usize = 65536;
const MAX_ENCODED_RESPONSE: usize = 256 * 1024;
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
const CACHE_LIFETIME: Duration = Duration::from_hours(168);
#[derive(Debug)]
struct Entry {
    der: Vec<u8>,
    used: Instant,
}
/// Shared bounded cache. Only fully verified intermediates enter it, always untrusted.
#[derive(Debug, Default)]
pub struct HttpIssuerSource {
    cache: Mutex<VecDeque<Entry>>,
}
impl IssuerSource for HttpIssuerSource {
    fn start(
        &self,
        urls: &[String],
        timeout: Duration,
        cancelled: Arc<AtomicBool>,
    ) -> Box<dyn IssuerRequest> {
        let (runtime, owned) = if let Ok(handle) = Handle::try_current() {
            (handle, None)
        } else {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return Box::new(EmptyRequest);
            };
            (runtime.handle().clone(), Some(runtime))
        };
        let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(10));
        let jobs = urls.iter().take(5).map(|url| {
            let url = url.clone();
            let cancelled = cancelled.clone();
            runtime.spawn(async move {
                tokio::select! {
                    result = tokio::time::timeout_at(deadline, fetch(url)) => result.ok().flatten().unwrap_or_default(),
                    () = cancellation(cancelled) => Vec::new(),
                }
            })
        }).collect();
        Box::new(HttpRequest {
            jobs,
            runtime,
            owned,
        })
    }
    fn cached_issuers(&self) -> Vec<Vec<u8>> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.used.elapsed() < CACHE_LIFETIME);
        cache.iter().map(|entry| entry.der.clone()).collect()
    }
    fn cache_verified_path(&self, path: &[Vec<u8>]) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.used.elapsed() < CACHE_LIFETIME);
        for der in path.iter().skip(1).take(path.len().saturating_sub(2)) {
            if der.len() > MAX_RESPONSE {
                continue;
            }
            cache.retain(|entry| entry.der != *der);
            cache.push_back(Entry {
                der: der.clone(),
                used: Instant::now(),
            });
        }
        while cache.len() > 256
            || cache.iter().map(|entry| entry.der.len()).sum::<usize>() > MAX_CACHE_BYTES
        {
            cache.pop_front();
        }
    }
}
struct EmptyRequest;
impl IssuerRequest for EmptyRequest {
    fn next(&mut self) -> Option<Vec<u8>> {
        None
    }
}
struct HttpRequest {
    jobs: VecDeque<JoinHandle<Vec<u8>>>,
    runtime: Handle,
    owned: Option<Runtime>,
}
impl IssuerRequest for HttpRequest {
    fn next(&mut self) -> Option<Vec<u8>> {
        let job = self.jobs.pop_front()?;
        let mut guard = AbortOnDrop(job);
        let next = async { (&mut guard.0).await.unwrap_or_default() };
        Some(if let Some(runtime) = &self.owned {
            runtime.block_on(next)
        } else {
            self.runtime.block_on(next)
        })
    }
}
impl Drop for HttpRequest {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.abort();
        }
    }
}
struct AbortOnDrop<T>(JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn cancellation(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn fetch(input: String) -> Option<Vec<u8>> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let _slot = SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(64)))
        .acquire()
        .await
        .ok()?;
    let mut url = url::Url::parse(&input).ok()?;
    for _ in 0..=20 {
        // Match Chromium's cert fetcher: HTTP only on every redirect, with no credentials.
        if url.scheme() != "http" {
            return None;
        }
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
        url.set_fragment(None);
        let host = url.host_str()?.trim_matches(['[', ']']);
        let socket = tokio::net::TcpStream::connect((host, url.port_or_known_default()?))
            .await
            .ok()?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_headers(64)
            .max_buf_size(MAX_RESPONSE)
            .handshake::<_, Empty<Bytes>>(TokioIo::new(socket))
            .await
            .ok()?;
        let _driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(&url[url::Position::BeforePath..url::Position::AfterQuery])
            .header(
                http::header::HOST,
                &url[url::Position::BeforeHost..url::Position::AfterPort],
            )
            .header(http::header::CONNECTION, "close")
            .header(http::header::ACCEPT_ENCODING, "gzip, deflate")
            .body(Empty::<Bytes>::new())
            .ok()?;
        let mut response = sender.send_request(request).await.ok()?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(http::header::LOCATION)?
                .to_str()
                .ok()?;
            url = url.join(location).ok()?;
            continue;
        }
        if response.status() != http::StatusCode::OK {
            return None;
        }
        let encoding = response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .ok()?
            .unwrap_or_default();
        if response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok())
            .is_some_and(|len| len > MAX_ENCODED_RESPONSE)
        {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(frame) = response.body_mut().frame().await {
            if let Ok(data) = frame.ok()?.into_data() {
                if data.len() > MAX_ENCODED_RESPONSE.saturating_sub(bytes.len()) {
                    return None;
                }
                bytes.extend_from_slice(&data);
            }
        }
        return decode_body(bytes, &encoding).await;
    }
    None
}

async fn decode_body(mut bytes: Vec<u8>, encoding: &str) -> Option<Vec<u8>> {
    use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder};
    use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
    let encodings: Vec<_> = encoding
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if encodings.len() > 4 {
        return None;
    }
    for encoding in encodings.into_iter().rev() {
        let reader = BufReader::new(std::io::Cursor::new(bytes));
        let mut decoder: Box<dyn AsyncRead + Send + Unpin> =
            match encoding.to_ascii_lowercase().as_str() {
                "gzip" | "x-gzip" => Box::new(GzipDecoder::new(reader)),
                "deflate" => Box::new(ZlibDecoder::new(reader)),
                "br" => Box::new(BrotliDecoder::new(reader)),
                "identity" => Box::new(reader),
                _ => return None,
            };
        bytes = Vec::new();
        decoder
            .as_mut()
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > MAX_RESPONSE {
            return None;
        }
    }
    (bytes.len() <= MAX_RESPONSE).then_some(bytes)
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
/// Offloads the one verifier; the TCP/QUIC driver must set its TLS task waker.
/// # Errors
/// Returns native callback-data allocation errors.
pub fn enable_async_certificate_verification(
    context: &mut boring::ssl::SslContextBuilder,
) -> Result<(), boring::error::ErrorStack> {
    transmog_tls_native::enable_async_verification(context, |input| {
        Box::pin(async move {
            static WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
            let _cancel = Cancel(input.cancellation());
            let Ok(slot) = WORKERS
                .get_or_init(|| Arc::new(Semaphore::new(16)))
                .clone()
                .acquire_owned()
                .await
            else {
                return transmog_tls_native::CompletedVerification::failure(
                    "Certificate verifier unavailable",
                );
            };
            crate::blocking_job::run(move |_| {
                let _slot = slot;
                input.verify()
            })
            .await
            .unwrap_or_else(|_| {
                transmog_tls_native::CompletedVerification::failure(
                    "Certificate verification worker failed",
                )
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn compressed_responses_obey_decoded_size_limit() {
        for size in [65536, 65537] {
            let mut encoder = async_compression::tokio::write::GzipEncoder::new(Vec::new());
            encoder.write_all(&vec![b'a'; size]).await.unwrap();
            encoder.shutdown().await.unwrap();
            let compressed = encoder.into_inner();
            assert!(compressed.len() < 1024);
            let body = decode_body(compressed, "gzip").await;
            assert_eq!(body.is_some(), size == MAX_RESPONSE);
        }
        assert!(decode_body(vec![0; 65537], "").await.is_none());
        assert!(decode_body(vec![0; 10], "unknown").await.is_none());
    }

    #[tokio::test]
    async fn cancellation_and_timeout_close_active_http_connections() {
        for cancel in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/issuer", listener.local_addr().unwrap());
            let started = Arc::new(tokio::sync::Notify::new());
            let seen = started.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = [0; 2048];
                assert!(socket.read(&mut input).await.unwrap() > 0);
                seen.notify_one();
                assert_eq!(
                    socket.read(&mut input).await.unwrap(),
                    0,
                    "Cancelled issuer connection remained open"
                );
            });
            let flag = Arc::new(AtomicBool::new(false));
            let cancelled = flag.clone();
            let worker = tokio::task::spawn_blocking(move || {
                let source = HttpIssuerSource::default();
                let mut request = source.start(&[url], Duration::from_millis(150), cancelled);
                assert_eq!(request.next().unwrap(), [] as [u8; 0]);
            });
            tokio::time::timeout(Duration::from_secs(2), started.notified())
                .await
                .unwrap();
            if cancel {
                flag.store(true, Ordering::Release);
            }
            tokio::time::timeout(Duration::from_secs(2), worker)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn request_drop_aborts_every_parallel_fetch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/issuer", listener.local_addr().unwrap());
        let source = HttpIssuerSource::default();
        let request = source.start(&[url.clone(), url], Duration::from_secs(10), Arc::default());
        let mut sockets = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut bytes = [0; 2048];
            assert!(socket.read(&mut bytes).await.unwrap() > 0);
            // A partial response keeps both jobs waiting on a body.
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\na")
                .await
                .unwrap();
            sockets.push(socket);
        }
        drop(request);
        for mut socket in sockets {
            let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut [0; 1]))
                .await
                .unwrap();
            // Windows may reset a socket closed with unread response data.
            assert!(
                matches!(closed, Ok(0))
                    || closed
                        .as_ref()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset),
                "Fetch remained open: {closed:?}"
            );
        }
    }
}
