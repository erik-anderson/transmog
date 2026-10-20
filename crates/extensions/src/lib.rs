//! Read-only extension metadata, native admission and host-owned presentation contracts.
mod columns;
mod document;
mod manifest;
mod matching;
mod package;
mod protocol;

pub use columns::*;
pub use document::*;
pub use manifest::*;
pub use matching::*;
pub use package::*;
pub use protocol::*;

/// A bounded extension failure, without captured values or source snippets.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid manifest, input, output or archive structure.
    #[error("{0}")]
    Invalid(String),
    /// Data outside the contribution's declared grant.
    #[error("The visualizer did not declare this data access")]
    AccessDenied,
    /// Unsupported contract version.
    #[error("This extension requires an unsupported format, API or JavaScript runtime")]
    Unsupported,
    /// A finite operation budget was exhausted.
    #[error("The extension operation exceeded its resource limit")]
    Limit,
    /// A host-owned package or developer source could not be read.
    #[error("Extension file operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid metadata JSON.
    #[error("Extension metadata is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// Bounded ZIP validation failure.
    #[error(transparent)]
    Zip(#[from] transmog_package_zip::Error),
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.to_owned())
}

/// SHA-256 of exact bytes, encoded as lowercase hexadecimal.
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Whether an identifier is portable and bounded.
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && transmog_package_zip::name(value.as_bytes()).is_ok()
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// Parses a canonical unsigned decimal without JavaScript precision loss.
/// # Errors
/// Rejects signs, leading zeros and overflow.
pub fn decimal(value: &str) -> Result<u64, Error> {
    let result = value
        .parse::<u64>()
        .map_err(|_| invalid("Invalid byte offset or length"))?;
    if result.to_string() != value {
        return Err(invalid("Noncanonical byte offset or length"));
    }
    Ok(result)
}

/// Versioned TypeScript authoring declarations, packaged without loading a runtime.
pub fn typescript_declarations() -> &'static str {
    include_str!("../sdk/transmog-extensions.d.ts")
}
