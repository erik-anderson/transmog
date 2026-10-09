use std::collections::HashSet;

use thiserror::Error;

use crate::{HeaderBlock, HeaderField, HttpLegVersion};

const FIXED_HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "upgrade",
];

/// Whether a block belongs to a request or response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    /// Request headers.
    Request,
    /// Response headers.
    Response,
}

/// Body handling facts used while repairing framing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodySemantics {
    /// A body is permitted.
    Allowed,
    /// Semantics forbid a body regardless of framing headers.
    Forbidden,
}

/// Header translation controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranslationOptions {
    /// Destination HTTP version.
    pub destination: HttpLegVersion,
    /// Request or response block.
    pub kind: MessageKind,
    /// Whether a modifier changed body bytes.
    pub body_modified: bool,
    /// Whether to force identity encoding for deterministic modification.
    pub force_identity_encoding: bool,
}

/// Determines whether response semantics permit a body.
pub fn body_semantics(request_method: &str, response_status: u16) -> BodySemantics {
    if request_method.eq_ignore_ascii_case("HEAD")
        || (100..200).contains(&response_status)
        || response_status == 204
        || response_status == 304
    {
        BodySemantics::Forbidden
    } else {
        BodySemantics::Allowed
    }
}

/// Validates ambiguous framing and removes headers that cannot cross a proxy hop.
///
/// # Errors
///
/// Returns [`TranslationError`] for invalid or ambiguous framing and malformed
/// `Connection` nominations.
///
/// # Panics
///
/// Panics only if the compile-time constant headers cease to satisfy the `http`
/// crate's header grammar.
pub fn prepare_headers(
    source: &HeaderBlock,
    options: TranslationOptions,
) -> Result<HeaderBlock, TranslationError> {
    validate_framing(source)?;
    let nominated = connection_nominations(source)?;
    let mut output = HeaderBlock::new();

    for field in source.iter() {
        let lowercase = ascii_lower(field.name());
        if FIXED_HOP_BY_HOP
            .iter()
            .any(|name| lowercase == name.as_bytes())
            || nominated.contains(&lowercase)
            || field.name_eq("proxy-authorization")
            || field.name_eq("proxy-authenticate")
            || (options.kind == MessageKind::Response && field.name_eq("alt-svc"))
            || (matches!(
                options.destination,
                HttpLegVersion::Http2 | HttpLegVersion::Http3
            ) && field.name_eq("host"))
            || (options.body_modified
                && (field.name_eq("content-length")
                    || field.name_eq("content-md5")
                    || field.name_eq("digest")))
            || (options.force_identity_encoding && field.name_eq("content-encoding"))
        {
            continue;
        }
        output.push(field.clone());
    }

    if options.kind == MessageKind::Request && options.force_identity_encoding {
        output.replace_all(
            HeaderField::try_new("accept-encoding", "identity").expect("static header is valid"),
        );
    }
    // Transfer-coding preferences describe the incoming hop. Multiplexed
    // transports support trailers independently, including for streaming RPCs.
    if options.kind == MessageKind::Request
        && matches!(
            options.destination,
            HttpLegVersion::Http2 | HttpLegVersion::Http3
        )
        && source.values("te").any(|value| {
            value
                .split(|byte| *byte == b',')
                .any(|token| trim_ascii(token).eq_ignore_ascii_case(b"trailers"))
        })
    {
        output.push(HeaderField::try_new("te", "trailers").expect("static header is valid"));
    }
    Ok(output)
}

fn validate_framing(headers: &HeaderBlock) -> Result<(), TranslationError> {
    let lengths: Vec<_> = headers.values("content-length").map(trim_ascii).collect();
    if !lengths.is_empty() {
        let first = lengths[0];
        if lengths.iter().any(|value| *value != first) {
            return Err(TranslationError::ConflictingContentLength);
        }
        if first.is_empty() || !first.iter().all(u8::is_ascii_digit) {
            return Err(TranslationError::InvalidContentLength);
        }
        let text =
            std::str::from_utf8(first).map_err(|_| TranslationError::InvalidContentLength)?;
        text.parse::<u64>()
            .map_err(|_| TranslationError::InvalidContentLength)?;
    }
    let transfer_encodings: Vec<_> = headers.values("transfer-encoding").collect();
    if !lengths.is_empty() && !transfer_encodings.is_empty() {
        return Err(TranslationError::AmbiguousFraming);
    }
    if !transfer_encodings.is_empty() {
        if transfer_encodings.len() != 1 {
            return Err(TranslationError::InvalidTransferEncoding);
        }
        let value = std::str::from_utf8(transfer_encodings[0])
            .map_err(|_| TranslationError::InvalidTransferEncoding)?;
        let mut tokens = value.split(',').map(str::trim);
        if !tokens
            .next()
            .is_some_and(|token| token.eq_ignore_ascii_case("chunked"))
            || tokens.next().is_some()
        {
            return Err(TranslationError::InvalidTransferEncoding);
        }
    }
    Ok(())
}

fn connection_nominations(headers: &HeaderBlock) -> Result<HashSet<Vec<u8>>, TranslationError> {
    let mut result = HashSet::new();
    for value in headers.values("connection") {
        let text = std::str::from_utf8(value).map_err(|_| TranslationError::InvalidConnection)?;
        for token in text
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let name = http::HeaderName::from_bytes(token.as_bytes())
                .map_err(|_| TranslationError::InvalidConnection)?;
            result.insert(ascii_lower(name.as_str().as_bytes()));
        }
    }
    Ok(result)
}

fn ascii_lower(value: &[u8]) -> Vec<u8> {
    value.iter().map(u8::to_ascii_lowercase).collect()
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

/// Unsafe or invalid translation input.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TranslationError {
    /// Distinct Content-Length field values create a request-smuggling ambiguity.
    #[error("conflicting Content-Length values")]
    ConflictingContentLength,
    /// Content-Length was not an unsigned decimal integer.
    #[error("invalid Content-Length value")]
    InvalidContentLength,
    /// Content-Length and Transfer-Encoding appeared together.
    #[error("Content-Length and Transfer-Encoding cannot both be present")]
    AmbiguousFraming,
    /// Transfer-Encoding was duplicated, malformed, or unsupported.
    #[error("unsupported or ambiguous Transfer-Encoding")]
    InvalidTransferEncoding,
    /// Connection header did not contain a valid comma-separated token list.
    #[error("invalid Connection header")]
    InvalidConnection,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(fields: &[(&str, &str)]) -> HeaderBlock {
        HeaderBlock::from_fields(
            fields
                .iter()
                .map(|(name, value)| HeaderField::try_new(*name, *value).unwrap())
                .collect(),
        )
    }

    #[test]
    fn removes_hop_fields_nominated_fields_credentials_and_alt_svc() {
        let source = block(&[
            ("Connection", "Foo, keep-alive"),
            ("Foo", "secret-hop"),
            ("Keep-Alive", "timeout=5"),
            ("Proxy-Authorization", "Basic nope"),
            ("Alt-Svc", "h3=\":443\""),
            ("Set-Cookie", "a=1"),
            ("Set-Cookie", "b=2"),
        ]);
        let result = prepare_headers(
            &source,
            TranslationOptions {
                destination: HttpLegVersion::Http2,
                kind: MessageKind::Response,
                body_modified: false,
                force_identity_encoding: false,
            },
        )
        .unwrap();
        assert_eq!(result.fields().len(), 2);
        assert_eq!(result.values("set-cookie").count(), 2);
    }

    #[test]
    fn rejects_all_distinct_content_length_pairs() {
        for left in ["0", "1", "10", "100"] {
            for right in ["0", "1", "10", "100"] {
                let result = prepare_headers(
                    &block(&[("Content-Length", left), ("content-length", right)]),
                    TranslationOptions {
                        destination: HttpLegVersion::Http1,
                        kind: MessageKind::Request,
                        body_modified: false,
                        force_identity_encoding: false,
                    },
                );
                assert_eq!(result.is_ok(), left == right, "pair {left:?}, {right:?}");
            }
        }
    }

    #[test]
    fn rejects_non_decimal_content_lengths() {
        for value in ["+1", "", "1, 1", "-1", "0x10", "18446744073709551616"] {
            assert_eq!(
                prepare_headers(
                    &block(&[("Content-Length", value)]),
                    TranslationOptions {
                        destination: HttpLegVersion::Http1,
                        kind: MessageKind::Request,
                        body_modified: false,
                        force_identity_encoding: false,
                    },
                ),
                Err(TranslationError::InvalidContentLength),
                "value {value:?}",
            );
        }
    }

    #[test]
    fn transfer_encoding_preferences_do_not_cross_hops() {
        for destination in [
            HttpLegVersion::Http1,
            HttpLegVersion::Http2,
            HttpLegVersion::Http3,
        ] {
            for kind in [MessageKind::Request, MessageKind::Response] {
                let result = prepare_headers(
                    &block(&[("TE", "gzip"), ("X-End-To-End", "retained")]),
                    TranslationOptions {
                        destination,
                        kind,
                        body_modified: false,
                        force_identity_encoding: false,
                    },
                )
                .unwrap();
                assert!(
                    result.values("te").next().is_none(),
                    "{destination:?} {kind:?}"
                );
                assert_eq!(
                    result.values("x-end-to-end").next(),
                    Some(b"retained".as_slice())
                );
            }
        }
    }

    #[test]
    fn multiplexed_requests_advertise_only_supported_trailers() {
        for destination in [HttpLegVersion::Http2, HttpLegVersion::Http3] {
            for value in ["trailers", "TRAILERS", "gzip, trailers"] {
                let result = prepare_headers(
                    &block(&[("Connection", "TE"), ("TE", value)]),
                    TranslationOptions {
                        destination,
                        kind: MessageKind::Request,
                        body_modified: false,
                        force_identity_encoding: false,
                    },
                )
                .unwrap();
                assert_eq!(
                    result.values("te").collect::<Vec<_>>(),
                    vec![b"trailers".as_slice()]
                );
                assert!(result.values("connection").next().is_none());
            }
        }
    }

    #[test]
    fn body_edit_repairs_stale_metadata_and_forces_identity() {
        let result = prepare_headers(
            &block(&[
                ("Content-Length", "12"),
                ("Content-Encoding", "gzip"),
                ("Digest", "sha-256=bad"),
                ("Accept-Encoding", "br, gzip"),
            ]),
            TranslationOptions {
                destination: HttpLegVersion::Http3,
                kind: MessageKind::Request,
                body_modified: true,
                force_identity_encoding: true,
            },
        )
        .unwrap();
        assert!(result.values("content-length").next().is_none());
        assert!(result.values("content-encoding").next().is_none());
        assert_eq!(
            result.values("accept-encoding").next(),
            Some(b"identity".as_slice())
        );
    }

    #[test]
    fn rejects_unsupported_or_ambiguous_transfer_codings() {
        for fields in [
            vec![("Transfer-Encoding", "gzip")],
            vec![("Transfer-Encoding", "gzip, chunked")],
            vec![("Transfer-Encoding", "chunked, chunked")],
            vec![
                ("Transfer-Encoding", "chunked"),
                ("Transfer-Encoding", "chunked"),
            ],
        ] {
            assert_eq!(
                prepare_headers(
                    &block(&fields),
                    TranslationOptions {
                        destination: HttpLegVersion::Http1,
                        kind: MessageKind::Request,
                        body_modified: false,
                        force_identity_encoding: false,
                    }
                ),
                Err(TranslationError::InvalidTransferEncoding)
            );
        }
        assert!(
            prepare_headers(
                &block(&[("Transfer-Encoding", "chunked")]),
                TranslationOptions {
                    destination: HttpLegVersion::Http1,
                    kind: MessageKind::Request,
                    body_modified: false,
                    force_identity_encoding: false,
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn response_body_semantics_cover_head_and_bodyless_statuses() {
        assert_eq!(body_semantics("GET", 200), BodySemantics::Allowed);
        assert_eq!(body_semantics("HEAD", 200), BodySemantics::Forbidden);
        for status in [100, 101, 199, 204, 304] {
            assert_eq!(
                body_semantics("GET", status),
                BodySemantics::Forbidden,
                "status {status}"
            );
        }
    }
}
