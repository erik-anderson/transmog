use super::common::{Error, LIMIT, Shared, acceptor, hello, hex, tls};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::json;
use std::{
    convert::Infallible,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use transmog_tls::IssuedLeaf;

struct Recorded<T> {
    io: T,
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl<T: AsyncRead + Unpin> AsyncRead for Recorded<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.io).poll_read(cx, buffer);
        if result.is_ready() {
            let mut bytes = self.bytes.lock().unwrap();
            let next = &buffer.filled()[before..];
            if bytes.len() + next.len() > LIMIT {
                return Poll::Ready(Err(std::io::Error::other("probe capture limit exceeded")));
            }
            bytes.extend_from_slice(next);
        }
        result
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for Recorded<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
async fn connection(
    socket: tokio::net::TcpStream,
    leaf: IssuedLeaf,
    mode: String,
    shared: Arc<Shared>,
) -> Result<(), Error> {
    let peer = socket.peer_addr()?;
    let local = socket.local_addr()?;
    let hello = hello();
    let alpn: &[u8] = if mode == "h2" {
        b"\x02h2"
    } else {
        b"\x08http/1.1"
    };
    let acceptor = acceptor(&leaf, alpn, &hello)?;
    let stream = tokio_boring::accept(&acceptor, socket).await?;
    let tls = tls(stream.ssl(), &hello);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let captured = bytes.clone();
    let service = service_fn(move |request: Request<Incoming>| {
        let shared = shared.clone();
        let tls = tls.clone();
        let bytes = captured.clone();
        async move {
            if request.uri().path() == "/probe" {
                let record = json!({"protocol":if request.version() == http::Version::HTTP_2 {"h2"} else {"h1"}, "peer":peer.to_string(), "local":local.to_string(), "tls":tls, "application_bytes":hex(&bytes.lock().unwrap())});
                if let Err(error) = shared.save(&record) {
                    eprintln!("record: {error}");
                }
            }
            Ok::<_, Infallible>(
                Response::builder()
                    .header("content-type", "text/html")
                    .header("x-local-probe", &shared.proof)
                    .body(Full::new(Bytes::from(shared.body())))
                    .unwrap(),
            )
        }
    });
    let stream = TokioIo::new(Recorded { io: stream, bytes });
    if mode == "h2" {
        hyper::server::conn::http2::Builder::new(TokioExecutor::new())
            .serve_connection(stream, service)
            .await?;
    } else {
        hyper::server::conn::http1::Builder::new()
            .serve_connection(stream, service)
            .await?;
    }
    Ok(())
}
pub async fn serve(
    listener: tokio::net::TcpListener,
    leaf: IssuedLeaf,
    mode: String,
    shared: Arc<Shared>,
) {
    while let Ok((socket, _)) = listener.accept().await {
        let leaf = leaf.clone();
        let mode = mode.clone();
        let shared = shared.clone();
        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(20),
                connection(socket, leaf, mode, shared),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("TCP connection: {error}"),
                Err(error) => eprintln!("TCP timeout: {error}"),
            }
        });
    }
}
