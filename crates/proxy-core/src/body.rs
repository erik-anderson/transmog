use std::{
    num::NonZeroUsize,
    task::{Context, Poll},
};

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::HeaderBlock;

/// A protocol-neutral HTTP body frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BodyFrame {
    /// Body bytes. Empty frames remain representable.
    Data(Bytes),
    /// Terminal trailer block.
    Trailers(HeaderBlock),
}

/// One bounded, protocol-neutral body stream.
///
/// The channel releases transport flow-control credit only as its receiver is
/// polled, so an idle consumer cannot accumulate more than the configured
/// number of frames between adapters.
pub struct BodyStream {
    receiver: mpsc::Receiver<Result<BodyFrame, BodyStreamError>>,
}

impl std::fmt::Debug for BodyStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BodyStream")
            .field("remaining_capacity", &self.receiver.capacity())
            .finish_non_exhaustive()
    }
}

impl BodyStream {
    /// Creates one bounded body channel with an explicit nonzero frame limit.
    pub fn channel(capacity: NonZeroUsize) -> (BodyStreamSender, Self) {
        let (sender, receiver) = mpsc::channel(capacity.get());
        (BodyStreamSender { sender }, Self { receiver })
    }

    /// Waits for the next body frame or terminal stream error.
    pub async fn recv(&mut self) -> Option<Result<BodyFrame, BodyStreamError>> {
        self.receiver.recv().await
    }

    /// Polls the next body frame for use by protocol-specific `Body` adapters.
    pub fn poll_recv(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame, BodyStreamError>>> {
        self.receiver.poll_recv(context)
    }

    /// Attempts to receive a frame without waiting.
    ///
    /// # Errors
    ///
    /// Returns Tokio's bounded-channel state when no frame is ready or all
    /// senders have closed.
    pub fn try_recv(
        &mut self,
    ) -> Result<Result<BodyFrame, BodyStreamError>, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Sending half of a bounded protocol-neutral body stream.
#[derive(Clone, Debug)]
pub struct BodyStreamSender {
    sender: mpsc::Sender<Result<BodyFrame, BodyStreamError>>,
}

impl BodyStreamSender {
    /// Sends one frame while applying bounded-channel backpressure.
    ///
    /// # Errors
    ///
    /// Returns [`BodyChannelClosed`] when the receiving transport was
    /// cancelled or otherwise dropped.
    pub async fn send(
        &self,
        frame: Result<BodyFrame, BodyStreamError>,
    ) -> Result<(), BodyChannelClosed> {
        self.sender.send(frame).await.map_err(|_| BodyChannelClosed)
    }

    /// Attempts to send without waiting for channel capacity.
    ///
    /// # Errors
    ///
    /// Returns Tokio's bounded-channel error with the original item when the
    /// channel is full or its receiver has closed.
    pub fn try_send(
        &self,
        frame: Result<BodyFrame, BodyStreamError>,
    ) -> Result<(), mpsc::error::TrySendError<Result<BodyFrame, BodyStreamError>>> {
        self.sender.try_send(frame)
    }

    /// Whether the receiving transport has been dropped.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

/// A body stream receiver was cancelled before accepting a frame.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("body stream receiver closed")]
pub struct BodyChannelClosed;

/// Error carried in-band on a protocol-neutral body stream.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyStreamError {
    /// A configured byte limit would be exceeded.
    #[error("body stream limit {limit} exceeded by attempted size {attempted}")]
    LimitExceeded {
        /// Configured maximum.
        limit: usize,
        /// Size after accepting the attempted frame.
        attempted: usize,
    },
    /// Data appeared after a terminal trailers frame.
    #[error("body stream data appeared after trailers")]
    DataAfterTrailers,
    /// More than one terminal trailers block appeared.
    #[error("body stream contained duplicate trailers")]
    DuplicateTrailers,
    /// No body frame arrived before the configured idle deadline.
    #[error("body stream exceeded its idle timeout")]
    IdleTimeout,
    /// A callback or protocol adapter terminated the stream.
    #[error("body stream failed: {0}")]
    Failed(String),
}

/// Explicitly bounded helper for modifiers that need a complete body.
#[derive(Debug)]
pub struct BoundedBodyBuffer {
    limit: usize,
    data: BytesMut,
    trailers: Option<HeaderBlock>,
}

impl BoundedBodyBuffer {
    /// Creates a buffer with an exact byte limit.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            data: BytesMut::new(),
            trailers: None,
        }
    }

    /// Appends one frame, rejecting duplicate trailers and data after trailers.
    ///
    /// # Errors
    ///
    /// Returns [`BodyLimitError`] when the byte limit would be exceeded or the
    /// frame ordering is invalid.
    pub fn push(&mut self, frame: BodyFrame) -> Result<(), BodyLimitError> {
        match frame {
            BodyFrame::Data(bytes) => {
                if self.trailers.is_some() {
                    return Err(BodyLimitError::DataAfterTrailers);
                }
                let attempted = self.data.len().checked_add(bytes.len()).ok_or(
                    BodyLimitError::LimitExceeded {
                        limit: self.limit,
                        attempted: usize::MAX,
                    },
                )?;
                if attempted > self.limit {
                    return Err(BodyLimitError::LimitExceeded {
                        limit: self.limit,
                        attempted,
                    });
                }
                self.data.extend_from_slice(&bytes);
            }
            BodyFrame::Trailers(trailers) => {
                if self.trailers.replace(trailers).is_some() {
                    return Err(BodyLimitError::DuplicateTrailers);
                }
            }
        }
        Ok(())
    }

    /// Current buffered data size.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether no body bytes have been buffered.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Consumes the buffer into data and optional trailers.
    pub fn finish(self) -> (Bytes, Option<HeaderBlock>) {
        (self.data.freeze(), self.trailers)
    }
}

/// Typed bounded-buffer policy outcome.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyLimitError {
    /// Appending a frame would exceed the configured bound.
    #[error("body buffer limit {limit} exceeded by attempted size {attempted}")]
    LimitExceeded {
        /// Configured maximum.
        limit: usize,
        /// Size after the attempted append.
        attempted: usize,
    },
    /// A second trailers frame was received.
    #[error("duplicate body trailers")]
    DuplicateTrailers,
    /// Data arrived after trailers.
    #[error("body data arrived after trailers")]
    DataAfterTrailers,
}

impl From<BodyLimitError> for BodyStreamError {
    fn from(error: BodyLimitError) -> Self {
        match error {
            BodyLimitError::LimitExceeded { limit, attempted } => {
                Self::LimitExceeded { limit, attempted }
            }
            BodyLimitError::DuplicateTrailers => Self::DuplicateTrailers,
            BodyLimitError::DataAfterTrailers => Self::DataAfterTrailers,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use bytes::Bytes;

    use super::{BodyFrame, BodyLimitError, BodyStream, BoundedBodyBuffer};

    #[test]
    fn limit_rejection_does_not_grow_the_buffer() {
        let mut buffer = BoundedBodyBuffer::new(4);
        buffer
            .push(BodyFrame::Data(Bytes::from_static(b"123")))
            .unwrap();
        assert_eq!(
            buffer.push(BodyFrame::Data(Bytes::from_static(b"45"))),
            Err(BodyLimitError::LimitExceeded {
                limit: 4,
                attempted: 5
            })
        );
        assert_eq!(buffer.len(), 3);
    }

    #[tokio::test]
    async fn body_channel_applies_frame_backpressure() {
        let (sender, mut stream) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"one"))))
            .await
            .unwrap();
        let second = tokio::spawn(async move {
            sender
                .send(Ok(BodyFrame::Data(Bytes::from_static(b"two"))))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"one"))
        );
        second.await.unwrap().unwrap();
        assert_eq!(
            stream.recv().await.unwrap().unwrap(),
            BodyFrame::Data(Bytes::from_static(b"two"))
        );
    }
}
