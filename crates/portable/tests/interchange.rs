//! Production parser/converter tests use independent authored fixtures and native engines.
use std::{
    fs::File,
    io::{Cursor, Read},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_automation as native;
use transmog_portable as wire;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../verification/fixtures/portable")
}
fn fixture() -> wire::Collection {
    wire::Collection::parse(include_bytes!(
        "../../../verification/fixtures/portable/manifests/rules-only.json"
    ))
    .unwrap()
}
fn prepare(value: &wire::Collection, stage: &std::path::Path) -> wire::PreparedCollection {
    wire::PreparedCollection::prepare(
        &value.encode().unwrap(),
        stage,
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}
fn native_rule() -> native::Rule {
    native::Rule {
        id: "test-rule".into(),
        revision: 9_007_199_254_740_993,
        display_name: Some("Test header".into()),
        enabled: true,
        priority: 7,
        matcher: native::RuleMatcher::default(),
        request: native::RequestActions {
            headers: vec![native::HeaderOperation::append("X-Bytes", [0x80, 0xff]).unwrap()],
            ..Default::default()
        },
        response: native::ResponseActions::default(),
    }
}

#[test]
fn checked_in_rules_fixture_has_exact_revisions_order_headers_and_owned_objects() {
    let value = fixture();
    let rule = &value.experiments[0].variants[1].revisions[0].rules[0];
    let native = wire::to_native(rule, 0).unwrap();
    assert_eq!(native.revision, 9_007_199_254_740_993);
    assert_eq!(wire::from_native(&native).unwrap().revision, rule.revision);
    let stage = tempfile::tempdir().unwrap();
    let prepared = prepare(&value, stage.path());
    let sha = &value.objects[0].sha256;
    let mut bytes = Vec::new();
    prepared
        .open_object(sha)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"h1 { color: blue; }\n");
    assert_eq!(wire::sha256(&bytes), *sha);
    assert!(prepared.open_object("../outside").is_err());
    assert!(
        prepared.manifest().assets[0]
            .headers
            .iter()
            .any(|h| h.value_base64 == "gP8=")
    );
    drop(prepared);
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[test]
fn saved_response_revision_accepts_one_thousand_rules_and_rejects_one_more() {
    let mut value = fixture();
    let template = value.experiments[0].variants[1].revisions[0].rules[0].clone();
    value.experiments[0].variants[1].revisions[0].rules = (0..1_000)
        .map(|index| wire::Rule {
            id: format!("response-{index}"),
            ..template.clone()
        })
        .collect();
    let encoded = value.encode().unwrap();
    assert_eq!(wire::Collection::parse(&encoded).unwrap(), value);
    let mut case = wire::CaseManifest::parse(include_bytes!(
        "../../../verification/fixtures/portable/manifests/simple-css.json"
    ))
    .unwrap();
    case.experiments = vec![serde_json::to_value(&value.experiments[0]).unwrap()];
    let stage = tempfile::tempdir().unwrap();
    let canceled = AtomicBool::new(false);
    let prepared = wire::PreparedCase::prepare(
        &case.encode().unwrap(),
        stage.path(),
        |sha| {
            Ok(Box::new(File::open(
                corpus().join("objects/sha256").join(sha),
            )?))
        },
        &canceled,
    )
    .unwrap();
    let path = stage.path().join("thousand-response-case.tmcase");
    wire::write_case_package(&prepared, &path, None, false, &canceled).unwrap();
    let package = wire::Package::open(&path, None, &canceled).unwrap();
    let wire::PackageManifest::Case(restored) = package.manifest() else {
        panic!("Expected an investigation case");
    };
    assert_eq!(restored, &case);
    let import = restored
        .rules_for_import(std::slice::from_ref(&value.experiments[0].id), false)
        .unwrap();
    assert_eq!(
        import.experiments[0].variants[1].revisions[0].rules.len(),
        1_000
    );
    value.experiments[0].variants[1].revisions[0]
        .rules
        .push(wire::Rule {
            id: "one-too-many".into(),
            ..template
        });
    let error = value.encode().unwrap_err();
    assert!(error.to_string().contains("1,000"));
    assert!(wire::Collection::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    case.experiments = vec![serde_json::to_value(&value.experiments[0]).unwrap()];
    assert!(case.encode().unwrap_err().to_string().contains("1,000"));
}

#[test]
fn strict_json_rejects_duplicate_unknown_future_fields_and_excessive_depth() {
    let value = String::from_utf8(fixture().encode().unwrap()).unwrap();
    assert!(
        wire::Collection::parse(
            value
                .replacen(
                    "\"format\":",
                    "\"format\":\"transmog-investigation\",\"format\":",
                    1
                )
                .as_bytes()
        )
        .is_err()
    );
    let mut parsed = serde_json::from_str::<serde_json::Value>(&value).unwrap();
    parsed["unknown"] = true.into();
    assert!(wire::Collection::parse(&serde_json::to_vec(&parsed).unwrap()).is_err());
    parsed["schemaVersion"] = 99.into();
    assert!(matches!(
        wire::Collection::parse(&serde_json::to_vec(&parsed).unwrap()),
        Err(wire::Error::Unsupported(_))
    ));
    let mut value = fixture();
    value.required_features.push("future/99".into());
    assert!(matches!(value.encode(), Err(wire::Error::Unsupported(_))));
    let mut json = fixture().encode().unwrap();
    let index = json.iter().rposition(|b| *b == b'}').unwrap();
    json.splice(
        index..index,
        format!(",\"unknown\":{}0{}", "[".repeat(34), "]".repeat(34)).bytes(),
    );
    assert!(wire::Collection::parse(&json).is_err());
    assert!(wire::decimal("9007199254740993", false).is_ok());
    for invalid in ["01", "+1", "-1", "1.0", "18446744073709551616"] {
        assert!(wire::decimal(invalid, true).is_err());
    }
    let mut plain = fixture();
    plain.extensions.clear();
    let valid = String::from_utf8(plain.encode().unwrap()).unwrap();
    let overflowing = valid.replacen(
        "\"extensions\": {}",
        "\"extensions\": {\"test.number\":18446744073709551616}",
        1,
    );
    assert_ne!(valid, overflowing);
    assert!(wire::Collection::parse(overflowing.as_bytes()).is_err());
    let mut oversized = fixture();
    oversized
        .extensions
        .insert("test.text".into(), "x".repeat(8 * 1024 * 1024).into());
    assert!(oversized.encode().is_err());
}

#[test]
fn missing_corrupt_oversized_and_canceled_closures_publish_nothing() {
    let mut value = fixture();
    let stage = tempfile::tempdir().unwrap();
    value.assets[0].body.state = "missing".into();
    assert!(value.encode().is_err());
    let value = fixture().encode().unwrap();
    let result = wire::PreparedCollection::prepare(
        &value,
        stage.path(),
        |_| Ok(Box::new(Cursor::new(b"corrupt"))),
        &AtomicBool::new(false),
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
    let called = AtomicBool::new(false);
    let result = wire::PreparedCollection::prepare(
        &value,
        stage.path(),
        |_| {
            called.store(true, Ordering::Release);
            Ok(Box::new(Cursor::new([])))
        },
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(wire::Error::Canceled)));
    assert!(!called.load(Ordering::Acquire));
    let mut oversized = fixture();
    oversized.objects[0].bytes = "1073741825".into();
    assert!(oversized.encode().is_err());
    let mut duplicate = fixture();
    duplicate.objects.push(duplicate.objects[0].clone());
    assert!(duplicate.encode().is_err());
    let mut dangling = fixture();
    dangling.assets[0].body.sha256 = Some("a".repeat(64));
    dangling.assets[0].body.bytes = None;
    assert!(dangling.encode().is_err());
}

#[test]
fn url_query_examples_and_binary_predicates_round_trip_through_native_matching() {
    let addresses = [
        native::UrlCondition::Exact("https://portable.invalid:443/p/42?a=1&a=1".into()),
        native::UrlCondition::Pattern(native::UrlPattern {
            address: "https://portable.invalid/p/{digits}".into(),
            query: native::QueryCondition::Parameters(vec![
                native::QueryParameter {
                    name: "a".into(),
                    value: "1".into()
                };
                2
            ]),
            case_sensitive: true,
        }),
        native::UrlCondition::Regex(native::UrlRegex {
            pattern: "/p/[0-9]+".into(),
            scope: native::RegexScope::Path,
            whole: true,
            case_sensitive: true,
            query: Some(native::QueryCondition::Exact(Some("a=1&a=1".into()))),
        }),
    ];
    for address in addresses {
        let mut rule = native_rule();
        rule.matcher.url = Some(address);
        rule.matcher.request_headers = vec![native::HeaderPredicate {
            name: "X-Client".into(),
            condition: native::HeaderCondition::Prefix(b"fixture".to_vec()),
        }];
        rule.matcher.examples = vec![native::MatchExample {
            method: "GET".into(),
            url: "https://portable.invalid/p/42?a=1&a=1".into(),
            expected: true,
            headers: vec![native::ExampleHeader {
                name: "X-Client".into(),
                value: "fixture".into(),
            }],
        }];
        let copy = wire::to_native(&wire::from_native(&rule).unwrap(), 0).unwrap();
        assert_eq!(copy, rule);
        let request = native::request_for_test(
            "GET",
            "https://portable.invalid/p/42?a=1&a=1",
            &[("X-Client".into(), "fixture".into())],
        )
        .unwrap();
        let expected =
            serde_json::to_value(native::test_matcher(&rule.matcher, &request).unwrap()).unwrap();
        let actual =
            serde_json::to_value(native::test_matcher(&copy.matcher, &request).unwrap()).unwrap();
        assert_eq!(expected, actual);
    }
}

#[test]
fn header_and_literal_actions_preserve_native_semantics_and_reject_mixed_actions() {
    let mut original = native_rule();
    original
        .request
        .headers
        .push(native::HeaderOperation::append("x-bytes", b"duplicate").unwrap());
    let copy = wire::to_native(&wire::from_native(&original).unwrap(), 0).unwrap();
    assert_eq!(copy, original);
    let request = native::request_for_test("GET", "https://portable.invalid/", &[]).unwrap();
    let output = native::test_header_rule(&copy, &request, None).unwrap();
    assert_eq!(
        output.after.values("x-bytes").collect::<Vec<_>>(),
        [b"\x80\xff".as_slice(), b"duplicate".as_slice()]
    );
    native::compile(vec![copy], native::AutomationLimits::default()).unwrap();
    original.request.headers.extend([
        native::HeaderOperation::override_if_present("X-Bytes", [0xfe, 0x80]).unwrap(),
        native::HeaderOperation::override_if_present("X-Missing", "absent").unwrap(),
    ]);
    let saved = serde_json::to_value(&original).unwrap();
    assert_eq!(
        saved["request"]["headers"][2]["operation"],
        "override-if-present"
    );
    let restored: native::Rule = serde_json::from_value(saved).unwrap();
    assert_eq!(restored, original);
    let portable = wire::from_native(&restored).unwrap();
    let copy = wire::to_native(&portable, 0).unwrap();
    assert_eq!(copy, original);
    let output = native::test_header_rule(&copy, &request, None).unwrap();
    assert_eq!(
        output.after.values("x-bytes").collect::<Vec<_>>(),
        [b"\xfe\x80".as_slice()]
    );
    assert_eq!(output.after.values("x-missing").count(), 0);
    native::compile(vec![copy], native::AutomationLimits::default()).unwrap();
    original.request.headers.clear();
    original.response.text_transform = Some(native::LiteralTransform {
        find: "seed".into(),
        replacement: "edited".into(),
        occurrences: native::Occurrences::All,
        expected_matches: Some(2),
        media_type: "text/plain".into(),
        encoding: "utf-8".into(),
        max_bytes: 1024,
    });
    let copy = wire::to_native(&wire::from_native(&original).unwrap(), 0).unwrap();
    assert_eq!(copy, original);
    let response = transmog_core::ResponseHead {
        status: 200,
        headers: transmog_core::HeaderBlock::from_fields(vec![
            transmog_core::HeaderField::try_new("Content-Type", "text/plain").unwrap(),
        ]),
        source_version: transmog_core::HttpLegVersion::Http1,
    };
    let output = native::test_text_transform(&copy, &request, &response, b"seed and seed").unwrap();
    assert_eq!(output.after_text, "edited and edited");
    native::compile(vec![copy], native::AutomationLimits::default()).unwrap();
    original.request.replace_body = Some(b"unsupported mixed action".to_vec());
    assert!(matches!(
        wire::from_native(&original),
        Err(wire::Error::Unsupported(_))
    ));
    let mut invalid = native_rule();
    invalid.request.headers = vec![native::HeaderOperation::set("Host", "evil.invalid").unwrap()];
    let mut collection = fixture();
    collection.rules = vec![wire::from_native(&invalid).unwrap()];
    assert!(collection.encode().is_err());
}

#[test]
fn compressed_entity_object_is_not_decoded_or_reencoded_during_preparation() {
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../verification/fixtures/portable/manifests/coded-duplicate-headers.json"
    ))
    .unwrap();
    value["kind"] = "rules".into();
    value["requiredFeatures"] = serde_json::json!(["response-assets/1"]);
    let object = value.as_object_mut().unwrap();
    for key in ["sources", "notes", "runs"] {
        object.remove(key);
    }
    object
        .get_mut("objects")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .retain(|o| o["role"] != "capture");
    let bytes = serde_json::to_vec(&value).unwrap();
    let value = wire::Collection::parse(&bytes).unwrap();
    let stage = tempfile::tempdir().unwrap();
    let prepared = prepare(&value, stage.path());
    let source = value.assets[1].body.sha256.as_ref().unwrap();
    let mut actual = Vec::new();
    prepared
        .open_object(source)
        .unwrap()
        .read_to_end(&mut actual)
        .unwrap();
    assert_eq!(
        actual,
        std::fs::read(corpus().join("objects/sha256").join(source)).unwrap()
    );
    assert_eq!(actual[..2], [0x1f, 0x8b]);
}

struct CancelRead {
    canceled: Arc<AtomicBool>,
}
impl Read for CancelRead {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        assert!(bytes.len() <= 128 * 1024);
        self.canceled.store(true, Ordering::Release);
        bytes[0] = b'x';
        Ok(1)
    }
}
#[test]
fn midstream_cancellation_releases_owned_staging() {
    let canceled = Arc::new(AtomicBool::new(false));
    let stage = tempfile::tempdir().unwrap();
    let result = wire::PreparedCollection::prepare(
        &fixture().encode().unwrap(),
        stage.path(),
        |_| {
            Ok(Box::new(CancelRead {
                canceled: canceled.clone(),
            }))
        },
        &canceled,
    );
    assert!(matches!(result, Err(wire::Error::Canceled)));
    assert_eq!(std::fs::read_dir(stage.path()).unwrap().count(), 0);
}
