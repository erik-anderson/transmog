#![no_main]

use std::num::NonZeroUsize;

use libfuzzer_sys::fuzz_target;
use rustymiddle_content::ContentCodingStack;
use rustymiddle_core::{HeaderBlock, HeaderField};

fuzz_target!(|data: &[u8]| {
    if data.len() > 16 * 1024 {
        return;
    }
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    let data = data.strip_suffix(b"\r").unwrap_or(data);
    let max_layers = NonZeroUsize::new(4).expect("layer limit is nonzero");
    let fields = data
        .split(|byte| *byte == 0)
        .take(16)
        .filter_map(|value| HeaderField::try_new("content-encoding", value.to_vec()).ok())
        .collect();
    let headers = HeaderBlock::from_fields(fields);

    if let Ok(stack) = ContentCodingStack::from_headers(&headers, max_layers) {
        assert!(stack.len() <= max_layers.get());
        let canonical = stack.header_value();
        let canonical_headers = canonical.map_or_else(HeaderBlock::new, |value| {
            HeaderBlock::from_fields(vec![
                HeaderField::try_new("content-encoding", value).expect("canonical header"),
            ])
        });
        let reparsed = ContentCodingStack::from_headers(&canonical_headers, max_layers)
            .expect("canonical coding stack must reparse");
        assert_eq!(reparsed, stack);
    }
});
