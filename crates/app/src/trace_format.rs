//! Conservative file routing shared by desktop launch, drop and import.
use std::{fs::File, io::Read, path::Path};

/// Supported traffic input. `NetLog` is deliberately import-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceFormat {
    /// Transmog native capture.
    Native,
    /// Fiddler archive; ZIP alone is not a reliable SAZ signature.
    Saz,
    /// HTTP Archive JSON.
    Har,
    /// Chromium `NetLog` JSON.
    Netlog,
}

/// Inspect only the first 4 KiB, then fall back to a recognized extension.
/// JSON signatures require the first top-level key; no search through payloads.
pub fn detect_trace_format(path: &Path) -> Option<TraceFormat> {
    let mut start = [0; 4096];
    let count = File::open(path).ok()?.read(&mut start).ok()?;
    sniff(&start[..count]).or_else(|| {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "tmcap" => Some(TraceFormat::Native),
            "saz" => Some(TraceFormat::Saz),
            "har" => Some(TraceFormat::Har),
            "netlog" | "json" => Some(TraceFormat::Netlog),
            _ => None,
        }
    })
}

fn sniff(start: &[u8]) -> Option<TraceFormat> {
    if start.starts_with(b"TMCAP001\0") {
        return Some(TraceFormat::Native);
    }
    let start = start.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(start);
    let text = match std::str::from_utf8(start) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&start[..error.valid_up_to()]).ok()?,
    };
    let mut text = text.trim_start();
    text = text.strip_prefix('{')?.trim_start();
    for (key, format) in [
        ("\"log\"", TraceFormat::Har),
        ("\"constants\"", TraceFormat::Netlog),
    ] {
        if let Some(rest) = text.strip_prefix(key) {
            let rest = rest.trim_start().strip_prefix(':')?.trim_start();
            if rest.starts_with('{') {
                return Some(format);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_are_fixed_and_zip_is_ambiguous() {
        assert_eq!(sniff(b"TMCAP001\0"), Some(TraceFormat::Native));
        assert_eq!(sniff(b"TMCAP other data"), None);
        assert_eq!(
            sniff(b"\xef\xbb\xbf { \n\"log\" : {"),
            Some(TraceFormat::Har)
        );
        assert_eq!(sniff(b"{\"constants\": {"), Some(TraceFormat::Netlog));
        assert_eq!(sniff(b"PK\x03\x04"), None);
        assert_eq!(sniff(b"{\"payload\":\"log\"}"), None);
    }
}
