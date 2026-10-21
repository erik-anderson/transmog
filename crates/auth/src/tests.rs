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
    }
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
