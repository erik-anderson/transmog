use super::*;
use base64::Engine as _;
use std::io::Write as _;

#[test]
fn saml_binding_character_decoding_honors_bom_and_xml_declaration() {
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?><a:Assertion xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion"><a:Issuer>synthetic</a:Issuer></a:Assertion>"#;
    let mut bytes = vec![0xff, 0xfe];
    for word in xml.encode_utf16() {
        bytes.extend(word.to_le_bytes());
    }
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .replace('+', "%2B");
    let result = body(
        "application/x-www-form-urlencoded",
        format!("SAMLResponse={encoded}").into_bytes(),
    );
    assert_eq!(result.findings[0].children[0].format, "SAML 2.0 XML");
    assert!(
        result.findings[0].children[0]
            .source
            .transforms
            .iter()
            .any(|step| step.contains("UTF-16LE"))
    );
    let inner = base64::engine::general_purpose::STANDARD.encode(
        br#"<?xml version="1.0" encoding="UTF-8"?><a:Assertion xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion"/>"#,
    );
    let mut outer = vec![0xff, 0xfe];
    for word in format!("SAMLResponse={}", inner.replace('+', "%2B")).encode_utf16() {
        outer.extend(word.to_le_bytes());
    }
    let result = body("application/x-www-form-urlencoded; charset=utf-16", outer);
    assert_eq!(result.findings[0].children[0].format, "SAML 2.0 XML");
    assert_eq!(result.findings[0].children[0].status, "decoded");
    let result=body("application/xml",br#"<?xml version="1.0" encoding="ISO-8859-1"?><a:Assertion xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion"/>"#.to_vec());
    assert_eq!(
        result
            .coverage
            .iter()
            .find(|source| source.source == "body")
            .unwrap()
            .status,
        "unsupported"
    );
}

#[test]
fn fractional_time_conditions_do_not_invent_subsecond_capture_ordering() {
    let token = compact(
        r#"{"alg":"none","typ":"JWT"}"#,
        r#"{"iss":"synthetic","aud":["workbench"],"admin":false,"exp":1700000000.5}"#,
        "",
    );
    let result = body(
        "application/json",
        format!(r#"{{"id_token":"{token}"}}"#).into_bytes(),
    );
    assert!(
        result.findings[0].children[0]
            .fields
            .iter()
            .any(|field| field.name == "exp" && field.note.contains("uncertain"))
    );
    for name in ["iss", "aud[0]", "admin"] {
        let field = result.findings[0].children[0]
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap();
        assert!(field.note.is_empty(), "Generic note repeated for {name}");
    }
    let result=body("application/xml",br#"<a:Assertion xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion" Version="2.0"><a:Issuer>synthetic</a:Issuer><a:Conditions NotOnOrAfter="2023-11-14T22:13:20.500Z"/></a:Assertion>"#.to_vec());
    assert!(
        result.findings[0]
            .fields
            .iter()
            .any(|field| field.name.contains("NotOnOrAfter") && field.note.contains("uncertain"))
    );
    for name in ["Issuer", "Assertion @Version"] {
        let field = result.findings[0]
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap();
        assert!(field.note.is_empty(), "Generic note repeated for {name}");
    }
}

#[test]
fn generic_json_jose_and_context_limited_oauth_errors() {
    let token = compact(
        r#"{"alg":"none","typ":"JWT"}"#,
        r#"{"sub":"synthetic"}"#,
        "",
    );
    let result = body(
        "application/json",
        format!(r#"{{"token":"{token}"}}"#).into_bytes(),
    );
    assert_eq!(result.findings[0].format, "JWT / JWS");
    assert!(
        body("application/json", br#"{"token":"a.b.c"}"#.to_vec())
            .findings
            .is_empty()
    );
    let result = body(
        "application/json",
        br#"{"error":"invalid_grant","error_description":"Synthetic expired grant"}"#.to_vec(),
    );
    assert_eq!(result.findings[0].recognition, "candidate");
}

#[test]
fn bounded_extraction_never_reports_unscanned_tail_as_complete() {
    let form = (0..2050)
        .map(|index| format!("ignored{index}=value"))
        .collect::<Vec<_>>()
        .join("&");
    let result = body("application/x-www-form-urlencoded", form.into_bytes());
    assert!(result.findings.is_empty());
    assert_eq!(
        result
            .coverage
            .iter()
            .find(|source| source.source == "body")
            .unwrap()
            .status,
        "limited"
    );
    let html = "<input type=hidden name=ignored value=x>".repeat(257);
    let result = body("text/html", html.into_bytes());
    assert_eq!(
        result
            .coverage
            .iter()
            .find(|source| source.source == "body")
            .unwrap()
            .status,
        "limited"
    );
    let scalar = "x".repeat(MAX_CANDIDATE + 1);
    let result = body(
        "application/json",
        format!(r#"{{"ignored":"{scalar}"}}"#).into_bytes(),
    );
    assert_eq!(
        result
            .coverage
            .iter()
            .find(|source| source.source == "body")
            .unwrap()
            .status,
        "limited"
    );
    let result = body(
        "application/x-www-form-urlencoded",
        format!("SAMLResponse={scalar}").into_bytes(),
    );
    assert_eq!(result.findings[0].status, "limited");
}
fn body(mime: &str, bytes: Vec<u8>) -> Inspection {
    inspect(
        Input {
            headers: vec![],
            target: None,
            body: Some(Body {
                bytes,
                media_type: mime.into(),
                transforms: vec![],
            }),
            body_status: "complete".into(),
            captured_at: Some(1_700_000_000),
        },
        Arc::new(AtomicBool::new(false)),
    )
}
fn compact(header: &str, payload: &str, signature: &str) -> String {
    let encode = |v| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "{}.{}.{}",
        encode(header),
        encode(payload),
        encode(signature)
    )
}
#[test]
fn jose_precision_duplicate_members_time_and_encryption() {
    let token = compact(
        r#"{"alg":"none","typ":"JWT"}"#,
        r#"{"iss":"example","aud":["a","b"],"exp":1700000100,"custom":900719925474099300000,"sub":"a","sub":"b","nonce":"private-nonce"}"#,
        "",
    );
    let result = body(
        "application/json",
        format!(r#"{{"id_token":"{token}"}}"#).into_bytes(),
    );
    let jwt = &result.findings[0].children[0];
    assert_eq!(jwt.verification, "Not checked");
    assert_eq!(jwt.status, "incomplete");
    assert!(
        jwt.fields
            .iter()
            .any(|f| f.value == "900719925474099300000")
    );
    assert!(
        jwt.fields
            .iter()
            .any(|f| f.name == "exp" && f.note.contains("not crossed"))
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-nonce")
    );
    let token = format!(
        "{}..AA.AA.AA",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"dir","enc":"A256GCM"}"#)
    );
    let result = body(
        "application/json",
        format!(r#"{{"id_token":"{token}"}}"#).into_bytes(),
    );
    assert_eq!(result.findings[0].children[0].status, "encrypted");
}
#[test]
fn json_oauth_context_and_duplicates() {
    assert!(
        body(
            "application/json",
            br#"{"code":"example","state":"example","error":"example"}"#.to_vec()
        )
        .findings
        .is_empty()
    );
    let result = body("application/problem+json", br#"{"grant_type":"refresh_token","refresh_token":"one-secret","refresh_token":"two-secret","client_id":"example"}"#.to_vec());
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].status, "incomplete");
    let shown = serde_json::to_string(&result).unwrap();
    assert!(!shown.contains("one-secret"));
    assert!(!shown.contains("two-secret"));
}
#[test]
fn saml_post_redirect_namespaces_entities_and_attack_limits() {
    let xml = r#"<p:Response xmlns:p="urn:oasis:names:tc:SAML:2.0:protocol" InResponseTo="private-id"><a:Issuer xmlns:a="urn:oasis:names:tc:SAML:2.0:assertion">example&amp;issuer</a:Issuer><p:Status><p:StatusCode Value="Success"/></p:Status></p:Response>"#;
    let encoded = base64::engine::general_purpose::STANDARD.encode(xml);
    let result = body(
        "application/x-www-form-urlencoded",
        format!(
            "SAMLResponse={}",
            encoded.replace('+', "%2B").replace('=', "%3D")
        )
        .into_bytes(),
    );
    assert_eq!(result.findings[0].children[0].format, "SAML 2.0 XML");
    let mut compressor =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    compressor.write_all(xml.as_bytes()).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(compressor.finish().unwrap());
    let result = inspect(
        Input {
            headers: vec![],
            target: Some(format!(
                "/?SAMLResponse={}",
                encoded.replace('+', "%2B").replace('=', "%3D")
            )),
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(false)),
    );
    assert!(
        result.findings[0].children[0]
            .source
            .transforms
            .iter()
            .any(|s| s == "Raw DEFLATE")
    );
    assert!(
        body("application/xml", b"<Response>not SAML</Response>".to_vec())
            .findings
            .is_empty()
    );
    let result = body("application/xml", format!("{xml}<extra/>").into_bytes());
    assert_ne!(result.findings[0].status, "decoded");
    let attack = format!("<!DOCTYPE p:Response [<!ENTITY x SYSTEM 'file:///secret'>]>{xml}");
    assert!(
        body("application/xml", attack.into_bytes())
            .findings
            .is_empty()
    );
}
#[test]
fn inert_hidden_html_and_unicode_are_bounded() {
    let result = body("text/html", br#"<script>"<input type=hidden name=id_token value=ignore>"</script><input TYPE='hidden' name='client_id' value='a&amp;b'><input type=hidden name=response_type value=code><input type=hidden name=state value='secret&#x2b;state'>"#.to_vec());
    assert_eq!(result.findings.len(), 1);
    assert!(
        result.findings[0]
            .fields
            .iter()
            .any(|f| f.name == "client_id" && f.value == "a&b")
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("secret+state")
    );
    for bytes in [vec![0xff, 0xfe, 0], b"{\"x\": [".to_vec(), vec![b'['; 4096]] {
        let _ = body("application/json", bytes);
    }
}

#[test]
fn explicit_interpretation_is_bounded_masked_and_does_not_invent_success() {
    let input = || Input {
        headers: vec![],
        target: None,
        body: Some(Body {
            bytes: compact(
                r#"{"alg":"none","typ":"JWT"}"#,
                r#"{"sub":"synthetic"}"#,
                "",
            )
            .into_bytes(),
            media_type: "text/plain".into(),
            transforms: vec![],
        }),
        body_status: "complete".into(),
        captured_at: None,
    };
    let result = inspect_as(input(), Arc::new(AtomicBool::new(false)), "jose");
    assert_eq!(result.findings[0].format, "JWT / JWS");
    assert_eq!(result.findings[0].verification, "Not checked");
    let result = inspect_as(input(), Arc::new(AtomicBool::new(false)), "saml");
    assert_eq!(result.findings[0].recognition, "candidate");
    assert_eq!(result.findings[0].status, "malformed");
}

#[test]
fn redirect_header_parent_occurrence_survives_nested_query_decoding() {
    let result = inspect(
        Input {
            headers: vec![
                Header {
                    name: "x-unrelated".into(),
                    value: vec![],
                    redacted: false,
                    limited: false,
                },
                Header {
                    name: "Location".into(),
                    value: b"/callback?other=1&access_token=private&access_token=again".to_vec(),
                    redacted: false,
                    limited: false,
                },
            ],
            target: None,
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(result.findings[0].source.header_index, Some(1));
    assert_eq!(result.findings[0].source.occurrence, 1);
}
#[test]
fn aggregate_nodes_compression_bombs_and_nested_prefixes_are_finite() {
    let mut compressor =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    compressor
        .write_all(&vec![b'A'; 4 * 1024 * 1024 + 1])
        .unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(compressor.finish().unwrap());
    let result = inspect(
        Input {
            headers: vec![],
            target: Some(format!("/?SAMLResponse={}", encoded.replace('+', "%2B"))),
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(result.findings[0].status, "limited");
    let fields = std::iter::once("grant_type=client_credentials".into())
        .chain((0..4096).map(|n| format!("extra{n}=private")))
        .collect::<Vec<_>>()
        .join("&");
    let result = body("application/x-www-form-urlencoded", fields.into_bytes());
    assert_ne!(result.diagnostics, [] as [std::string::String; 0]);
    assert!(result.values.len() <= MAX_FIELDS);
    let xml = br#"<p:AuthnRequest xmlns:p="urn:oasis:names:tc:SAML:2.0:protocol" ID="synthetic"/>"#;
    for end in 0..xml.len() {
        let _ = body("application/xml", xml[..end].to_vec());
    }
    let mut value =
        br#"{"id_token":"opaque","nested":[{"grant_type":"client_credentials"}]}"#.to_vec();
    for end in 0..value.len() {
        let _ = body("application/json", value[..end].to_vec());
    }
    value = vec![b'['; MAX_DEPTH + 10];
    value.extend(vec![b']'; MAX_DEPTH + 10]);
    assert!(body("application/json", value).findings.is_empty());
}
