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
    /// Chromium `NetLog` JSON or typed JSONL.
    Netlog,
}

/// Inspect only the first 4 KiB, then fall back to a recognized extension.
/// JSON signatures require the first top-level key; no search through payloads.
pub fn detect_trace_format(path: &Path) -> Option<TraceFormat> {
    detect_trace_format_from(path, File::open(path).ok()?)
}

/// Route an already pinned source, rather than reopening a replaceable path.
pub(crate) fn detect_trace_format_from(path: &Path, reader: impl Read) -> Option<TraceFormat> {
    let mut start = Vec::with_capacity(4096);
    reader.take(4096).read_to_end(&mut start).ok()?;
    sniff(&start).or_else(
        || match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "tmcap" => Some(TraceFormat::Native),
            "saz" => Some(TraceFormat::Saz),
            "har" => Some(TraceFormat::Har),
            "netlog" | "json" | "jsonl" => Some(TraceFormat::Netlog),
            _ => None,
        },
    )
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
    if let Some(rest) = text.strip_prefix("\"type\"") {
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        if rest
            .strip_prefix("\"constants\"")
            .is_some_and(|rest| rest.trim_start().starts_with(','))
        {
            return Some(TraceFormat::Netlog);
        }
    }
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
    fn pinned_source_wins_over_replaced_path_and_short_reads() {
        struct ShortReads<'a>(&'a [u8]);
        impl Read for ShortReads<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let len = buffer.len().min(1);
                self.0.read(&mut buffer[..len])
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.har");
        std::fs::write(&path, b"{\"log\":{}}").unwrap();
        assert_eq!(detect_trace_format(&path), Some(TraceFormat::Har));
        assert_eq!(
            detect_trace_format_from(&path, ShortReads(b"TMCAP001\0")),
            Some(TraceFormat::Native)
        );
    }

    #[test]
    fn signatures_are_fixed_and_zip_is_ambiguous() {
        assert_eq!(sniff(b"TMCAP001\0"), Some(TraceFormat::Native));
        assert_eq!(sniff(b"TMCAP other data"), None);
        assert_eq!(
            sniff(b"\xef\xbb\xbf { \n\"log\" : {"),
            Some(TraceFormat::Har)
        );
        assert_eq!(sniff(b"{\"constants\": {"), Some(TraceFormat::Netlog));
        assert_eq!(
            sniff(b"{\"type\":\"constants\",\"constants\": {"),
            Some(TraceFormat::Netlog)
        );
        assert_eq!(sniff(b"{\"type\":\"event\",\"event\": {"), None);
        assert_eq!(sniff(b"PK\x03\x04"), None);
        assert_eq!(sniff(b"{\"payload\":\"log\"}"), None);
    }
}
