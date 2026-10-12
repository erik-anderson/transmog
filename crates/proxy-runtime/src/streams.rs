//! Adapters from protocol observations to the exchange's sequenced evidence.
use bytes::Bytes;
use std::{future::Future, pin::Pin, sync::Arc};
use transmog_core::{
    observe::{ExchangeObserver, ObserverEventKind},
    stream::{StreamMessage, StreamProtocol, StreamState, StreamStatus},
};
use transmog_websocket::{ControlFrame, DataKind, Direction, MessageEvent, RelayObserver};

pub(crate) struct WebSocketObserver(pub Arc<ExchangeObserver>);
impl RelayObserver for WebSocketObserver {
    fn observe(
        &self,
        direction: Direction,
        event: Result<MessageEvent, String>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let sender = if direction == Direction::ClientToServer {
                "client"
            } else {
                "server"
            };
            let message = match event {
                Ok(MessageEvent::Message(message)) => {
                    let mut observed = StreamMessage::new(
                        sender,
                        if message.kind == DataKind::Text {
                            "text"
                        } else {
                            "binary"
                        },
                        message.payload,
                    );
                    observed.compressed = message.was_compressed;
                    observed
                }
                Ok(MessageEvent::Control(ControlFrame::Ping(payload))) => {
                    StreamMessage::new(sender, "ping", payload)
                }
                Ok(MessageEvent::Control(ControlFrame::Pong(payload))) => {
                    StreamMessage::new(sender, "pong", payload)
                }
                Ok(MessageEvent::Control(ControlFrame::Close(close))) => {
                    self.0
                        .emit(ObserverEventKind::StreamState(StreamState::now(
                            StreamProtocol::WebSocket,
                            StreamStatus::Closing,
                            close.code.map(|code| format!("Close code {code}")),
                        )))
                        .await;
                    let mut message =
                        StreamMessage::new(sender, "close", Bytes::from(close.reason));
                    message.close_code = close.code;
                    message
                }
                Err(error) => StreamMessage::new(sender, "unavailable", Bytes::from(error)),
            };
            self.0.emit(ObserverEventKind::StreamMessage(message)).await;
        })
    }
}

pub(crate) struct SseObservation(pub tokio::sync::Mutex<Option<transmog_content::sse::SseDecoder>>);
