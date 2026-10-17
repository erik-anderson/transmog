//! Experimental revision-0 case/rule interchange and protected portable archives.
mod case_model;
mod case_prepare;
mod case_validate;
mod convert;
mod model;
mod package;
mod package_zip;
mod prepare;
mod strict_json;
mod validate;
mod working;
pub use package::{
    Package, PackageManifest, PackageMember, ReadSeek, write_case_package, write_rules_package,
};

pub use case_model::*;
pub use case_prepare::PreparedCase;
pub use convert::{from_headers, from_native, to_native};
pub use model::*;
pub use prepare::PreparedCollection;
pub use working::WorkingCase;

/// A bounded interchange failure; callers must not publish partial imports.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Protected package needs an explicit source password.
    #[error("This package requires a password")]
    PasswordRequired,
    /// Source password did not authenticate the protected header.
    #[error("The package password is incorrect")]
    InvalidPassword,
    /// Invalid schema, identity, semantics or content integrity.
    #[error("{0}")]
    Invalid(String),
    /// A format, action or required capability this implementation cannot use.
    #[error("Unsupported portable content: {0}")]
    Unsupported(String),
    /// Explicit cancellation before publication.
    #[error("Portable operation canceled")]
    Canceled,
    /// A source or owned staging file could not be read or written.
    #[error("Portable object I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error.to_string())
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

/// Canonical unsigned decimal, optionally permitting zero; never a JSON float.
/// # Errors
/// Rejects signs, leading zeros, overflow and a forbidden zero.
pub fn decimal(value: &str, zero: bool) -> Result<u64, Error> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| invalid("Invalid decimal integer"))?;
    if parsed.to_string() != value || (!zero && parsed == 0) {
        return Err(invalid("Noncanonical decimal integer"));
    }
    Ok(parsed)
}

/// Whether a string is a portable entity identity.
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// SHA-256 of exact object bytes, in canonical lowercase hexadecimal.
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Whether a digest is exactly 64 lowercase hexadecimal digits.
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
