use std::num::NonZeroUsize;

use rustymiddle_core::HeaderBlock;
use thiserror::Error;

/// A supported HTTP content-coding token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContentCoding {
    /// RFC 1952 gzip framing.
    Gzip,
    /// RFC 1950 zlib-wrapped DEFLATE.
    Deflate,
    /// Brotli content coding.
    Brotli,
    /// Zstandard content coding.
    Zstd,
}

impl ContentCoding {
    /// Canonical lowercase HTTP token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
            Self::Zstd => "zstd",
        }
    }

    fn parse(value: &str) -> Result<Option<Self>, ContentCodingError> {
        if value.eq_ignore_ascii_case("identity") {
            return Ok(None);
        }
        if value.eq_ignore_ascii_case("gzip") {
            Ok(Some(Self::Gzip))
        } else if value.eq_ignore_ascii_case("deflate") {
            Ok(Some(Self::Deflate))
        } else if value.eq_ignore_ascii_case("br") {
            Ok(Some(Self::Brotli))
        } else if value.eq_ignore_ascii_case("zstd") {
            Ok(Some(Self::Zstd))
        } else {
            Err(ContentCodingError::Unsupported(value.to_ascii_lowercase()))
        }
    }
}

/// Ordered content codings as applied by the sender.
///
/// Encoding uses iteration order. Decoding uses reverse iteration order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContentCodingStack(Vec<ContentCoding>);

impl ContentCodingStack {
    /// Parses all `Content-Encoding` fields in wire order.
    ///
    /// An absent header and a single `identity` token both produce an empty
    /// stack. `identity` mixed with any other element is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`ContentCodingError`] for invalid syntax, unsupported codings,
    /// ambiguous identity use, or a stack deeper than `max_layers`.
    pub fn from_headers(
        headers: &HeaderBlock,
        max_layers: NonZeroUsize,
    ) -> Result<Self, ContentCodingError> {
        let mut codings = Vec::new();
        let mut element_count = 0_usize;
        let mut identity_count = 0_usize;

        for value in headers.values("content-encoding") {
            let value =
                std::str::from_utf8(value).map_err(|_| ContentCodingError::InvalidHeaderValue)?;
            for element in value.split(',') {
                let element = trim_ows(element);
                if element.is_empty() {
                    return Err(ContentCodingError::EmptyElement);
                }
                if !element.bytes().all(is_tchar) {
                    return Err(ContentCodingError::InvalidToken);
                }
                element_count = element_count.saturating_add(1);
                match ContentCoding::parse(element)? {
                    Some(coding) => {
                        if codings.len() == max_layers.get() {
                            return Err(ContentCodingError::TooManyLayers {
                                limit: max_layers.get(),
                                attempted: codings.len().saturating_add(1),
                            });
                        }
                        codings.push(coding);
                    }
                    None => identity_count = identity_count.saturating_add(1),
                }
            }
        }

        if identity_count > 0 && element_count != 1 {
            return Err(ContentCodingError::IdentityCombined);
        }
        if codings.len() > max_layers.get() {
            return Err(ContentCodingError::TooManyLayers {
                limit: max_layers.get(),
                attempted: codings.len(),
            });
        }
        Ok(Self(codings))
    }

    /// Number of non-identity coding layers.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the representation uses identity coding.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Codings in the order an encoder applies them.
    pub fn encode_order(&self) -> impl Iterator<Item = ContentCoding> + '_ {
        self.0.iter().copied()
    }

    /// Codings in the reverse order a decoder applies them.
    pub fn decode_order(&self) -> impl Iterator<Item = ContentCoding> + '_ {
        self.0.iter().rev().copied()
    }

    /// Canonical comma-separated header value, or `None` for identity.
    pub fn header_value(&self) -> Option<String> {
        (!self.is_empty()).then(|| {
            self.0
                .iter()
                .map(|coding| coding.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
    }
}

/// Failure parsing or validating an HTTP content-coding stack.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContentCodingError {
    /// A field value was not valid ASCII/UTF-8 text.
    #[error("Content-Encoding contains a non-text field value")]
    InvalidHeaderValue,
    /// A comma-separated field contained an empty element.
    #[error("Content-Encoding contains an empty element")]
    EmptyElement,
    /// An element was not an HTTP token.
    #[error("Content-Encoding contains an invalid token")]
    InvalidToken,
    /// A syntactically valid coding is not supported by this layer.
    #[error("unsupported content coding: {0}")]
    Unsupported(String),
    /// `identity` appeared with another field element.
    #[error("identity content coding cannot be combined with another element")]
    IdentityCombined,
    /// The coding stack exceeded its configured depth.
    #[error("content coding stack has {attempted} layers; limit is {limit}")]
    TooManyLayers {
        /// Configured maximum coding depth.
        limit: usize,
        /// Parsed coding depth.
        attempted: usize,
    },
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn is_tchar(value: u8) -> bool {
    value.is_ascii_alphanumeric()
        || matches!(
            value,
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
}

#[cfg(test)]
mod tests {
    use rustymiddle_core::HeaderField;

    use super::*;

    fn headers(values: &[&str]) -> HeaderBlock {
        HeaderBlock::from_fields(
            values
                .iter()
                .map(|value| HeaderField::try_new("Content-Encoding", *value).unwrap())
                .collect(),
        )
    }

    #[test]
    fn combines_fields_and_reverses_decode_order() {
        let stack = ContentCodingStack::from_headers(
            &headers(&["gzip, br", "deflate, zstd"]),
            NonZeroUsize::new(4).unwrap(),
        )
        .unwrap();
        assert_eq!(
            stack.encode_order().collect::<Vec<_>>(),
            [
                ContentCoding::Gzip,
                ContentCoding::Brotli,
                ContentCoding::Deflate,
                ContentCoding::Zstd
            ]
        );
        assert_eq!(
            stack.decode_order().collect::<Vec<_>>(),
            [
                ContentCoding::Zstd,
                ContentCoding::Deflate,
                ContentCoding::Brotli,
                ContentCoding::Gzip
            ]
        );
        assert_eq!(
            stack.header_value().as_deref(),
            Some("gzip, br, deflate, zstd")
        );
    }

    #[test]
    fn absent_and_single_identity_are_identity() {
        let limit = NonZeroUsize::new(1).unwrap();
        assert!(
            ContentCodingStack::from_headers(&HeaderBlock::new(), limit)
                .unwrap()
                .is_empty()
        );
        assert!(
            ContentCodingStack::from_headers(&headers(&[" identity\t"]), limit)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_ambiguous_invalid_unsupported_and_too_deep_stacks() {
        let four = NonZeroUsize::new(4).unwrap();
        assert_eq!(
            ContentCodingStack::from_headers(&headers(&["identity, gzip"]), four),
            Err(ContentCodingError::IdentityCombined)
        );
        let non_text = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-encoding", vec![0x80]).unwrap(),
        ]);
        assert_eq!(
            ContentCodingStack::from_headers(&non_text, four),
            Err(ContentCodingError::InvalidHeaderValue)
        );
        assert_eq!(
            ContentCodingStack::from_headers(&headers(&["gzip,,br"]), four),
            Err(ContentCodingError::EmptyElement)
        );
        assert_eq!(
            ContentCodingStack::from_headers(&headers(&["gzip;level=1"]), four),
            Err(ContentCodingError::InvalidToken)
        );
        assert_eq!(
            ContentCodingStack::from_headers(&headers(&["compress"]), four),
            Err(ContentCodingError::Unsupported("compress".to_owned()))
        );
        assert_eq!(
            ContentCodingStack::from_headers(
                &headers(&["gzip, br"]),
                NonZeroUsize::new(1).unwrap()
            ),
            Err(ContentCodingError::TooManyLayers {
                limit: 1,
                attempted: 2
            })
        );
    }
}
