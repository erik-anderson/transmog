use super::*;
use crate::{
    CrlSet, CtConfig, CtLog, IssuerRevocations, LogOperator, RootConstraint, SecurityPolicy,
    SubjectLimit,
};
use boring::sign::Signer;

const START: i64 = 1_773_532_800;
fn key() -> PKey<Private> {
    PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap()
}
fn policy(root: &X509, logs: Vec<CtLog>, public: bool) -> CertificatePolicy {
    let root = root.to_der().unwrap();
    CertificatePolicy {
        anchors: Arc::new([root.clone()].into()),
        anchor_only: Arc::new([root.clone()].into()),
        known_public_roots: Arc::new(if public {
            [root].into()
        } else {
            std::collections::HashSet::default()
        }),
        security: Some(Arc::new(SecurityPolicy {
            ct: CtConfig {
                disabled: false,
                timestamp_ms: START * 1000,
                require_rfc6962: false,
                logs,
            },
            crl: CrlSet::default(),
        })),
        ..Default::default()
    }
}
fn log(key: &PKey<Private>, operator: &str) -> CtLog {
    let public = key.public_key_to_der().unwrap();
    CtLog {
        id: boring::sha::sha256(&public),
        key: public,
        name: "fixture".into(),
        log_type: 1,
        retired_at_ms: None,
        operators: vec![LogOperator {
            name: operator.into(),
            start_ms: 0,
        }],
    }
}
fn vector(data: &[u8]) -> Vec<u8> {
    [
        u16::try_from(data.len()).unwrap().to_be_bytes().as_slice(),
        data,
    ]
    .concat()
}
fn entry(leaf: &[u8]) -> Vec<u8> {
    let length = u32::try_from(leaf.len()).unwrap().to_be_bytes();
    [&[0, 0], &length[1..], leaf].concat()
}
fn sct(key: &PKey<Private>, timestamp: i64, entry: &[u8], extensions: &[u8]) -> Vec<u8> {
    let time = timestamp.to_be_bytes();
    let message = [&[0, 0], time.as_slice(), entry, &vector(extensions)].concat();
    let mut signer = Signer::new(MessageDigest::sha256(), key).unwrap();
    signer.update(&message).unwrap();
    let signature = signer.sign_to_vec().unwrap();
    [
        &[0],
        boring::sha::sha256(&key.public_key_to_der().unwrap()).as_slice(),
        time.as_slice(),
        &vector(extensions),
        &[4, 3],
        &vector(&signature),
    ]
    .concat()
}
fn list(scts: &[Vec<u8>]) -> Vec<u8> {
    vector(&scts.iter().flat_map(|sct| vector(sct)).collect::<Vec<_>>())
}
fn check(
    policy: &CertificatePolicy,
    leaf: &[u8],
    now: i64,
    scts: &[u8],
    ocsp: &[u8],
) -> Verification {
    let policy = Arc::new(policy.clone());
    verify_time(
        &compile(&policy).unwrap(),
        &policy,
        &[leaf.to_vec()],
        false,
        now,
        scts,
        ocsp,
    )
}
#[test]
fn ct_signatures_operator_diversity_future_grace_and_static_leaf_index() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let leaf = certificate(&issuer, Some(&root), START, START + 86400)
        .to_der()
        .unwrap();
    let a = key();
    let b = key();
    let mut p = policy(&root, vec![log(&a, "A"), log(&b, "B")], true);
    let signed = |time| {
        list(&[
            sct(&a, time, &entry(&leaf), &[]),
            sct(&b, time, &entry(&leaf), &[]),
        ])
    };
    assert!(!check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted);
    let good = check(&p, &leaf, START * 1000 + 1000, &signed(START * 1000), &[]);
    assert!(good.accepted, "{}", good.diagnostic);
    assert_eq!(good.security.ct, "Compliant");
    assert_eq!(good.security.valid_scts, 2);
    assert!(
        check(
            &p,
            &leaf,
            START * 1000 + 1000,
            &signed(START * 1000 + 61_000),
            &[]
        )
        .accepted
    );
    assert!(
        !check(
            &p,
            &leaf,
            START * 1000 + 1000,
            &signed(START * 1000 + 61_001),
            &[]
        )
        .accepted
    );
    let mut corrupt = signed(START * 1000);
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(!check(&p, &leaf, START * 1000 + 1000, &corrupt, &[]).accepted);
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.ct.logs[1].operators[0].name = "A".into();
    assert!(!check(&p, &leaf, START * 1000 + 1000, &signed(START * 1000), &[]).accepted);
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.ct.logs[1].operators[0].name = "B".into();
    data.ct.logs[1].log_type = 2;
    assert!(!check(&p, &leaf, START * 1000 + 1000, &signed(START * 1000), &[]).accepted);
    let static_scts = list(&[
        sct(&a, START * 1000, &entry(&leaf), &[]),
        sct(&b, START * 1000, &entry(&leaf), &[0, 0, 5, 0, 0, 0, 0, 1]),
    ]);
    assert!(check(&p, &leaf, START * 1000 + 1000, &static_scts, &[]).accepted);
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.ct.logs[1].log_type = 1;
    data.ct.logs[1].operators = vec![
        LogOperator {
            name: "A".into(),
            start_ms: START * 1000 + 1,
        },
        LogOperator {
            name: "B".into(),
            start_ms: 0,
        },
    ];
    assert!(check(&p, &leaf, START * 1000 + 1000, &signed(START * 1000), &[]).accepted);
    assert!(
        !check(
            &p,
            &leaf,
            START * 1000 + 1000,
            &signed(START * 1000 + 1),
            &[]
        )
        .accepted
    );
}
#[test]
fn ct_private_disabled_and_exact_seventy_day_boundary() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let leaf = certificate(&issuer, Some(&root), START, START + 100 * 86400)
        .to_der()
        .unwrap();
    let mut p = policy(&root, vec![], false);
    let result = check(&p, &leaf, START * 1000, &[], &[]);
    assert!(result.accepted);
    assert_eq!(result.security.ct, "Not required for private trust");
    p.known_public_roots = Arc::new([root.to_der().unwrap()].into());
    p.root_constraints = Arc::new(
        [(
            root.to_der().unwrap(),
            vec![RootConstraint {
                version_allowed: true,
                sct_all_after: Some(START),
                ..Default::default()
            }],
        )]
        .into(),
    );
    assert!(!check(&p, &leaf, (START + 70 * 86400) * 1000 - 1, &[], &[]).accepted);
    let result = check(&p, &leaf, (START + 70 * 86400) * 1000, &[], &[]);
    assert!(result.accepted);
    assert_eq!(result.security.ct, "Not enforced: CT log list is stale");
    Arc::make_mut(p.security.as_mut().unwrap()).ct.disabled = true;
    let result = check(&p, &leaf, START * 1000, &[], &[]);
    assert!(result.accepted);
    assert_eq!(result.security.ct, "Disabled by component");
}

#[test]
fn recovery_defers_leaf_metadata_but_tls_retry_enforces_it() {
    let issuer = key();
    let root = certificate(&issuer, None, 1_577_836_800, 4_102_444_800); // 2020
    let root_der = root.to_der().unwrap();
    let leaf = certificate(&issuer, Some(&root), START, START + 86400)
        .to_der()
        .unwrap();
    let a = key();
    let b = key();
    let mut p = policy(&root, vec![log(&a, "A"), log(&b, "B")], true);
    let group = RootConstraint {
        version_allowed: true,
        validity_after: Some(START - 1),
        validity_not_after: Some(START),
        sct_all_after: Some(START),
        sct_not_after: Some(START + 1),
        permitted_dns_names: vec![".example.com".into()],
    };
    p.root_constraints = Arc::new([(root_der.clone(), vec![group.clone()])].into());
    let recovery = |p: &CertificatePolicy| {
        let p = Arc::new(p.clone());
        verify_time(
            &compile(&p).unwrap(),
            &p,
            std::slice::from_ref(&root_der),
            true,
            START * 1000 + 1000,
            &[],
            &[],
        )
    };
    let result = recovery(&p);
    assert!(result.accepted, "{}", result.diagnostic);
    assert_eq!(result.security.ct, "Not reached");
    let signed = |bytes: &[u8], timestamp| {
        list(&[
            sct(&a, timestamp, &entry(bytes), &[]),
            sct(&b, timestamp, &entry(bytes), &[]),
        ])
    };
    let result = check(
        &p,
        &leaf,
        START * 1000 + 1000,
        &signed(&leaf, START * 1000 + 500),
        &[],
    );
    assert!(result.accepted, "{}", result.diagnostic);
    assert!(!check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted);
    assert!(
        !check(
            &p,
            &leaf,
            START * 1000 + 1000,
            &signed(&leaf, START * 1000),
            &[]
        )
        .accepted,
        "The fresh TLS retry must enforce the SCT lower bound"
    );
    let older_leaf = certificate(&issuer, Some(&root), START - 1, START + 86400)
        .to_der()
        .unwrap();
    assert!(
        !check(
            &p,
            &older_leaf,
            START * 1000 + 1000,
            &signed(&older_leaf, START * 1000 + 500),
            &[]
        )
        .accepted,
        "The fresh TLS retry must enforce the issuance lower bound"
    );
    p.root_constraints = Arc::new(
        [(
            root_der.clone(),
            vec![RootConstraint {
                version_allowed: false,
                ..group.clone()
            }],
        )]
        .into(),
    );
    assert!(
        !recovery(&p).accepted,
        "Root version limits still govern recovery"
    );
    p.root_constraints = Arc::new([(root_der.clone(), vec![group])].into());
    let mut leaf_only = p.clone();
    leaf_only.anchors = Arc::default();
    leaf_only.trusted_leaves = Arc::new(vec![root]);
    assert!(
        !recovery(&leaf_only).accepted,
        "An exact TrustedPeople leaf must not become an issuer"
    );
    Arc::make_mut(p.security.as_mut().unwrap())
        .crl
        .blocked_spkis
        .push(boring::sha::sha256(&issuer.public_key_to_der().unwrap()));
    assert!(!recovery(&p).accepted, "Revocation still governs recovery");
}
pub(super) fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut result = vec![tag];
    if body.len() < 128 {
        result.push(u8::try_from(body.len()).unwrap());
    } else {
        let length = body.len().to_be_bytes();
        let first = length.iter().position(|b| *b != 0).unwrap();
        result.push(0x80 | u8::try_from(length.len() - first).unwrap());
        result.extend(&length[first..]);
    }
    result.extend(body);
    result
}
fn element(input: &[u8]) -> (&[u8], &[u8]) {
    let (header, length) = if input[1] & 0x80 == 0 {
        (2, usize::from(input[1]))
    } else {
        let count = usize::from(input[1] & 0x7f);
        (
            2 + count,
            input[2..2 + count]
                .iter()
                .fold(0, |n, b| n * 256 + usize::from(*b)),
        )
    };
    (&input[..header + length], &input[header..header + length])
}
#[test]
fn embedded_ct_quorum_retirement_and_root_sct_constraints() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let before = certificate(&issuer, Some(&root), START, START + 86400)
        .to_der()
        .unwrap();
    let (tbs, _) = element(element(&before).1);
    let length = u32::try_from(tbs.len()).unwrap().to_be_bytes();
    let precert = [
        &[0, 1],
        boring::sha::sha256(&issuer.public_key_to_der().unwrap()).as_slice(),
        &length[1..],
        tbs,
    ]
    .concat();
    let a = key();
    let b = key();
    let time = START * 1000 + 500;
    let scts = list(&[sct(&a, time, &precert, &[]), sct(&b, time, &precert, &[])]);
    let leaf = certificate_extra(&issuer, Some(&root), START, START + 86400, Some(&scts))
        .to_der()
        .unwrap();
    let mut p = policy(&root, vec![log(&a, "A"), log(&b, "B")], true);
    assert!(check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted);
    Arc::make_mut(p.security.as_mut().unwrap()).ct.logs[1].retired_at_ms = Some(START * 1000 + 750);
    assert!(
        check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted,
        "Retired embedded SCT issued before retirement should count"
    );
    p.root_constraints = Arc::new(
        [(
            root.to_der().unwrap(),
            vec![RootConstraint {
                version_allowed: true,
                sct_not_after: Some(START),
                ..Default::default()
            }],
        )]
        .into(),
    );
    assert!(
        !check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted,
        "Do not round away SCT milliseconds"
    );
    p.root_constraints = Arc::new(
        [(
            root.to_der().unwrap(),
            vec![RootConstraint {
                version_allowed: true,
                sct_not_after: Some(START + 1),
                sct_all_after: Some(START),
                ..Default::default()
            }],
        )]
        .into(),
    );
    assert!(check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted);
    Arc::make_mut(p.security.as_mut().unwrap()).ct.logs[0].retired_at_ms = Some(START * 1000 + 750);
    assert!(
        !check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted,
        "Retired logs cannot satisfy root SCT constraints"
    );
    Arc::make_mut(p.security.as_mut().unwrap()).ct.disabled = true;
    assert!(
        check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted,
        "Disable switch bypasses SCT metadata rules"
    );
}

#[test]
fn embedded_ct_lifetime_quorum_deduplicates_log_ids() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let a = key();
    let b = key();
    let c = key();
    let p = policy(&root, vec![log(&a, "A"), log(&b, "B"), log(&c, "C")], true);
    for days in [180, 181] {
        let expiry = START + days * 86400;
        let before = certificate(&issuer, Some(&root), START, expiry)
            .to_der()
            .unwrap();
        let (tbs, _) = element(element(&before).1);
        let length = u32::try_from(tbs.len()).unwrap().to_be_bytes();
        let precert = [
            &[0, 1],
            boring::sha::sha256(&issuer.public_key_to_der().unwrap()).as_slice(),
            &length[1..],
            tbs,
        ]
        .concat();
        let first = sct(&a, START * 1000, &precert, &[]);
        let second = sct(&b, START * 1000, &precert, &[]);
        let repeated = list(&[first.clone(), first.clone(), second.clone()]);
        let leaf = certificate_extra(&issuer, Some(&root), START, expiry, Some(&repeated))
            .to_der()
            .unwrap();
        assert_eq!(
            check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted,
            days == 180
        );
        let diverse = list(&[first, second, sct(&c, START * 1000, &precert, &[])]);
        let leaf = certificate_extra(&issuer, Some(&root), START, expiry, Some(&diverse))
            .to_der()
            .unwrap();
        assert!(check(&p, &leaf, START * 1000 + 1000, &[], &[]).accepted);
    }
}
#[test]
fn crlset_serial_subject_root_keys_expiry_and_interception() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let leaf = certificate(&issuer, Some(&root), START, START + 86400);
    let bytes = leaf.to_der().unwrap();
    let hash = boring::sha::sha256(&issuer.public_key_to_der().unwrap());
    let mut p = policy(&root, vec![], false);
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.crl.issuers.push(IssuerRevocations {
        issuer: hash,
        serials: vec![],
    });
    data.crl.interception_spkis.push(hash);
    let good = check(&p, &bytes, START * 1000, &[], &[]);
    assert!(good.accepted);
    assert_eq!(good.security.crl, "Leaf covered");
    assert!(good.security.known_interception);
    Arc::make_mut(p.security.as_mut().unwrap()).crl.not_after = u64::try_from(START - 1).unwrap();
    assert_eq!(
        check(&p, &bytes, START * 1000, &[], &[]).security.crl,
        "Inconclusive"
    );
    Arc::make_mut(p.security.as_mut().unwrap()).crl.issuers[0]
        .serials
        .push(vec![1]);
    let revoked = check(&p, &bytes, START * 1000, &[], &[]);
    assert!(!revoked.accepted);
    assert_eq!(revoked.security.crl, "Revoked");
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.crl.issuers.clear();
    data.crl.blocked_spkis.push(hash);
    let revoked = check(&p, &bytes, START * 1000, &[], &[]);
    assert!(!revoked.accepted);
    assert_eq!(revoked.depth, Some(1));
    let data = Arc::make_mut(p.security.as_mut().unwrap());
    data.crl.blocked_spkis.clear();
    data.crl.subjects.push(SubjectLimit {
        subject_hash: boring::sha::sha256(&leaf.subject_name().to_der().unwrap()),
        allowed_spkis: vec![],
    });
    assert!(!check(&p, &bytes, START * 1000, &[], &[]).accepted);
    Arc::make_mut(p.security.as_mut().unwrap()).crl.subjects[0]
        .allowed_spkis
        .push(hash);
    assert!(check(&p, &bytes, START * 1000, &[], &[]).accepted);
}

fn ocsp(issuer: &X509, key: &PKey<Private>, revoked: bool) -> Vec<u8> {
    let generalized = der(0x18, b"20260315000000Z");
    let name = issuer.subject_name().to_der().unwrap();
    let spki = key.public_key_to_der().unwrap();
    let body = element(&spki).1;
    let (algorithm, _) = element(body);
    let key_bits = element(&body[algorithm.len()..]).1;
    let cert_id = der(
        0x30,
        &[
            der(
                0x30,
                &[der(6, &[0x2b, 0x0e, 3, 2, 0x1a]), der(5, &[])].concat(),
            ),
            der(4, &boring::sha::sha1(&name)),
            der(4, &boring::sha::sha1(&key_bits[1..])),
            der(2, &[1]),
        ]
        .concat(),
    );
    let status = if revoked {
        der(0xa1, &generalized)
    } else {
        der(0x80, &[])
    };
    let single = der(
        0x30,
        &[
            cert_id,
            status,
            generalized.clone(),
            der(0xa0, &der(0x18, b"20260316000000Z")),
        ]
        .concat(),
    );
    let response = der(
        0x30,
        &[der(0xa1, &name), generalized, der(0x30, &single)].concat(),
    );
    let mut signer = Signer::new(MessageDigest::sha256(), key).unwrap();
    signer.update(&response).unwrap();
    let signature = signer.sign_to_vec().unwrap();
    let basic = der(
        0x30,
        &[
            response,
            der(0x30, &der(6, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 4, 3, 2])),
            der(3, &[vec![0], signature].concat()),
        ]
        .concat(),
    );
    der(
        0x30,
        &[
            der(0x0a, &[0]),
            der(
                0xa0,
                &der(
                    0x30,
                    &[der(6, &[0x2b, 6, 1, 5, 5, 7, 0x30, 1, 1]), der(4, &basic)].concat(),
                ),
            ),
        ]
        .concat(),
    )
}
#[test]
fn staple_revocation_is_enforced_even_when_online_checks_are_disabled() {
    let issuer = key();
    let root = certificate(&issuer, None, 1, 4_102_444_800);
    let leaf = certificate(&issuer, Some(&root), START, START + 86400)
        .to_der()
        .unwrap();
    let p = policy(&root, vec![], false);
    let good = check(
        &p,
        &leaf,
        (START + 30) * 1000,
        &[],
        &ocsp(&root, &issuer, false),
    );
    assert!(good.accepted, "{}", good.diagnostic);
    assert_eq!(good.security.ocsp, "Good");
    let bad = check(
        &p,
        &leaf,
        (START + 30) * 1000,
        &[],
        &ocsp(&root, &issuer, true),
    );
    assert!(!bad.accepted);
    assert_eq!(bad.security.ocsp, "Revoked");
    let unknown = check(&p, &leaf, (START + 30) * 1000, &[], b"invalid response");
    assert!(unknown.accepted);
    assert_eq!(unknown.security.ocsp, "Invalid or unknown (soft fail)");
    let stale = check(
        &p,
        &leaf,
        (START + 86400 - 1) * 1000,
        &[],
        &ocsp(&root, &issuer, false),
    );
    assert!(stale.accepted);
}
