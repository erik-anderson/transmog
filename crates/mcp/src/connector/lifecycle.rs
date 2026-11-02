//! Bounded protocol shutdown, independent of presentation activity and grants.
use super::Connector;
use crate::Error;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::watch,
};

impl Connector {
    pub(crate) async fn serve_io(
        self,
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
        initialization_timeout: Duration,
    ) -> Result<(), Error> {
        let (failed, mut failure) = watch::channel(None);
        let reader = DisconnectReader {
            reader,
            failed: failed.clone(),
        };
        let writer = DisconnectWriter { writer, failed };
        let lifetime = self.clone();
        let initialize = rmcp::ServiceExt::serve(self, (reader, writer));
        let service = tokio::select! {
            result = tokio::time::timeout(initialization_timeout, initialize) => match result {
                Ok(Ok(service)) => service,
                Ok(Err(rmcp::service::ServerInitializeError::ConnectionClosed(_))) => return Ok(()),
                Ok(Err(_)) => return Err(transport_error("The MCP stdio connection could not be initialized.")),
                Err(_) => return Err(transport_error("The MCP client did not initialize within 30 seconds.")),
            },
            _ = failure.changed() => return Ok(()),
        };
        let cancellation = service.cancellation_token();
        let waiting = service.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => {
                result.map_err(|_| transport_error("The MCP stdio connection ended unexpectedly."))?;
                tracing::info!("MCP connector stopped: stdio closed");
            },
            reason = async {
                tokio::select! {
                    _ = failure.changed() => (*failure.borrow()).unwrap_or("stdio closed"),
                    () = lifetime.instance_ended() => "associated desktop stopped",
                }
            } => {
                tracing::info!("MCP connector stopped: {reason}");
                cancellation.cancel();
                // A blocked output pipe must not prevent process shutdown either.
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut waiting).await;
            },
        }
        Ok(())
    }

    async fn instance_ended(&self) {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if self
                .binding
                .lock()
                .await
                .as_ref()
                .is_some_and(|bound| !bound.is_alive(&self.client))
            {
                return;
            }
        }
    }
}

fn transport_error(message: &str) -> Error {
    Error::new("mcp_transport", message)
}

// Observe EOF directly so SDK request-draining cannot extend shutdown. The SDK
// may also ignore failed response writes while continuing to read input.
struct DisconnectReader<R> {
    reader: R,
    failed: watch::Sender<Option<&'static str>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for DisconnectReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = bytes.filled().len();
        let can_read = bytes.remaining() > 0;
        let result = Pin::new(&mut this.reader).poll_read(cx, bytes);
        if matches!(&result, Poll::Ready(Err(_)))
            || (can_read
                && matches!(&result, Poll::Ready(Ok(())))
                && bytes.filled().len() == before)
        {
            this.failed.send_replace(Some("stdio input closed"));
        }
        result
    }
}

struct DisconnectWriter<W> {
    writer: W,
    failed: watch::Sender<Option<&'static str>>,
}

impl<W> DisconnectWriter<W> {
    fn observe<T>(&self, result: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if matches!(&result, Poll::Ready(Err(_))) {
            self.failed.send_replace(Some("stdio output closed"));
        }
        result
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for DisconnectWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_write(cx, bytes);
        if !bytes.is_empty() && matches!(result, Poll::Ready(Ok(0))) {
            return this.observe(Poll::Ready(Err(io::ErrorKind::WriteZero.into())));
        }
        this.observe(result)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_flush(cx);
        this.observe(result)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.writer).poll_shutdown(cx);
        this.observe(result)
    }
}
