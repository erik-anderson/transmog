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
        extension::{BasicConstraints, KeyUsage},
    },
};

fn certificate(key: &PKey<Private>, issuer: Option<&X509>, start: i64, expiry: i64) -> X509 {
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
    cert.sign(key, MessageDigest::sha256()).unwrap();
    cert.build()
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
