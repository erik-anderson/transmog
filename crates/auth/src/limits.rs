//! Size and structure safeguards shared by native auth decoders.
//! Cancellation is checked independently; parsing does not consume work units.

const KIBIBYTE: usize = 1024;
const MEBIBYTE: usize = KIBIBYTE * KIBIBYTE;

/// Maximum candidate bytes before decoding or allocating from encoded lengths.
pub const MAX_CANDIDATE: usize = MEBIBYTE;
/// Maximum automatically read encoded body.
pub const MAX_BODY: usize = 2 * MEBIBYTE;
/// Maximum decoded HTTP or SAML material.
pub const MAX_DECODED_BYTES: usize = 4 * MEBIBYTE;
/// Aggregate input, decoded material and retained value allowance.
pub const MAX_TOTAL: usize = 8 * MEBIBYTE;
/// Maximum output fields across all nested findings.
pub const MAX_FIELDS: usize = 2048;
/// Maximum parser nesting.
pub const MAX_DEPTH: usize = 24;

pub(crate) const MAX_FINDINGS: usize = 64;
pub(crate) const MAX_DIAGNOSTICS: usize = 16;
pub(crate) const MAX_COVERAGE_SOURCES: usize = 16;
pub(crate) const MAX_PREVIEW_BYTES: usize = 8 * KIBIBYTE;
pub(crate) const MAX_FIELD_NAME_BYTES: usize = 256;
pub(crate) const MAX_COVERAGE_DETAIL_BYTES: usize = KIBIBYTE;
pub(crate) const MAX_SOURCE_LABEL_BYTES: usize = 128;
pub(crate) const MAX_PARAMETERS: usize = 2048;
pub(crate) const MAX_HTTP_ENVELOPES: usize = 64;
pub(crate) const MAX_HTTP_PARAMETERS: usize = 256;
pub(crate) const MAX_ELEMENT_ATTRIBUTES: usize = 128;
pub(crate) const MAX_HTML_CONTROLS: usize = 256;
pub(crate) const MAX_NTLM_AV_PAIRS: usize = 256;
pub(crate) const MAX_ASN1_CHILDREN: usize = 256;
pub(crate) const MAX_HEX_PREVIEW_BYTES: usize = 4 * KIBIBYTE;
