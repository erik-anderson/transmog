#![no_main]

use std::num::NonZeroUsize;

use libfuzzer_sys::fuzz_target;
use rustymiddle_content::ContentCodingStack;
use rustymiddle_core::{HeaderBlock, HeaderField};

const MAX_INPUT: usize = 16 * 1024;
const STRUCTURED_PREFIX: &[u8] = b"structured:";

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    let data = data.strip_suffix(b"\r").unwrap_or(data);
    let synthesized;
    let (data, max_layers) = if let Some(control) = data.strip_prefix(STRUCTURED_PREFIX) {
        synthesized = synthesize_fields(control);
        (
            synthesized.0.as_slice(),
            NonZeroUsize::new(synthesized.1).expect("layer limit is nonzero"),
        )
    } else {
        (data, NonZeroUsize::new(4).expect("layer limit is nonzero"))
    };
    let fields = data
        .split(|byte| *byte == 0)
        .take(16)
        .filter_map(|value| HeaderField::try_new("content-encoding", value.to_vec()).ok())
        .collect();
    let headers = HeaderBlock::from_fields(fields);

    let parsed = ContentCodingStack::from_headers(&headers, max_layers);
    assert_eq!(
        parsed,
        ContentCodingStack::from_headers(&headers, max_layers),
        "content-coding parsing must be deterministic"
    );
    if let Ok(stack) = parsed {
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

fn synthesize_fields(control: &[u8]) -> (Vec<u8>, usize) {
    let field_count = usize::from(selector(control.first().copied().unwrap_or(0)) % 17);
    let padding = match selector(control.get(1).copied().unwrap_or(0)) % 6 {
        0 => 0,
        1 => 1,
        2 => 16,
        3 => 1_024,
        4 => 8_192,
        _ => MAX_INPUT.saturating_sub(84),
    };
    let max_layers = usize::from(selector(control.get(2).copied().unwrap_or(3)) % 4) + 1;
    let mut output = Vec::with_capacity(MAX_INPUT);
    let padding_per_field = padding / field_count.max(1);

    for index in 0..field_count {
        if index != 0 {
            if output.len() == MAX_INPUT {
                break;
            }
            output.push(0);
        }
        let token_selector = control.get(index + 3).copied().unwrap_or(index as u8);
        let token = match selector(token_selector) % 8 {
            0 => b"gzip".as_slice(),
            1 => b"br".as_slice(),
            2 => b"deflate".as_slice(),
            3 => b"zstd".as_slice(),
            4 => b"identity".as_slice(),
            5 => b"compress".as_slice(),
            6 => b"".as_slice(),
            _ => b"gzip;level=1".as_slice(),
        };
        let remaining = MAX_INPUT.saturating_sub(output.len() + token.len());
        output.extend(std::iter::repeat_n(b' ', padding_per_field.min(remaining)));
        if output.len().saturating_add(token.len()) <= MAX_INPUT {
            output.extend_from_slice(token);
        }
    }
    debug_assert!(output.len() <= MAX_INPUT);
    (output, max_layers)
}

const fn selector(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => value,
    }
}
