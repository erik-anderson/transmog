use super::*;
fn run(headers: Vec<Header>, target: Option<&str>) -> Inspection {
    inspect(
        Input {
            headers,
            target: target.map(str::to_owned),
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(false)),
    )
}
fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.into(),
        value: value.as_bytes().into(),
        redacted: false,
        limited: false,
    }
}

#[test]
fn custom_auth_access_and_token_headers_decode_jwt_without_a_fixed_prefix() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    for protected in [
        r#"{"alg":"HS256","typ":"JWT"}"#,
        "{\n \"typ\":\"JWT\",\"alg\":\"HS256\"}",
    ] {
        let token = format!(
            "{}.{}.c2ln",
            URL_SAFE_NO_PAD.encode(protected),
            URL_SAFE_NO_PAD.encode(r#"{"sub":"someone","nonce":"private-nonce"}"#)
        );
        for name in [
            "x-gerrit-auth",
            "MOCSI-AccessToken",
            "X-ACCESS-Key",
            "X-Token",
        ] {
            for value in [token.clone(), format!(" \tbEaReR\t {token} \t")] {
                let result = run(vec![header(name, &value), header(name, &value)], None);
                assert_eq!(result.findings.len(), 2);
                for (index, finding) in result.findings.iter().enumerate() {
                    assert_eq!(finding.format, "JWT / JWS");
                    assert_eq!(finding.verification, "Not checked");
                    assert_eq!(finding.source.header_index, Some(index));
                    assert_eq!(finding.source.offset, value.find(&token));
                    assert_eq!(finding.source.length, Some(token.len()));
                    let raw = finding
                        .fields
                        .iter()
                        .find(|f| f.name == "Compact token")
                        .unwrap();
                    assert!(raw.sensitive);
                    assert_eq!(result.value(&raw.id), Some(token.as_str()));
                }
                let serialized = serde_json::to_string(&result).unwrap();
                assert!(!serialized.contains(&token));
                assert!(!serialized.contains("private-nonce"));
            }
        }
    }
}

#[test]
fn custom_header_heuristic_skips_non_tokens_and_obeys_redaction_and_limits() {
    let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.";
    for value in [
        "",
        "opaque-csrf-token",
        "eyJhbGciOiJ",
        "foo.bar.baz",
        "*.example.test",
    ] {
        assert!(run(vec![header("X-Auth", value)], None).findings.is_empty());
    }
    assert!(
        run(vec![header("X-Unrelated", token)], None)
            .findings
            .is_empty()
    );
    let result = run(
        vec![Header {
            redacted: true,
            ..header("MOCSI-AccessToken", token)
        }],
        None,
    );
    assert_eq!(result.findings[0].status, "redacted");
    assert!(!serde_json::to_string(&result).unwrap().contains(token));
    let result = run(
        vec![header("X-Gerrit-Auth", &"a".repeat(MAX_CANDIDATE + 1))],
        None,
    );
    assert!(result.findings.is_empty());
    assert!(
        result
            .coverage
            .iter()
            .any(|c| c.source == "headers" && c.status == "limited")
    );
}

#[test]
fn jwt_converter_preserves_json_evidence_and_accepts_bearer_values() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let token = format!("{}.{}.c2ln", URL_SAFE_NO_PAD.encode("{\n\"typ\":\"JWT\",\"alg\":\"HS256\"}"), URL_SAFE_NO_PAD.encode(r#"{"sub":"user","exp":9007199254740993.125,"sub":"other","nested":[{},[],{"quoted":"a\nb\"c 🦊"}]}"#));
    let decoded = decode_jwt(&format!(" \r\nBearer {token}\t")).unwrap();
    assert_eq!(decoded.matches("\"sub\":").count(), 2);
    assert!(decoded.contains("9007199254740993.125"));
    let value: serde_json::Value = serde_json::from_str(&decoded).unwrap();
    assert_eq!(value["header"]["alg"], "HS256");
    assert_eq!(value["payload"]["nested"][2]["quoted"], "a\nb\"c 🦊");
    assert_eq!(value["signature"], "c2ln");
    assert_eq!(value["verification"], "Not checked");
    let unsigned = "eyJhbGciOiJub25lIn0.e30.";
    assert!(decode_jwt(unsigned).is_ok());
}

#[test]
fn jwt_converter_rejects_malformed_encrypted_and_bounded_inputs_without_echoes() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    for (protected, payload, signature) in [
        (r#"{"alg":"HS256"}"#, r#"{"sub":"private-marker"}"#, ""),
        (r#"{"alg":"none"}"#, "[]", ""),
        (r#"{"alg":"none"}"#, "{\u{b}\"sub\":1}", ""),
        (r#"{"alg":"none"}"#, "{\"sub\":\"bad\u{1}\"}", ""),
        (r#"{"alg":"none","alg":"HS256"}"#, "{}", ""),
        (r#"{"alg":"none","b64":false}"#, "{}", ""),
        (r#"{"alg":"none","b64":true,"b64":false}"#, "{}", ""),
        (r#"{"alg":"HS256"}"#, "{}", "!!!private-marker"),
    ] {
        let token = format!(
            "{}.{}.{signature}",
            URL_SAFE_NO_PAD.encode(protected),
            URL_SAFE_NO_PAD.encode(payload)
        );
        let error = decode_jwt(&token).unwrap_err();
        assert!(!error.contains("private-marker"));
    }
    for token in [
        "",
        "eyJhbGciOiJ",
        "a.b.c",
        "eyJhbGciOiJub25lIn0.@@.",
        "eyJhbGciOiJub25lIn0.e30..extra",
        "eyJhbGciOiJub25lIn0.e30=.",
    ] {
        assert!(decode_jwt(token).is_err());
    }
    assert!(
        decode_jwt("eyJhbGciOiJkaXIiLCJlbmMiOiJBMTI4R0NNIn0..aXY.Y2lwaGVy.dGFn")
            .unwrap_err()
            .contains("encrypted")
    );
    assert!(decode_jwt(&"a".repeat(MAX_CANDIDATE + 1)).is_err());
    let deep = format!(
        "eyJhbGciOiJub25lIn0.{}.",
        URL_SAFE_NO_PAD.encode(format!(
            "{{\"deep\":{}0{}}}",
            "[".repeat(30),
            "]".repeat(30)
        ))
    );
    assert!(decode_jwt(&deep).is_err());
}

#[test]
fn jwt_payload_encoding_requires_an_unambiguous_boolean() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    for (metadata, expected) in [
        (r#""b64":true"#, "decoded"),
        (r#""b64":false"#, "unsupported"),
        (r#""b64":"true""#, "malformed"),
        (r#""b64":"false""#, "malformed"),
        (r#""b64":null"#, "malformed"),
        (r#""b64":true,"b64":false"#, "malformed"),
        (r#""b64":true,"b64":true"#, "malformed"),
    ] {
        let protected = format!(r#"{{"alg":"HS256",{metadata}}}"#);
        let token = format!(
            "{}.{}.c2ln",
            URL_SAFE_NO_PAD.encode(protected),
            URL_SAFE_NO_PAD.encode(r#"{"sub":"encoding-test"}"#)
        );
        assert_eq!(
            decode_jwt(&token).is_ok(),
            expected == "decoded",
            "{metadata}"
        );
        let inspection = run(vec![header("X-Gerrit-Auth", &token)], None);
        let finding = &inspection.findings[0];
        assert_eq!(finding.status, expected, "{metadata}");
        assert_eq!(
            finding.fields.iter().any(|field| field.name == "sub"),
            expected == "decoded"
        );
    }
}

#[test]
fn unsecured_jwt_must_have_an_empty_signature() {
    let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.c2ln";
    assert!(decode_jwt(token).is_err());
    let inspection = run(vec![header("MOCSI-AccessToken", token)], None);
    assert_eq!(inspection.findings[0].status, "malformed");
}

#[test]
fn custom_jose_parser_and_artifact_limits_do_not_report_complete_header_coverage() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let oversized_header = format!(
        r#"{{"alg":"none","extra":[{}]}}"#,
        vec!["0"; MAX_FIELDS].join(",")
    );
    let oversized_payload = format!(
        r#"{{"sub":"user","extra":[{}]}}"#,
        vec!["0"; MAX_FIELDS].join(",")
    );
    for (protected, payload) in [
        (oversized_header.as_str(), "{}"),
        (r#"{"alg":"none"}"#, oversized_payload.as_str()),
    ] {
        let token = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(protected),
            URL_SAFE_NO_PAD.encode(payload)
        );
        let inspection = run(vec![header("X-Access-Token", &token)], None);
        assert_eq!(inspection.findings.len(), 1);
        assert_eq!(inspection.findings[0].status, "limited");
        assert!(
            inspection
                .coverage
                .iter()
                .any(|source| source.source == "headers" && source.status == "limited")
        );
    }
    let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.";
    let inspection = run((0..65).map(|_| header("X-Token", token)).collect(), None);
    assert_eq!(inspection.findings.len(), 64);
    assert!(
        inspection
            .coverage
            .iter()
            .any(|source| source.source == "headers" && source.status == "limited")
    );
}

#[test]
fn custom_jose_header_decode_budget_remains_visible() {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let opaque = "x".repeat(MAX_CANDIDATE);
    let mut headers: Vec<_> = (0..7).map(|_| header("X-Auth", &opaque)).collect();
    let protected = format!(
        r#"{{"alg":"none","extra":"{}"}}"#,
        "x".repeat(MAX_CANDIDATE / 2)
    );
    let token = format!("{}.e30.", URL_SAFE_NO_PAD.encode(protected));
    headers.push(header("X-Token", &token));
    let inspection = run(headers, None);
    assert_eq!(inspection.findings.len(), 1);
    assert_eq!(inspection.findings[0].status, "limited");
    assert!(
        inspection
            .coverage
            .iter()
            .any(|source| source.source == "headers" && source.status == "limited")
    );
}

#[test]
fn oversized_custom_header_names_are_not_copied_or_decoded() {
    let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.";
    let name = format!("X-Auth-{}", "x".repeat(MAX_CANDIDATE));
    let inspection = run(vec![header(&name, token), header("X-Token", token)], None);
    assert_eq!(inspection.findings[0].status, "limited");
    assert_eq!(inspection.findings[0].source.header_index, Some(0));
    assert_eq!(inspection.findings[1].format, "JWT / JWS");
    assert_eq!(inspection.findings[1].source.header_index, Some(1));
    assert!(!serde_json::to_string(&inspection).unwrap().contains(&name));
    assert!(
        inspection
            .coverage
            .iter()
            .any(|source| source.source == "headers" && source.status == "limited")
    );
}
#[test]
fn quoted_commas_duplicates_and_bare_challenges() {
    let result = run(
        vec![header(
            "WWW-Authenticate",
            r#"Digest realm="a,b", nonce="secret", realm="other", Basic realm="ok", Negotiate"#,
        )],
        None,
    );
    assert_eq!(result.findings.len(), 3);
    assert_eq!(result.findings[0].fields[1].value, "a,b");
    assert_eq!(result.findings[0].status, "incomplete");
    assert_eq!(result.findings[2].format, "Negotiate");
    assert!(!serde_json::to_string(&result).unwrap().contains("secret"));
}
#[test]
fn basic_first_colon_and_secret_serialization() {
    let result = run(
        vec![header("Authorization", "Basic dXNlcjpwYXNzOm1vcmU=")],
        None,
    );
    let password = result.findings[0]
        .fields
        .iter()
        .find(|f| f.name == "Password")
        .unwrap();
    assert_eq!(result.value(&password.id), Some("pass:more"));
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("pass:more"));
    assert!(!serialized.contains("dXNlcjpwYXNzOm1vcmU="));
}
#[test]
fn retained_occurrences_redaction_and_url_fragments() {
    let mut headers: Vec<_> = (0..520).map(|_| header("x-test", "ignored")).collect();
    headers.push(header("aUtHoRiZaTiOn", "Bearer private-token"));
    headers.push(Header {
        redacted: true,
        limited: false,
        ..header("Authorization", "Basic dXNlcjpwYXNz")
    });
    headers.push(header(
        "Location",
        "/cb#access_token=a%2Bb&access_token=&id_token",
    ));
    let result = run(headers, Some("/path#access_token=not-transmitted"));
    assert_eq!(result.findings[0].source.occurrence, 520);
    assert_eq!(result.findings[1].status, "redacted");
    assert_eq!(result.findings.len(), 3);
    assert_eq!(result.findings[2].status, "incomplete");
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-token")
    );
}
#[test]
fn malformed_and_canceled_inputs_are_bounded() {
    for value in [
        "Basic !!!",
        "Digest a=\"unterminated",
        "unknown a=1, a=2",
        "Bearer a=b=c",
        "",
        ",,,",
    ] {
        let _ = run(
            vec![header("Authorization", value)],
            Some("/?access_token=%zz"),
        );
    }
    let result = inspect(
        Input {
            headers: vec![header("Authorization", "Basic dXNlcjpwYXNz")],
            target: None,
            body: None,
            body_status: "skipped".into(),
            captured_at: None,
        },
        Arc::new(AtomicBool::new(true)),
    );
    assert!(result.findings.is_empty());
}

#[test]
fn non_ascii_challenges_and_authentication_info_provenance() {
    let result = run(
        vec![
            header(
                "WWW-Authenticate",
                "Digest realm=\"café\", nonce=\"secret\"",
            ),
            header(
                "Authentication-Info",
                "nextnonce=\"private-next\", qop=auth",
            ),
            header("Authorization", "Basic"),
        ],
        None,
    );
    assert_eq!(result.findings[0].fields[1].value, "café");
    assert!(result.findings[1].source.offset.is_none());
    assert_eq!(result.findings[2].status, "malformed");
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private-next")
    );
}
