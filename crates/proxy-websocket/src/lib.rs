#![deny(missing_docs)]

//! Protocol-neutral, bounded WebSocket handshake, framing, hooks, and relay.
//!
//! HTTP adapters own the upgrade itself. Once both byte streams exist, this
//! crate either relays them byte-for-byte or runs the explicitly configured
//! message hook chain. WebSocket frames never become HTTP body frames.

mod compression;
mod frame;
mod handshake;
mod hook;
mod relay;

pub use compression::{CompressionError, PerMessageDeflateCodec};
pub use frame::{
    CloseFrame, ControlFrame, DataKind, DecodeError, Direction, EncodeError, Frame, FrameDecoder,
    FrameLimits, Message, MessageDecoder, MessageEvent, encode_frame, encode_message,
};
pub use handshake::{
    ClientHandshake, HandshakeError, NegotiatedExtensions, PerMessageDeflate, RequestHandshake,
    ResponseHandshake, expected_accept, validate_request, validate_response,
};
pub use hook::{
    BoxWebSocketFuture, ControlAction, ControlEvent, ControlOutcome, HookActionKind, MessageAction,
    MessageEventHook, MessageOutcome, WebSocketEffect, WebSocketHookChain, WebSocketHookError,
    WebSocketHookFactory, WebSocketHookId, WebSocketHookIdentity, WebSocketHookLimits,
    WebSocketHookRegistration, WebSocketInterceptor, WebSocketInterceptorFactory,
    WebSocketSessionMetadata,
};
pub use relay::{
    RelayError, RelayLimits, RelayReport, SessionCancellation, relay_inspected, relay_transparent,
};
