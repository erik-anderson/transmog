#![no_main]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, atomic::AtomicBool};
use transmog_auth::{Body, Header, Input};
fuzz_target!(|data: &[u8]| {
    if data.len() > 8192 {
        return;
    }
    let kind = data.first().copied().unwrap_or(0) % 5;
    let scheme = if kind == 0 { "NTLM" } else { "Negotiate" };
    let headers = vec![Header {
        name: "Authorization".into(),
        value: if kind < 2 {
            format!("{scheme} {}", STANDARD.encode(data)).into_bytes()
        } else {
            data.to_vec()
        },
        redacted: false,
        limited: false,
    }];
    let input = Input {
        headers,
        target: Some(format!(
            "/?SAMLResponse={}",
            STANDARD.encode(data).replace('+', "%2B")
        )),
        body: Some(Body {
            bytes: data.to_vec(),
            media_type: match kind {
                2 => "application/json",
                3 => "application/xml",
                _ => "text/html",
            }
            .into(),
            transforms: vec![],
        }),
        body_status: "complete".into(),
        captured_at: None,
    };
    let result = transmog_auth::inspect(input, Arc::new(AtomicBool::new(false)));
    assert!(result.findings.len() <= 64);
    let _ = serde_json::to_vec(&result).unwrap();
});
