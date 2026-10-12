//! Passive decoding, independent of the rewriting hook chain.
use crate::{Direction, FrameLimits, MessageDecoder, MessageEvent, NegotiatedExtensions};
use std::{future::Future, pin::Pin, sync::Arc};

/// Observation callbacks never decide how bytes are forwarded.
pub trait RelayObserver: Send + Sync {
    /// Receives a complete message/control frame or an explanation of unavailable decoding.
    fn observe(
        &self,
        direction: Direction,
        event: Result<MessageEvent, String>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Per-direction, bounded passive decoder. Failure disables inspection, not forwarding.
pub(crate) struct PassiveDecoder {
    decoder: Option<MessageDecoder>,
    error: Option<String>,
    direction: Direction,
    observer: Arc<dyn RelayObserver>,
}

impl PassiveDecoder {
    pub(crate) fn new(
        direction: Direction,
        limits: FrameLimits,
        negotiated: &NegotiatedExtensions,
        observer: Arc<dyn RelayObserver>,
    ) -> Self {
        let result = MessageDecoder::new(direction, limits, negotiated.permessage_deflate);
        let (decoder, error) = match result {
            Ok(decoder) => (Some(decoder), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            decoder,
            error,
            direction,
            observer,
        }
    }

    pub(crate) async fn push(&mut self, bytes: &[u8]) -> bool {
        let mut closed = false;
        if let Some(error) = self.error.take() {
            self.observer.observe(self.direction, Err(error)).await;
        }
        let Some(decoder) = &mut self.decoder else {
            return false;
        };
        match decoder.push(bytes) {
            Ok(events) => {
                for event in events {
                    closed |=
                        matches!(&event, MessageEvent::Control(crate::ControlFrame::Close(_)));
                    self.observer.observe(self.direction, Ok(event)).await;
                }
            }
            Err(error) => {
                self.decoder = None;
                self.observer
                    .observe(self.direction, Err(error.to_string()))
                    .await;
            }
        }
        closed
    }
}
