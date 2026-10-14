//! Checks which apply even when Cargo is invoked without the portable tools.

/// Source, headers and precompiled archives must come from the qualified provider.
pub fn replaces_native_library(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    let Some((_, suffix)) = name.split_once("BORING_BSSL_") else {
        return false;
    };
    let suffix = suffix.strip_prefix("FIPS_").unwrap_or(suffix);
    ["PATH", "INCLUDE_PATH", "SOURCE_PATH", "ASSUME_PATCHED"]
        .iter()
        .any(|option| suffix == *option || suffix.starts_with(&format!("{option}_")))
}
