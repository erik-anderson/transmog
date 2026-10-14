//! The small Hyper transport boundary shared by our own TLS connectors.
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::{
    client::legacy::connect::{Connected, Connection},
    rt::TokioIo,
};
use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_boring::SslStream;

/// An origin transport with optional TLS, implementing Hyper's I/O traits.
pub enum MaybeHttpsStream<T> {
    /// An unencrypted HTTP connection.
    Http(T),
    /// An HTTPS connection negotiated by Transmog's TLS connector.
    Https(SslStream<T>),
}

impl<T> fmt::Debug for MaybeHttpsStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SSL's Debug implementation accesses legacy X.509 state, which is
        // incompatible with the buffer-based certificate verifier.
        f.write_str(match self {
            Self::Http(_) => "Http(..)",
            Self::Https(_) => "Https(..)",
        })
    }
}

impl<T: Connection> Connection for MaybeHttpsStream<T> {
    fn connected(&self) -> Connected {
        match self {
            Self::Http(stream) => stream.connected(),
            Self::Https(stream) => {
                let connected = stream.get_ref().connected();
                if stream.ssl().selected_alpn_protocol() == Some(b"h2") {
                    connected.negotiated_h2()
                } else {
                    connected
                }
            }
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Read for MaybeHttpsStream<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(&mut TokioIo::new(stream)).poll_read(cx, output),
            Self::Https(stream) => Pin::new(&mut TokioIo::new(stream)).poll_read(cx, output),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> Write for MaybeHttpsStream<T> {
    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Http(stream) => stream.is_write_vectored(),
            Self::Https(stream) => stream.is_write_vectored(),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_write_vectored(cx, buffers),
            Self::Https(stream) => Pin::new(stream).poll_write_vectored(cx, buffers),
        }
    }

    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        input: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_write(cx, input),
            Self::Https(stream) => Pin::new(stream).poll_write(cx, input),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_flush(cx),
            Self::Https(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Http(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Https(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
