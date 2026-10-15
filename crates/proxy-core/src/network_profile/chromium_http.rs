//! Chromium 154.0.8037.99 HTTP receive policy.
//!
//! The windows come from `net/http/http_network_session.cc` and
//! `net/quic/quic_context.cc`; header limits come from
//! `net/http/http_network_session.h`. These are transport defaults, independent
//! of HTTP message body limits and the number of queued application requests.

/// Initial receive credit for each HTTP/2 or QUIC stream (6 MiB).
pub const STREAM_RECEIVE_WINDOW: u32 = 6 * 1024 * 1024;
/// Initial aggregate receive credit for HTTP/2 or QUIC (15 MiB).
pub const CONNECTION_RECEIVE_WINDOW: u32 = 15 * 1024 * 1024;
/// Advertised HTTP/2 HPACK decoder table capacity (64 KiB).
pub const HTTP2_HEADER_TABLE_SIZE: u32 = 64 * 1024;
/// Maximum uncompressed HTTP field section size (256 KiB).
pub const HEADER_LIST_SIZE: u32 = 256 * 1024;
