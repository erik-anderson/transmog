use super::*;
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::{PKey, Private},
    x509::{
        X509NameBuilder,
        extension::{BasicConstraints, KeyUsage, SubjectAlternativeName},
    },
};

#[derive(Debug)]
struct CachedOnly;
impl crate::IssuerSource for CachedOnly {
    fn start(
        &self,
        _: &[String],
        _: std::time::Duration,
        _: Arc<AtomicBool>,
    ) -> Box<dyn crate::IssuerRequest> {
        panic!("Missing peer cannot trigger AIA")
    }
    fn cached_issuers(&self) -> Vec<Vec<u8>> {
        panic!("Missing peer cannot substitute a cached certificate")
    }
}
#[test]
fn missing_peer_rejects_before_consulting_cached_or_network_issuers() {
    let policy = Arc::new(CertificatePolicy {
        issuer_source: Some(Arc::new(CachedOnly)),
        ..Default::default()
    });
    let result = verify(&compile(&policy).unwrap(), &policy, &[], false);
    assert!(!result.accepted);
    assert_eq!(result.path, [] as [std::vec::Vec<u8>; 0]);
    assert_eq!(result.diagnostic, "No peer certificate");
}

fn certificate(key: &PKey<Private>, issuer: Option<&X509>, start: i64, expiry: i64) -> X509 {
    certificate_extra(key, issuer, start, expiry, None)
}

fn certificate_extra(
    key: &PKey<Private>,
    issuer: Option<&X509>,
    start: i64,
    expiry: i64,
    scts: Option<&[u8]>,
) -> X509 {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", if issuer.is_some() { "leaf" } else { "root" })
        .unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(issuer.map_or(&name, |issuer| issuer.subject_name()))
        .unwrap();
    cert.set_pubkey(key).unwrap();
    cert.set_not_before(&Asn1Time::from_unix(start).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::from_unix(expiry).unwrap())
        .unwrap();
    let mut constraints = BasicConstraints::new();
    constraints.critical();
    if issuer.is_none() {
        constraints.ca();
    }
    cert.append_extension(&constraints.build().unwrap())
        .unwrap();
    let mut usage = KeyUsage::new();
    usage.digital_signature();
    if issuer.is_none() {
        usage.key_cert_sign();
    }
    cert.append_extension(&usage.build().unwrap()).unwrap();
    if issuer.is_some() {
        cert.append_extension(
            &SubjectAlternativeName::new()
                .dns("www.example.com")
                .build(&cert.x509v3_context(None, None))
                .unwrap(),
        )
        .unwrap();
    }
    if let Some(scts) = scts {
        let der = security::der(4, scts);
        let value = format!(
            "DER:{}",
            der.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        );
        cert.append_extension(
            &boring::x509::X509Extension::new(None, None, "1.3.6.1.4.1.11129.2.4.2", &value)
                .unwrap(),
        )
        .unwrap();
    }
    cert.sign(key, MessageDigest::sha256()).unwrap();
    cert.build()
}

#[path = "security_tests.rs"]
mod security;

#[test]
fn root_metadata_checks_each_group_and_preserves_local_override() {
    use crate::RootConstraint;
    let key = PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap();
    let root = certificate(&key, None, 1, 4_102_444_800);
    let root_der = root.to_der().unwrap();
    let start = 1_773_532_800;
    let leaf = certificate(&key, Some(&root), start, start + 86_400)
        .to_der()
        .unwrap();
    let group = RootConstraint {
        version_allowed: true,
        validity_not_after: Some(start),
        validity_after: Some(start - 1),
        permitted_dns_names: vec![".example.com".into()],
        ..Default::default()
    };
    let mut policy = CertificatePolicy {
        anchors: Arc::new([root_der.clone()].into()),
        anchor_only: Arc::new([root_der.clone()].into()),
        ignore_anchor_expiry: Arc::new([root_der.clone()].into()),
        root_constraints: Arc::new([(root_der.clone(), vec![group.clone()])].into()),
        ..Default::default()
    };
    let check = |policy: &CertificatePolicy| {
        let policy = Arc::new(policy.clone());
        verify_at(
            &compile(&policy).unwrap(),
            &policy,
            std::slice::from_ref(&leaf),
            false,
            start + 1,
        )
    };
    assert!(check(&policy).accepted);
    for bad in [
        RootConstraint {
            version_allowed: false,
            ..group.clone()
        },
        RootConstraint {
            validity_after: Some(start),
            ..group.clone()
        },
        RootConstraint {
            validity_not_after: Some(start - 1),
            ..group.clone()
        },
        RootConstraint {
            permitted_dns_names: vec![".other.com".into()],
            ..group.clone()
        },
        RootConstraint {
            validity_not_after: Some(i64::MAX),
            ..group.clone()
        },
    ] {
        policy.root_constraints = Arc::new([(root_der.clone(), vec![bad.clone()])].into());
        let failure = check(&policy);
        assert!(!failure.accepted, "{bad:?}");
        assert!(failure.diagnostic.contains("root metadata"));
        policy.root_constraints = Arc::new([(root_der.clone(), vec![bad, group.clone()])].into());
        assert!(check(&policy).accepted, "OR groups were evaluated as AND");
    }
    policy.root_constraints = Arc::new(
        [(
            root_der.clone(),
            vec![RootConstraint {
                version_allowed: false,
                ..group
            }],
        )]
        .into(),
    );
    policy.local_anchors = Arc::new([root_der].into());
    assert!(
        check(&policy).accepted,
        "Eligible local ROOT did not override metadata"
    );
}

#[test]
fn constrained_candidate_is_rejected_before_selecting_an_alternate_path() {
    let key = PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap();
    let rejected_root = certificate(&key, None, 1, 4_102_444_800).to_der().unwrap();
    let alternate_root = certificate(&key, None, 1, 4_134_067_200).to_der().unwrap();
    let start = 1_773_532_800;
    let issuer = X509::from_der(&rejected_root).unwrap();
    let leaf = certificate(&key, Some(&issuer), start, start + 86_400)
        .to_der()
        .unwrap();
    let mut policy = CertificatePolicy {
        anchors: Arc::new([rejected_root.clone()].into()),
        anchor_only: Arc::new([rejected_root.clone()].into()),
        ignore_anchor_expiry: Arc::new([rejected_root.clone(), alternate_root.clone()].into()),
        root_constraints: Arc::new(
            [(
                rejected_root.clone(),
                vec![crate::RootConstraint {
                    version_allowed: true,
                    permitted_dns_names: vec![".other.com".into()],
                    ..Default::default()
                }],
            )]
            .into(),
        ),
        ..Default::default()
    };
    let check = |policy: &CertificatePolicy| {
        let policy = Arc::new(policy.clone());
        verify_at(
            &compile(&policy).unwrap(),
            &policy,
            std::slice::from_ref(&leaf),
            false,
            start + 1,
        )
    };
    assert!(!check(&policy).accepted);
    policy.anchors = Arc::new([rejected_root.clone(), alternate_root.clone()].into());
    policy.anchor_only = policy.anchors.clone();
    let result = check(&policy);
    assert!(result.accepted, "{}", result.diagnostic);
    assert_eq!(result.path.last(), Some(&alternate_root));
}

#[test]
fn public_lifetime_boundaries_and_private_root_exemption_match_chromium() {
    let key = PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap();
    let root = certificate(&key, None, 1, 4_102_444_800);
    let der = root.to_der().unwrap();
    let mut policy = CertificatePolicy {
        anchors: Arc::new([der.clone()].into()),
        anchor_only: Arc::new([der.clone()].into()),
        known_public_roots: Arc::new([der.clone()].into()),
        ignore_anchor_expiry: Arc::new([der].into()),
        ..Default::default()
    };
    for (start, days) in [
        (1_770_000_000, 398),
        (1_773_532_800, 200),
        (1_805_068_800, 100),
        (1_868_227_200, 47),
    ] {
        for extra in [0, 1] {
            let leaf = certificate(&key, Some(&root), start, start + days * 86_400 + extra);
            let current = Arc::new(policy.clone());
            let result = verify_at(
                &compile(&current).unwrap(),
                &current,
                &[leaf.to_der().unwrap()],
                false,
                start + 1,
            );
            assert_eq!(
                result.accepted,
                extra == 0,
                "{start}: {}",
                result.diagnostic
            );
            assert!(result.known_public_root);
        }
    }
    policy.known_public_roots = Arc::default();
    let policy = Arc::new(policy);
    let start = 1_773_532_800;
    let leaf = certificate(&key, Some(&root), start, start + 20 * 365 * 86_400);
    let result = verify_at(
        &compile(&policy).unwrap(),
        &policy,
        &[leaf.to_der().unwrap()],
        false,
        start + 1,
    );
    assert!(result.accepted, "{}", result.diagnostic);
    assert!(!result.known_public_root);
}
