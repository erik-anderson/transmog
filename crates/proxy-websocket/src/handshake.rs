use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha1::{Digest, Sha1};
use thiserror::Error;

const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Borrowed fields required to validate an HTTP/1.1 WebSocket request.
#[derive(Clone, Copy, Debug)]
pub struct RequestHandshake<'a> {
    /// HTTP method.
    pub method: &'a str,
    /// Combined `Connection` header field value.
    pub connection: &'a str,
    /// Combined `Upgrade` header field value.
    pub upgrade: &'a str,
    /// `Sec-WebSocket-Version` field value.
    pub version: &'a str,
    /// `Sec-WebSocket-Key` field value.
    pub key: &'a str,
    /// Combined extension offers, if present.
    pub extensions: Option<&'a str>,
    /// Combined requested subprotocols, if present.
    pub subprotocols: Option<&'a str>,
}

/// Validated, owned client handshake used to authenticate the response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientHandshake {
    key: String,
    compression_offered: bool,
    subprotocols: Vec<String>,
}

impl ClientHandshake {
    /// Original, trimmed nonce used to verify `Sec-WebSocket-Accept`.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether the client offered `permessage-deflate`.
    pub fn compression_offered(&self) -> bool {
        self.compression_offered
    }

    /// Requested subprotocol tokens in preference order.
    pub fn subprotocols(&self) -> &[String] {
        &self.subprotocols
    }
}

/// Borrowed fields required to validate an HTTP/1.1 WebSocket response.
#[derive(Clone, Copy, Debug)]
pub struct ResponseHandshake<'a> {
    /// HTTP status.
    pub status: u16,
    /// Combined `Connection` header field value.
    pub connection: &'a str,
    /// Combined `Upgrade` header field value.
    pub upgrade: &'a str,
    /// `Sec-WebSocket-Accept` field value.
    pub accept: &'a str,
    /// Selected extensions, if present.
    pub extensions: Option<&'a str>,
    /// Selected subprotocol, if present.
    pub subprotocol: Option<&'a str>,
}

/// Negotiated `permessage-deflate` policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PerMessageDeflate {
    /// Client messages reset compression state after every message.
    pub client_no_context_takeover: bool,
    /// Server messages reset compression state after every message.
    pub server_no_context_takeover: bool,
    /// Client compression window selected by the server.
    pub client_max_window_bits: Option<u8>,
    /// Server compression window selected by the server.
    pub server_max_window_bits: Option<u8>,
}

impl PerMessageDeflate {
    /// Whether the sender for this direction must reset after each message.
    pub fn no_context_takeover(self, direction: crate::Direction) -> bool {
        match direction {
            crate::Direction::ClientToServer => self.client_no_context_takeover,
            crate::Direction::ServerToClient => self.server_no_context_takeover,
        }
    }

    /// Negotiated sender window for this direction.
    pub fn window_bits(self, direction: crate::Direction) -> Option<u8> {
        match direction {
            crate::Direction::ClientToServer => self.client_max_window_bits,
            crate::Direction::ServerToClient => self.server_max_window_bits,
        }
    }
}

/// Extensions and subprotocol accepted by both peers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NegotiatedExtensions {
    /// Negotiated message compression, when selected.
    pub permessage_deflate: Option<PerMessageDeflate>,
    /// Server-selected subprotocol.
    pub subprotocol: Option<String>,
}

/// Validates a client request before forwarding an upgrade.
///
/// # Errors
///
/// Rejects an invalid method, missing upgrade tokens, unsupported version,
/// malformed nonce, extension offer, or subprotocol list.
pub fn validate_request(input: RequestHandshake<'_>) -> Result<ClientHandshake, HandshakeError> {
    if !input.method.eq_ignore_ascii_case("GET") {
        return Err(HandshakeError::InvalidMethod);
    }
    if !has_token(input.connection, "upgrade") || !has_token(input.upgrade, "websocket") {
        return Err(HandshakeError::MissingUpgradeToken);
    }
    if input.version.trim() != "13" {
        return Err(HandshakeError::UnsupportedVersion);
    }
    let key = input.key.trim();
    let decoded = STANDARD
        .decode(key)
        .map_err(|_| HandshakeError::InvalidKey)?;
    if decoded.len() != 16 {
        return Err(HandshakeError::InvalidKey);
    }
    let compression_offered = parse_extension_offer(input.extensions)?;
    let subprotocols = parse_tokens(input.subprotocols, false)?;
    Ok(ClientHandshake {
        key: key.to_owned(),
        compression_offered,
        subprotocols,
    })
}

/// Validates a server response against the authenticated client request.
///
/// # Errors
///
/// Rejects a non-switching response, bad accept proof, unsolicited or invalid
/// extension parameters, and an unoffered subprotocol.
pub fn validate_response(
    request: &ClientHandshake,
    input: ResponseHandshake<'_>,
) -> Result<NegotiatedExtensions, HandshakeError> {
    if input.status != 101 {
        return Err(HandshakeError::InvalidStatus(input.status));
    }
    if !has_token(input.connection, "upgrade") || !has_token(input.upgrade, "websocket") {
        return Err(HandshakeError::MissingUpgradeToken);
    }
    if input.accept.trim() != expected_accept(&request.key) {
        return Err(HandshakeError::InvalidAccept);
    }
    let permessage_deflate = parse_extension_response(input.extensions)?;
    if permessage_deflate.is_some() && !request.compression_offered {
        return Err(HandshakeError::UnsolicitedExtension);
    }
    let subprotocol = input
        .subprotocol
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if subprotocol
        .as_ref()
        .is_some_and(|selected| !request.subprotocols.iter().any(|item| item == selected))
    {
        return Err(HandshakeError::UnofferedSubprotocol);
    }
    Ok(NegotiatedExtensions {
        permessage_deflate,
        subprotocol,
    })
}

/// Computes the RFC 6455 server accept proof for a client nonce.
pub fn expected_accept(key: &str) -> String {
    let mut hash = Sha1::new();
    hash.update(key.trim().as_bytes());
    hash.update(WEBSOCKET_GUID);
    STANDARD.encode(hash.finalize())
}

fn has_token(value: &str, expected: &str) -> bool {
    value
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case(expected))
}

fn parse_tokens(value: Option<&str>, allow_empty: bool) -> Result<Vec<String>, HandshakeError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let tokens = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if (!allow_empty && tokens.iter().any(String::is_empty))
        || tokens.iter().any(|token| !is_token(token))
    {
        return Err(HandshakeError::InvalidSubprotocol);
    }
    Ok(tokens)
}

fn parse_extension_offer(value: Option<&str>) -> Result<bool, HandshakeError> {
    let Some(value) = value else {
        return Ok(false);
    };
    let mut offered = false;
    for extension in value.split(',') {
        let mut pieces = extension.split(';');
        let name = pieces.next().unwrap_or_default().trim();
        if !is_token(name) {
            return Err(HandshakeError::InvalidExtension);
        }
        if name.eq_ignore_ascii_case("permessage-deflate") {
            if offered {
                return Err(HandshakeError::DuplicateExtension);
            }
            offered = true;
            parse_parameters(pieces, true)?;
        }
    }
    Ok(offered)
}

fn parse_extension_response(
    value: Option<&str>,
) -> Result<Option<PerMessageDeflate>, HandshakeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut selected = None;
    for extension in value.split(',') {
        let mut pieces = extension.split(';');
        let name = pieces.next().unwrap_or_default().trim();
        if !name.eq_ignore_ascii_case("permessage-deflate") {
            return Err(HandshakeError::UnsupportedExtension(name.to_owned()));
        }
        if selected.is_some() {
            return Err(HandshakeError::DuplicateExtension);
        }
        selected = Some(parse_parameters(pieces, false)?);
    }
    Ok(selected)
}

fn parse_parameters<'a>(
    pieces: impl Iterator<Item = &'a str>,
    offer: bool,
) -> Result<PerMessageDeflate, HandshakeError> {
    let mut result = PerMessageDeflate::default();
    let mut seen = std::collections::BTreeSet::new();
    for raw in pieces {
        let (name, value) = raw
            .trim()
            .split_once('=')
            .map_or((raw.trim(), None), |(name, value)| {
                (name.trim(), Some(value.trim().trim_matches('"')))
            });
        let lower = name.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(HandshakeError::DuplicateExtensionParameter(lower));
        }
        match lower.as_str() {
            "client_no_context_takeover" if value.is_none() => {
                result.client_no_context_takeover = true;
            }
            "server_no_context_takeover" if value.is_none() => {
                result.server_no_context_takeover = true;
            }
            "client_max_window_bits" => {
                result.client_max_window_bits = parse_window(value, offer)?;
            }
            "server_max_window_bits" => {
                result.server_max_window_bits = parse_window(value, false)?;
            }
            _ => return Err(HandshakeError::InvalidExtensionParameter(lower)),
        }
    }
    Ok(result)
}

fn parse_window(value: Option<&str>, may_be_omitted: bool) -> Result<Option<u8>, HandshakeError> {
    let Some(value) = value else {
        return may_be_omitted
            .then_some(None)
            .ok_or(HandshakeError::InvalidWindowBits);
    };
    let bits = value
        .parse::<u8>()
        .map_err(|_| HandshakeError::InvalidWindowBits)?;
    if !(8..=15).contains(&bits) {
        return Err(HandshakeError::InvalidWindowBits);
    }
    Ok(Some(bits))
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// WebSocket opening-handshake failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HandshakeError {
    /// Upgrade requests must use GET.
    #[error("WebSocket upgrade request must use GET")]
    InvalidMethod,
    /// `Connection: upgrade` or `Upgrade: websocket` was absent.
    #[error("WebSocket upgrade tokens are missing")]
    MissingUpgradeToken,
    /// Only RFC 6455 version 13 is supported.
    #[error("unsupported WebSocket version")]
    UnsupportedVersion,
    /// Client nonce was not canonical base64 for 16 bytes.
    #[error("invalid Sec-WebSocket-Key")]
    InvalidKey,
    /// Server did not return Switching Protocols.
    #[error("WebSocket server returned status {0}, not 101")]
    InvalidStatus(u16),
    /// Server proof did not authenticate the client nonce.
    #[error("invalid Sec-WebSocket-Accept")]
    InvalidAccept,
    /// Extension syntax was malformed.
    #[error("invalid WebSocket extension syntax")]
    InvalidExtension,
    /// An extension appeared more than once.
    #[error("duplicate WebSocket extension")]
    DuplicateExtension,
    /// An extension parameter appeared more than once.
    #[error("duplicate WebSocket extension parameter: {0}")]
    DuplicateExtensionParameter(String),
    /// An extension parameter is unsupported or malformed.
    #[error("invalid WebSocket extension parameter: {0}")]
    InvalidExtensionParameter(String),
    /// A negotiated window was outside 8 through 15.
    #[error("invalid permessage-deflate window bits")]
    InvalidWindowBits,
    /// Server selected an extension the client did not offer.
    #[error("server selected an unoffered WebSocket extension")]
    UnsolicitedExtension,
    /// Server selected an unsupported extension.
    #[error("unsupported WebSocket extension selected: {0}")]
    UnsupportedExtension(String),
    /// Subprotocol syntax was invalid.
    #[error("invalid WebSocket subprotocol list")]
    InvalidSubprotocol,
    /// Server selected a subprotocol the client did not offer.
    #[error("server selected an unoffered WebSocket subprotocol")]
    UnofferedSubprotocol,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_handshake_vector_and_negotiation_validate() {
        let request = validate_request(RequestHandshake {
            method: "GET",
            connection: "keep-alive, Upgrade",
            upgrade: "websocket",
            version: "13",
            key: "dGhlIHNhbXBsZSBub25jZQ==",
            extensions: Some(
                "permessage-deflate; client_no_context_takeover; server_max_window_bits=15",
            ),
            subprotocols: Some("chat, superchat"),
        })
        .unwrap();
        assert_eq!(
            expected_accept(request.key()),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        let negotiated = validate_response(
            &request,
            ResponseHandshake {
                status: 101,
                connection: "Upgrade",
                upgrade: "WebSocket",
                accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
                extensions: Some("permessage-deflate; client_no_context_takeover"),
                subprotocol: Some("chat"),
            },
        )
        .unwrap();
        assert!(
            negotiated
                .permessage_deflate
                .unwrap()
                .client_no_context_takeover
        );
        assert_eq!(negotiated.subprotocol.as_deref(), Some("chat"));
    }

    #[test]
    fn unsolicited_and_malformed_negotiation_fail_closed() {
        let request = validate_request(RequestHandshake {
            method: "GET",
            connection: "upgrade",
            upgrade: "websocket",
            version: "13",
            key: "dGhlIHNhbXBsZSBub25jZQ==",
            extensions: None,
            subprotocols: None,
        })
        .unwrap();
        assert_eq!(
            validate_response(
                &request,
                ResponseHandshake {
                    status: 101,
                    connection: "upgrade",
                    upgrade: "websocket",
                    accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
                    extensions: Some("permessage-deflate"),
                    subprotocol: None,
                }
            ),
            Err(HandshakeError::UnsolicitedExtension)
        );
        assert_eq!(
            parse_extension_response(Some("permessage-deflate; server_max_window_bits=7")),
            Err(HandshakeError::InvalidWindowBits)
        );
    }
}
