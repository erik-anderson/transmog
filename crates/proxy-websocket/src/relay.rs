use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::Notify,
    time::timeout,
};

use crate::{
    CompressionError, ControlFrame, ControlOutcome, DecodeError, Direction, EncodeError, Frame,
    FrameLimits, MessageDecoder, MessageEvent, MessageOutcome, NegotiatedExtensions,
    PerMessageDeflateCodec, WebSocketEffect, WebSocketHookChain, WebSocketHookError, encode_frame,
    encode_message,
};

/// Finite transport, framing, and close-handshake policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayLimits {
    /// Parser and message allocation bounds.
    pub frames: FrameLimits,
    /// Maximum payload emitted in one rewritten frame.
    pub outbound_frame_payload_bytes: usize,
    /// Size of each transport read buffer.
    pub read_buffer_bytes: usize,
    /// Maximum period without bytes in either inspected direction.
    pub idle_timeout: Duration,
    /// Maximum duration of one backpressured write.
    pub write_timeout: Duration,
    /// Time allowed for the peer's close response after one side closes.
    pub close_handshake_timeout: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            frames: FrameLimits::default(),
            outbound_frame_payload_bytes: 64 * 1024,
            read_buffer_bytes: 16 * 1024,
            idle_timeout: Duration::from_secs(120),
            write_timeout: Duration::from_secs(30),
            close_handshake_timeout: Duration::from_secs(5),
        }
    }
}

impl RelayLimits {
    /// Validates every relay bound.
    ///
    /// # Errors
    ///
    /// Returns [`RelayError::InvalidLimits`] when any scalar limit is zero.
    pub fn validate(self) -> Result<Self, RelayError> {
        if self.outbound_frame_payload_bytes == 0
            || self.read_buffer_bytes == 0
            || self.idle_timeout.is_zero()
            || self.write_timeout.is_zero()
            || self.close_handshake_timeout.is_zero()
        {
            return Err(RelayError::InvalidLimits);
        }
        Ok(self)
    }
}

/// Cloneable cancellation signal scoped to one upgraded session.
#[derive(Clone, Debug, Default)]
pub struct SessionCancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Debug, Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl SessionCancellation {
    /// Creates a live signal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels the session and wakes transport and hook waiters.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Whether cancellation has already been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Waits until cancellation is requested.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.inner.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Completed relay accounting and attributed decisions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelayReport {
    /// Bytes read client-to-server before any inspected reframing.
    pub client_to_server_wire_bytes: u64,
    /// Bytes read server-to-client before any inspected reframing.
    pub server_to_client_wire_bytes: u64,
    /// Complete application messages processed in both directions.
    pub messages: u64,
    /// Control frames processed in both directions.
    pub control_frames: u64,
    /// Whether both peers completed a close handshake.
    pub clean_close: bool,
    /// Centrally attributed hook decisions in completion order.
    pub effects: Vec<WebSocketEffect>,
}

/// Relays upgraded streams without interpreting or rewriting any byte.
///
/// This is the required fast path when no WebSocket hook is installed.
///
/// # Errors
///
/// Returns on transport failure, cancellation, or idle timeout.
pub async fn relay_transparent<Downstream, Upstream>(
    downstream: Downstream,
    upstream: Upstream,
    limits: RelayLimits,
    cancellation: SessionCancellation,
) -> Result<RelayReport, RelayError>
where
    Downstream: AsyncRead + AsyncWrite + Unpin,
    Upstream: AsyncRead + AsyncWrite + Unpin,
{
    let limits = limits.validate()?;
    let (downstream_read, downstream_write) = tokio::io::split(downstream);
    let (upstream_read, upstream_write) = tokio::io::split(upstream);
    let (client_to_server, server_to_client) = tokio::try_join!(
        copy_transparent_direction(
            downstream_read,
            upstream_write,
            limits,
            cancellation.clone(),
        ),
        copy_transparent_direction(upstream_read, downstream_write, limits, cancellation),
    )?;
    Ok(RelayReport {
        client_to_server_wire_bytes: client_to_server,
        server_to_client_wire_bytes: server_to_client,
        ..RelayReport::default()
    })
}

async fn copy_transparent_direction<Reader, Writer>(
    mut reader: Reader,
    mut writer: Writer,
    limits: RelayLimits,
    cancellation: SessionCancellation,
) -> Result<u64, RelayError>
where
    Reader: AsyncRead + Unpin,
    Writer: AsyncWrite + Unpin,
{
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; limits.read_buffer_bytes];
    loop {
        let read = tokio::select! {
            () = cancellation.cancelled() => return Err(RelayError::Cancelled),
            result = timeout(limits.idle_timeout, reader.read(&mut buffer)) => {
                result.map_err(|_| RelayError::IdleTimeout)??
            }
        };
        if read == 0 {
            writer.shutdown().await?;
            return Ok(copied);
        }
        copied = copied
            .checked_add(u64::try_from(read).map_err(|_| RelayError::AccountingOverflow)?)
            .ok_or(RelayError::AccountingOverflow)?;
        write_all_bounded(&mut writer, &buffer[..read], limits, &cancellation).await?;
    }
}

/// Relays upgraded streams through bounded message and control hooks.
///
/// The two directions execute independently, so pausing one direction never
/// pauses the reverse direction or another connection. Incoming compressed
/// messages are decompressed before hooks and safely recompressed afterward.
///
/// # Errors
///
/// Returns on protocol violations, limit failures, hook failure, transport
/// failure, cancellation, idle/write timeout, or an incomplete close race.
pub async fn relay_inspected<Downstream, Upstream>(
    downstream: Downstream,
    upstream: Upstream,
    negotiated: NegotiatedExtensions,
    limits: RelayLimits,
    hooks: Arc<WebSocketHookChain>,
    cancellation: SessionCancellation,
) -> Result<RelayReport, RelayError>
where
    Downstream: AsyncRead + AsyncWrite + Unpin,
    Upstream: AsyncRead + AsyncWrite + Unpin,
{
    let limits = limits.validate()?;
    let (downstream_read, downstream_write) = tokio::io::split(downstream);
    let (upstream_read, upstream_write) = tokio::io::split(upstream);
    let mut client_to_server = Box::pin(pump_direction(
        downstream_read,
        upstream_write,
        Direction::ClientToServer,
        negotiated.permessage_deflate,
        limits,
        Arc::clone(&hooks),
        cancellation.clone(),
    ));
    let mut server_to_client = Box::pin(pump_direction(
        upstream_read,
        downstream_write,
        Direction::ServerToClient,
        negotiated.permessage_deflate,
        limits,
        Arc::clone(&hooks),
        cancellation.clone(),
    ));

    let (client, server, clean_close) = tokio::select! {
        result = &mut client_to_server => {
            let first = result?;
            if first.end != PumpEnd::Close {
                cancellation.cancel();
                return Err(RelayError::UncleanEof(Direction::ClientToServer));
            }
            let second = timeout(limits.close_handshake_timeout, &mut server_to_client)
                .await
                .map_err(|_| RelayError::CloseHandshakeTimeout)??;
            if second.end != PumpEnd::Close {
                return Err(RelayError::UncleanEof(Direction::ServerToClient));
            }
            (first, second, true)
        }
        result = &mut server_to_client => {
            let first = result?;
            if first.end != PumpEnd::Close {
                cancellation.cancel();
                return Err(RelayError::UncleanEof(Direction::ServerToClient));
            }
            let second = timeout(limits.close_handshake_timeout, &mut client_to_server)
                .await
                .map_err(|_| RelayError::CloseHandshakeTimeout)??;
            if second.end != PumpEnd::Close {
                return Err(RelayError::UncleanEof(Direction::ClientToServer));
            }
            (second, first, true)
        }
        () = cancellation.cancelled() => return Err(RelayError::Cancelled),
    };
    Ok(RelayReport {
        client_to_server_wire_bytes: client.wire_bytes,
        server_to_client_wire_bytes: server.wire_bytes,
        messages: client.messages.saturating_add(server.messages),
        control_frames: client.controls.saturating_add(server.controls),
        clean_close,
        effects: hooks.effects()?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PumpEnd {
    Close,
    Eof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PumpReport {
    end: PumpEnd,
    wire_bytes: u64,
    messages: u64,
    controls: u64,
}

#[allow(clippy::too_many_lines)]
async fn pump_direction<Reader, Writer>(
    mut reader: Reader,
    mut writer: Writer,
    direction: Direction,
    compression: Option<crate::PerMessageDeflate>,
    limits: RelayLimits,
    hooks: Arc<WebSocketHookChain>,
    cancellation: SessionCancellation,
) -> Result<PumpReport, RelayError>
where
    Reader: AsyncRead + Unpin,
    Writer: AsyncWrite + Unpin,
{
    let mut decoder = MessageDecoder::new(direction, limits.frames, compression)?;
    let mut deflater = compression
        .map(|negotiated| PerMessageDeflateCodec::new(negotiated, direction))
        .transpose()?;
    let mut buffer = vec![0_u8; limits.read_buffer_bytes];
    let mut report = PumpReport {
        end: PumpEnd::Eof,
        wire_bytes: 0,
        messages: 0,
        controls: 0,
    };
    loop {
        let read = tokio::select! {
            () = cancellation.cancelled() => return Err(RelayError::Cancelled),
            result = timeout(limits.idle_timeout, reader.read(&mut buffer)) => {
                result.map_err(|_| RelayError::IdleTimeout)??
            }
        };
        if read == 0 {
            writer.shutdown().await?;
            return Ok(report);
        }
        report.wire_bytes = report
            .wire_bytes
            .checked_add(u64::try_from(read).map_err(|_| RelayError::AccountingOverflow)?)
            .ok_or(RelayError::AccountingOverflow)?;
        for event in decoder.push(&buffer[..read])? {
            match event {
                MessageEvent::Message(message) => {
                    report.messages = report.messages.saturating_add(1);
                    match hooks.process_message(direction, message).await? {
                        MessageOutcome::Forward(message) => {
                            let (wire_payload, compressed) = if message.was_compressed {
                                (
                                    deflater
                                        .as_mut()
                                        .ok_or(RelayError::MissingCompressionContext)?
                                        .compress(&message.payload)?,
                                    true,
                                )
                            } else {
                                (message.payload.to_vec(), false)
                            };
                            let mut masks = os_mask;
                            for frame in encode_message(
                                message.kind,
                                &wire_payload,
                                compressed,
                                direction,
                                limits.outbound_frame_payload_bytes,
                                &mut masks,
                            )? {
                                write_all_bounded(&mut writer, &frame, limits, &cancellation)
                                    .await?;
                            }
                        }
                        MessageOutcome::Drop => {}
                        MessageOutcome::Close(close) => {
                            write_control(
                                &mut writer,
                                direction,
                                ControlFrame::Close(close),
                                limits,
                                &cancellation,
                            )
                            .await?;
                            report.end = PumpEnd::Close;
                            return Ok(report);
                        }
                    }
                }
                MessageEvent::Control(control) => {
                    report.controls = report.controls.saturating_add(1);
                    match hooks.process_control(direction, control).await? {
                        ControlOutcome::Forward(control) => {
                            let close = matches!(control, ControlFrame::Close(_));
                            write_control(&mut writer, direction, control, limits, &cancellation)
                                .await?;
                            if close {
                                report.end = PumpEnd::Close;
                                return Ok(report);
                            }
                        }
                        ControlOutcome::Drop => {}
                        ControlOutcome::Close(close) => {
                            write_control(
                                &mut writer,
                                direction,
                                ControlFrame::Close(close),
                                limits,
                                &cancellation,
                            )
                            .await?;
                            report.end = PumpEnd::Close;
                            return Ok(report);
                        }
                    }
                }
            }
        }
    }
}

async fn write_control<Writer: AsyncWrite + Unpin>(
    writer: &mut Writer,
    direction: Direction,
    control: ControlFrame,
    limits: RelayLimits,
    cancellation: &SessionCancellation,
) -> Result<(), RelayError> {
    let (opcode, payload) = match control {
        ControlFrame::Close(close) => (0x8, close.encode()?),
        ControlFrame::Ping(payload) => (0x9, payload),
        ControlFrame::Pong(payload) => (0xa, payload),
    };
    let mask = direction.requires_mask().then(os_mask).transpose()?;
    let encoded = encode_frame(
        &Frame {
            fin: true,
            compressed: false,
            opcode,
            payload,
        },
        direction,
        mask,
    )?;
    write_all_bounded(writer, &encoded, limits, cancellation).await
}

async fn write_all_bounded<Writer: AsyncWrite + Unpin>(
    writer: &mut Writer,
    bytes: &[u8],
    limits: RelayLimits,
    cancellation: &SessionCancellation,
) -> Result<(), RelayError> {
    tokio::select! {
        () = cancellation.cancelled() => Err(RelayError::Cancelled),
        result = timeout(limits.write_timeout, writer.write_all(bytes)) => {
            result.map_err(|_| RelayError::WriteTimeout)?.map_err(RelayError::from)
        }
    }
}

fn os_mask() -> Result<[u8; 4], EncodeError> {
    let mut mask = [0_u8; 4];
    getrandom::fill(&mut mask).map_err(|_| EncodeError::MaskGeneration)?;
    Ok(mask)
}

/// WebSocket relay failure.
#[derive(Debug, Error)]
pub enum RelayError {
    /// One or more transport limits were zero.
    #[error("invalid WebSocket relay limits")]
    InvalidLimits,
    /// Session cancellation was requested.
    #[error("WebSocket relay cancelled")]
    Cancelled,
    /// No transport bytes arrived before the idle deadline.
    #[error("WebSocket relay idle timeout")]
    IdleTimeout,
    /// Backpressured write exceeded its deadline.
    #[error("WebSocket relay write timeout")]
    WriteTimeout,
    /// One side closed its byte stream before a close frame.
    #[error("unclean WebSocket EOF from {0:?}")]
    UncleanEof(Direction),
    /// Peer did not complete the close handshake in time.
    #[error("WebSocket close handshake timed out")]
    CloseHandshakeTimeout,
    /// Compressed message had no negotiated state.
    #[error("WebSocket message is compressed without codec state")]
    MissingCompressionContext,
    /// Byte accounting overflowed.
    #[error("WebSocket byte accounting overflow")]
    AccountingOverflow,
    /// Transport I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Frame or message decoding failed.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// Frame encoding failed.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// Compression failed.
    #[error(transparent)]
    Compression(#[from] CompressionError),
    /// Hook execution failed.
    #[error(transparent)]
    Hook(#[from] WebSocketHookError),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::{CloseFrame, WebSocketHookFactory, WebSocketHookLimits, WebSocketSessionMetadata};

    #[tokio::test]
    async fn transparent_path_preserves_invalid_bytes_exactly() {
        let (mut client, proxy_downstream) = tokio::io::duplex(64);
        let (proxy_upstream, mut server) = tokio::io::duplex(64);
        let cancellation = SessionCancellation::new();
        let relay = tokio::spawn(relay_transparent(
            proxy_downstream,
            proxy_upstream,
            RelayLimits {
                idle_timeout: Duration::from_secs(1),
                ..RelayLimits::default()
            },
            cancellation,
        ));
        client.write_all(b"not websocket framing").await.unwrap();
        client.shutdown().await.unwrap();
        let mut received = Vec::new();
        server.read_to_end(&mut received).await.unwrap();
        server.shutdown().await.unwrap();
        assert_eq!(received, b"not websocket framing");
        let report = relay.await.unwrap().unwrap();
        assert_eq!(report.client_to_server_wire_bytes, 21);
    }

    #[tokio::test]
    async fn inspected_relay_handles_message_and_simultaneous_close() {
        let (mut client, proxy_downstream) = tokio::io::duplex(1024);
        let (proxy_upstream, mut server) = tokio::io::duplex(1024);
        let cancellation = SessionCancellation::new();
        let factory =
            WebSocketHookFactory::new(Vec::new(), WebSocketHookLimits::default()).unwrap();
        let chain = Arc::new(
            factory
                .create_session(
                    WebSocketSessionMetadata {
                        session_id: 1,
                        target: Arc::from("ws://example.test/"),
                        subprotocol: None,
                    },
                    cancellation.clone(),
                )
                .unwrap(),
        );
        let relay = tokio::spawn(relay_inspected(
            proxy_downstream,
            proxy_upstream,
            NegotiatedExtensions::default(),
            RelayLimits {
                close_handshake_timeout: Duration::from_secs(1),
                ..RelayLimits::default()
            },
            chain,
            cancellation,
        ));
        let message = encode_frame(
            &Frame {
                fin: true,
                compressed: false,
                opcode: 1,
                payload: Bytes::from_static(b"hello"),
            },
            Direction::ClientToServer,
            Some([1, 2, 3, 4]),
        )
        .unwrap();
        let client_close = encode_frame(
            &Frame {
                fin: true,
                compressed: false,
                opcode: 8,
                payload: CloseFrame {
                    code: Some(1000),
                    reason: String::new(),
                }
                .encode()
                .unwrap(),
            },
            Direction::ClientToServer,
            Some([5, 6, 7, 8]),
        )
        .unwrap();
        client.write_all(&message).await.unwrap();
        client.write_all(&client_close).await.unwrap();

        let mut received = vec![0; 11];
        server.read_exact(&mut received).await.unwrap();
        assert_eq!(&received[..2], &[0x81, 0x85]);
        let mut close = vec![0; 8];
        server.read_exact(&mut close).await.unwrap();
        assert_eq!(close[0], 0x88);
        let server_close = encode_frame(
            &Frame {
                fin: true,
                compressed: false,
                opcode: 8,
                payload: CloseFrame {
                    code: Some(1000),
                    reason: String::new(),
                }
                .encode()
                .unwrap(),
            },
            Direction::ServerToClient,
            None,
        )
        .unwrap();
        server.write_all(&server_close).await.unwrap();
        let mut client_reply = [0_u8; 4];
        client.read_exact(&mut client_reply).await.unwrap();
        assert_eq!(client_reply, [0x88, 0x02, 0x03, 0xe8]);
        let report = relay.await.unwrap().unwrap();
        assert!(report.clean_close);
        assert_eq!(report.messages, 1);
        assert_eq!(report.control_frames, 2);
    }

    #[tokio::test]
    async fn cancellation_terminates_an_idle_inspected_session() {
        let (_client, proxy_downstream) = tokio::io::duplex(64);
        let (proxy_upstream, _server) = tokio::io::duplex(64);
        let cancellation = SessionCancellation::new();
        let factory = WebSocketHookFactory::empty();
        let chain = Arc::new(
            factory
                .create_session(
                    WebSocketSessionMetadata {
                        session_id: 2,
                        target: Arc::from("ws://example.test/"),
                        subprotocol: None,
                    },
                    cancellation.clone(),
                )
                .unwrap(),
        );
        let task = tokio::spawn(relay_inspected(
            proxy_downstream,
            proxy_upstream,
            NegotiatedExtensions::default(),
            RelayLimits::default(),
            chain,
            cancellation.clone(),
        ));
        cancellation.cancel();
        assert!(matches!(task.await.unwrap(), Err(RelayError::Cancelled)));
    }

    #[tokio::test]
    async fn downstream_backpressure_hits_the_finite_write_deadline() {
        let (mut client, proxy_downstream) = tokio::io::duplex(4096);
        let (proxy_upstream, _unread_server) = tokio::io::duplex(8);
        let cancellation = SessionCancellation::new();
        let factory = WebSocketHookFactory::empty();
        let chain = Arc::new(
            factory
                .create_session(
                    WebSocketSessionMetadata {
                        session_id: 3,
                        target: Arc::from("ws://example.test/"),
                        subprotocol: None,
                    },
                    cancellation.clone(),
                )
                .unwrap(),
        );
        let task = tokio::spawn(relay_inspected(
            proxy_downstream,
            proxy_upstream,
            NegotiatedExtensions::default(),
            RelayLimits {
                write_timeout: Duration::from_millis(20),
                ..RelayLimits::default()
            },
            chain,
            cancellation,
        ));
        let frame = encode_frame(
            &Frame {
                fin: true,
                compressed: false,
                opcode: 2,
                payload: Bytes::from(vec![7; 1024]),
            },
            Direction::ClientToServer,
            Some([1, 2, 3, 4]),
        )
        .unwrap();
        client.write_all(&frame).await.unwrap();
        assert!(matches!(task.await.unwrap(), Err(RelayError::WriteTimeout)));
    }
}
