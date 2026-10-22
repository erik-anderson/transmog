use super::*;
fn config(timestamp: i64, compatibility: u64, disabled: bool) -> Vec<u8> {
    Config {
        disabled,
        list: Some(LogList {
            timestamp: Some(Timestamp {
                seconds: timestamp,
                nanos: 0,
            }),
            compatibility,
            logs: vec![],
        }),
    }
    .encode_to_vec()
}
fn crl(sequence: u32) -> Vec<u8> {
    let header =
        serde_json::json!({"ContentType":"CRLSet","Version":0,"Sequence":sequence}).to_string();
    [
        u16::try_from(header.len())
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        header.as_bytes(),
    ]
    .concat()
}
fn put(directory: &Path, version: &str, file: &str, data: &[u8]) {
    let path = directory.join(version);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join(file), data).unwrap();
}
#[test]
fn ct_baseline_compatibility_kill_switch_empty_list_and_allocation_bounds() {
    let ct = decode_ct(BASELINE).unwrap();
    assert!(ct.logs.len() > 20);
    assert!(!ct.require_rfc6962);
    assert!(decode_ct(&[]).is_err());
    assert!(decode_ct(&config(1, 5, false)).is_err());
    assert!(decode_ct(&config(1, 5, true)).unwrap().disabled);
    assert!(decode_ct(&config(1, 4, false)).unwrap().logs.is_empty());
    let repeated = [0x32, 0].repeat(1025);
    let mut list = vec![0x12];
    // Build a valid varint without relying on prost's internal allocation behavior.
    prost::encoding::encode_varint(repeated.len() as u64, &mut list);
    list.extend(repeated);
    assert!(decode_ct(&list).is_err());
    assert!(decode_ct(&vec![0; LIMIT + 1]).is_err());
}
#[test]
fn discovery_keeps_sources_independent_and_retains_last_good() {
    let pki = tempfile::tempdir().unwrap();
    let revocation = tempfile::tempdir().unwrap();
    let provider =
        VerificationDataProvider::new(Some(pki.path().into()), Some(revocation.path().into()));
    let before = provider.current();
    let timestamp = before.policy.ct.timestamp_ms / 1000;
    put(
        pki.path(),
        "2",
        "ct_config.pb",
        &config(timestamp - 1, 4, false),
    );
    put(revocation.path(), "99999", "crl-set", &crl(10));
    put(revocation.path(), "1.1", "crl-set", &crl(11));
    provider.refresh();
    let first = provider.current();
    assert_eq!(first.ct_sha256, before.ct_sha256);
    assert_eq!(first.policy.crl.sequence, 11);
    put(
        pki.path(),
        "3",
        "ct_config.pb",
        &config(timestamp + 1, 4, false),
    );
    provider.refresh();
    let fresh = provider.current();
    assert!(fresh.policy.ct.logs.is_empty());
    assert!(fresh.generation > first.generation);
    put(pki.path(), "3", "ct_config.pb", b"truncated");
    put(revocation.path(), "1.1", "crl-set", b"truncated");
    provider.refresh();
    assert!(Arc::ptr_eq(&provider.current(), &fresh));
    put(pki.path(), "4", "ct_config.pb", &config(0, 999, true));
    provider.refresh();
    assert!(provider.current().policy.ct.disabled);
    put(
        pki.path(),
        "5",
        "ct_config.pb",
        &config(timestamp, 4, false),
    );
    provider.refresh();
    assert!(!provider.current().policy.ct.disabled);
    assert_eq!(provider.current().policy.crl.sequence, 11);
    assert_eq!(before.policy.crl.sequence, 0);
}
#[test]
fn crl_headers_maps_and_truncated_serial_records() {
    let mut bytes = crl(100);
    bytes.extend([7; 32]);
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend([2, 0, 1]);
    let parsed = decode_crl(&bytes).unwrap();
    assert_eq!(parsed.issuers[0].serials, vec![vec![0, 1]]);
    bytes.pop();
    assert!(decode_crl(&bytes).is_err());
    let mut oversized = crl(1);
    oversized.extend([7; 32]);
    oversized.extend(1_000_001_u32.to_le_bytes());
    oversized.extend(vec![0; 1_000_001]);
    assert!(
        decode_crl(&oversized).is_err(),
        "Reject serial counts before allocation"
    );
    let hash = boring::base64::encode_block(&[3; 32]);
    let header=serde_json::json!({"ContentType":"CRLSet","Version":0,"Sequence":1,"BlockedInterceptionSPKIs":[hash.clone()],"LimitedSubjects":{(hash):[]}}).to_string();
    let bytes = [
        u16::try_from(header.len())
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        header.as_bytes(),
    ]
    .concat();
    let parsed = decode_crl(&bytes).unwrap();
    assert_eq!(parsed.blocked_spkis, vec![[3; 32]]);
    assert_eq!(parsed.interception_spkis, vec![[3; 32]]);
    assert_eq!(parsed.subjects[0].allowed_spkis, [] as [[u8; 32]; 0]);
    for invalid in [b"".as_slice(), b"\x31\x00{}", &[0; 10]] {
        assert!(decode_crl(invalid).is_err());
    }
}

#[test]
#[ignore = "read-only inspection of installed public browser components"]
fn installed_browser_component_data() {
    let base = std::env::var_os("LOCALAPPDATA").unwrap();
    for relative in [
        "Google/Chrome/User Data",
        "Microsoft/Windows/SharedWebView/EBWebView",
    ] {
        let base = PathBuf::from(&base).join(relative);
        let provider = VerificationDataProvider::new(
            Some(base.join("PKIMetadata")),
            Some(base.join("CertificateRevocation")),
        );
        for (_, path) in versions(&base.join("CertificateRevocation")) {
            let bytes = read(&path.join("crl-set")).unwrap();
            if let Err(error) = decode_crl(&bytes) {
                panic!("{}: {error}", path.display());
            }
        }
        provider.refresh();
        let data = provider.current();
        println!(
            "{}: CT {} {} logs; CRLSet {} {} issuers; source {}",
            relative,
            data.policy.ct.timestamp_ms,
            data.policy.ct.logs.len(),
            data.policy.crl.sequence,
            data.policy.crl.issuers.len(),
            data.crl_source
        );
        assert!(
            data.policy.crl.sequence > 0,
            "Installed CRLSet was not selected"
        );
    }
}
