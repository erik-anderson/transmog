use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
fn token(bytes: &[u8], scheme: &str) -> Inspection {
    inspect(
        Input {
            headers: vec![Header {
                name: "WWW-Authenticate".into(),
                value: format!("{scheme} {}", STANDARD.encode(bytes)).into_bytes(),
                redacted: false,
                limited: false,
            }],
            target: None,
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(false)),
    )
}
fn descriptor(bytes: &mut [u8], at: usize, length: u16, offset: u32) {
    bytes[at..at + 2].copy_from_slice(&length.to_le_bytes());
    bytes[at + 2..at + 4].copy_from_slice(&length.to_le_bytes());
    bytes[at + 4..at + 8].copy_from_slice(&offset.to_le_bytes());
}
fn ntlm(kind: u32) -> Vec<u8> {
    let mut bytes = vec![
        0;
        match kind {
            1 => 40,
            2 => 60,
            _ => 88,
        }
    ];
    bytes[..8].copy_from_slice(b"NTLMSSP\0");
    bytes[8..12].copy_from_slice(&kind.to_le_bytes());
    match kind {
        1 => {
            bytes[12..16].copy_from_slice(&0x0200_0001u32.to_le_bytes());
        }
        2 => {
            bytes[20..24].copy_from_slice(&0x0280_0001u32.to_le_bytes());
            descriptor(&mut bytes, 40, 4, 56);
        }
        _ => {
            bytes[60..64].copy_from_slice(&0x0200_0001u32.to_le_bytes());
            descriptor(&mut bytes, 36, 0, 88);
        }
    }
    bytes
}
fn der(tag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut result = vec![tag];
    if bytes.len() < 128 {
        result.push(u8::try_from(bytes.len()).unwrap());
    } else if bytes.len() <= 255 {
        result.extend([0x81, u8::try_from(bytes.len()).unwrap()]);
    } else {
        result.extend([
            0x82,
            u8::try_from(bytes.len() >> 8).unwrap(),
            u8::try_from(bytes.len() & 255).unwrap(),
        ]);
    }
    result.extend(bytes);
    result
}
fn sequence(parts: Vec<Vec<u8>>) -> Vec<u8> {
    der(0x30, &parts.into_iter().flatten().collect::<Vec<_>>())
}
#[test]
fn ntlm_all_message_types_offsets_and_masks() {
    for kind in 1..=3 {
        let bytes = ntlm(kind);
        let result = token(&bytes, "NTLM");
        assert_eq!(result.findings[0].children[0].format, "NTLM");
        assert!(
            result.findings[0].children[0]
                .role
                .contains(&format!("Type {kind}"))
        );
        for end in 0..bytes.len() {
            let _ = token(&bytes[..end], "NTLM");
        }
    }
    let mut bytes = ntlm(3);
    descriptor(&mut bytes, 20, 65535, u32::MAX);
    let result = token(&bytes, "NTLM");
    assert_eq!(result.findings[0].children[0].status, "malformed");
    let mut bytes = ntlm(2);
    bytes[24..32].copy_from_slice(b"secret!!");
    assert!(
        !serde_json::to_string(&token(&bytes, "NTLM"))
            .unwrap()
            .contains("c2VjcmV0ISE=")
    );
}
#[test]
fn spnego_offered_selected_child_and_mic() {
    let ntlm_oid = der(6, &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 2, 2, 10]);
    let kerberos_oid = der(6, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 1, 2, 2]);
    let init = der(
        0xa0,
        &sequence(vec![
            der(0xa0, &sequence(vec![kerberos_oid, ntlm_oid.clone()])),
            der(0xa2, &der(4, &ntlm(1))),
            der(0xa3, &der(4, b"private-mic")),
        ]),
    );
    let result = token(&init, "Negotiate");
    for operation in ["auth-decode", "negotiate-decode"] {
        let decoded =
            decode_authentication(&STANDARD.encode(&init), operation, &AtomicBool::new(false))
                .unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(decoded["format"], "NTLM");
        assert_eq!(decoded["status"], "decoded");
        assert_eq!(decoded["fields"]["Message type"], "1");
        let layers = decoded["details"]["layers"].as_array().unwrap();
        let spnego = layers
            .iter()
            .find(|layer| layer["format"] == "SPNEGO")
            .unwrap();
        assert_eq!(
            spnego["decoded"]["mechanismBytes"]["data"],
            STANDARD.encode(&init)
        );
        assert_eq!(
            spnego["values"]["Mechanism-list MIC"],
            STANDARD.encode(b"private-mic")
        );
        assert!(spnego["source"]["transforms"].is_array());
        if operation == "negotiate-decode" {
            assert_eq!(layers[0]["format"], "Negotiate");
            assert_eq!(layers[0]["status"], "opaque");
        }
    }
    let finding = &result.findings[0].children[0];
    assert_eq!(finding.format, "SPNEGO");
    assert_eq!(
        finding
            .fields
            .iter()
            .filter(|f| f.name == "Offered mechanism")
            .count(),
        2
    );
    assert_eq!(finding.children[0].format, "NTLM");
    assert!(
        !finding
            .fields
            .iter()
            .any(|f| f.name == "Selected mechanism")
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("cHJpdmF0ZS1taWM=")
    );
    let response = der(
        0xa1,
        &sequence(vec![der(0xa0, &der(10, &[1])), der(0xa1, &ntlm_oid)]),
    );
    let result = token(&response, "Negotiate");
    assert!(
        result.findings[0].children[0]
            .fields
            .iter()
            .any(|f| f.name == "Selected mechanism")
    );
}

#[test]
fn bare_and_prefixed_windows_conversions_promote_without_invented_source_ranges() {
    for operation in ["ntlm-decode", "negotiate-decode"] {
        for prefixed in [false, true] {
            let bare = STANDARD.encode(ntlm(1));
            let text = if prefixed {
                format!(
                    "{} {bare}",
                    if operation == "ntlm-decode" {
                        "NTLM"
                    } else {
                        "Negotiate"
                    }
                )
            } else {
                bare
            };
            let json = decode_authentication(&text, operation, &AtomicBool::new(false)).unwrap();
            let json: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(json["format"], "NTLM");
            assert_eq!(json["status"], "decoded");
            assert_eq!(json["fields"]["Message type"], "1");
            if !prefixed {
                for layer in json["details"]["layers"].as_array().unwrap() {
                    assert!(layer["source"]["offset"].is_null());
                    assert!(layer["source"]["length"].is_null());
                    assert_eq!(
                        layer["source"]["transforms"][0],
                        "Explicit format interpretation of a bare token or parameter list"
                    );
                }
            }
        }
    }
}
#[test]
fn kerberos_exposed_ticket_and_encrypted_boundaries() {
    let encrypted = sequence(vec![
        der(0xa0, &der(2, &[18])),
        der(0xa1, &der(2, &[3])),
        der(0xa2, &der(4, b"opaque-ciphertext")),
    ]);
    let principal = sequence(vec![
        der(0xa0, &der(2, &[2])),
        der(
            0xa1,
            &sequence(vec![der(0x1b, b"HTTP"), der(0x1b, b"service.example")]),
        ),
    ]);
    let ticket = der(
        0x61,
        &sequence(vec![
            der(0xa0, &der(2, &[5])),
            der(0xa1, &der(0x1b, b"EXAMPLE.TEST")),
            der(0xa2, &principal),
            der(0xa3, &encrypted),
        ]),
    );
    let request = der(
        0x6e,
        &sequence(vec![
            der(0xa0, &der(2, &[5])),
            der(0xa1, &der(2, &[14])),
            der(0xa2, &der(3, &[0, 0, 0, 0, 0])),
            der(0xa3, &ticket),
            der(0xa4, &encrypted),
        ]),
    );
    let result = token(&request, "Negotiate");
    let finding = &result.findings[0].children[0];
    assert_eq!(finding.role, "AP-REQ");
    assert_eq!(finding.status, "encrypted");
    assert!(finding.fields.iter().any(|f| f.value == "EXAMPLE.TEST"));
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("opaque-ciphertext")
    );
    let mut payload = der(6, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 1, 2, 2]);
    payload.extend([1, 0]);
    payload.extend(&request);
    let wrapped = der(0x60, &payload);
    assert_eq!(
        token(&wrapped, "Negotiate").findings[0].children[0].children[0].role,
        "AP-REQ"
    );
    for end in 0..wrapped.len() {
        let _ = token(&wrapped[..end], "Negotiate");
    }
}
#[test]
fn malformed_asn1_and_randomized_tokens_never_panic() {
    for bytes in [
        vec![0x60, 0x80, 0, 0],
        vec![0xa0, 0x84, 255, 255, 255, 255],
        vec![0xa0, 1, 0xff],
        vec![0x60, 0, 0],
    ] {
        let _ = token(&bytes, "Negotiate");
    }
    let mut state = 0x0123_4567u32;
    for length in 0..512 {
        let bytes: Vec<_> = (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state.to_le_bytes()[3]
            })
            .collect();
        let _ = token(&bytes, "Negotiate");
        let _ = token(&bytes, "NTLM");
    }
}

#[test]
fn explicit_interpretation_accepts_direct_binary_and_base64_spnego() {
    let init = der(0xa0, &sequence(vec![der(0xa2, &der(4, &ntlm(1)))]));
    for bytes in [init.clone(), STANDARD.encode(&init).into_bytes()] {
        let result = inspect_as(
            Input {
                headers: vec![],
                target: None,
                body: Some(Body {
                    bytes,
                    media_type: "application/octet-stream".into(),
                    transforms: vec![],
                }),
                body_status: "complete".into(),
                captured_at: None,
            },
            Arc::new(AtomicBool::new(false)),
            "negotiate",
        );
        assert_eq!(result.findings[0].format, "SPNEGO");
        assert_eq!(result.findings[0].children[0].format, "NTLM");
    }
}
