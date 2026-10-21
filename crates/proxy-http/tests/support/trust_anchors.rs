//! Local public CA metadata and omitted-intermediate interoperability fixtures.
#![allow(dead_code)]
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    ec::{EcGroup, EcKey},
    hash::MessageDigest,
    nid::Nid,
    pkey::PKey,
    ssl::{SslAcceptor, SslMethod, SslVersion},
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, KeyUsage},
    },
};
use std::sync::Arc;
use transmog_tls::{
    PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    trust_anchors::{PkiMetadata, PkiMetadataProvider},
};

pub fn catalog(certificates: &[(&X509, u8)], version: u8) -> Arc<PkiMetadata> {
    Arc::new(
        PkiMetadata::decode(
            &catalog_bytes(certificates, version),
            "local public CA fixture".into(),
        )
        .unwrap(),
    )
}

pub fn catalog_bytes(certificates: &[(&X509, u8)], version: u8) -> Vec<u8> {
    catalog_bytes_with_flags(
        &certificates
            .iter()
            .map(|(cert, id)| (*cert, *id, false, false))
            .collect::<Vec<_>>(),
        version,
    )
}

pub fn catalog_bytes_with_flags(certificates: &[(&X509, u8, bool, bool)], version: u8) -> Vec<u8> {
    encode_catalog(
        &certificates
            .iter()
            .map(|(cert, id, expiry, constraints)| (*cert, Some(*id), *expiry, *constraints))
            .collect::<Vec<_>>(),
        version,
    )
}

pub fn catalog_bytes_without_ids(certificates: &[&X509], version: u8) -> Vec<u8> {
    encode_catalog(
        &certificates
            .iter()
            .map(|cert| (*cert, None, false, false))
            .collect::<Vec<_>>(),
        version,
    )
}

fn encode_catalog(certificates: &[(&X509, Option<u8>, bool, bool)], version: u8) -> Vec<u8> {
    fn varint(mut value: usize) -> Vec<u8> {
        let mut bytes = vec![];
        while value >= 128 {
            bytes.push(u8::try_from(value & 127).unwrap() | 128);
            value >>= 7;
        }
        bytes.push(u8::try_from(value).unwrap());
        bytes
    }
    let mut bytes = vec![0x10, version];
    for (cert, id, expiry, constraints) in certificates {
        let der = cert.to_der().unwrap();
        let mut anchor = vec![0x0a];
        anchor.extend(varint(der.len()));
        anchor.extend(der);
        if let Some(id) = id {
            anchor.extend([0x5a, 1, *id]);
        }
        anchor.extend([0x40, u8::from(*expiry), 0x48, u8::from(*constraints)]);
        bytes.push(0x0a);
        bytes.extend(varint(anchor.len()));
        bytes.extend(anchor);
    }
    bytes
}

pub fn factory(root: &ProxyCa, metadata: Arc<PkiMetadata>) -> UpstreamTlsContextFactory {
    let trust =
        PemTrustSource::from_pem(&root.certificate_pem().unwrap(), "configured test root").unwrap();
    UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
    .with_pki_metadata(Arc::new(PkiMetadataProvider::new(metadata, None)))
    .with_private_test_roots()
    .with_local_trust_monitor(transmog_tls::local_trust::LocalTrustMonitor::fixed(
        transmog_tls::local_trust::LocalCertificates::default(),
    ))
    .with_verification_data(Arc::new(
        transmog_tls::verification_data::VerificationDataProvider::new(None, None),
    ))
}

pub fn issuer_root(name: &str) -> ProxyCa {
    let (cert, key) =
        crate::support::local_certificates::certificate(name, true, false, None, None, None);
    ProxyCa::from_pem(
        &cert.to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap()
}

pub fn intermediate(root: &ProxyCa) -> ProxyCa {
    let key = PKey::from_ec_key(
        EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap(),
    )
    .unwrap();
    let root_key = PKey::private_key_from_pem(&root.private_key_pem_pkcs8().unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", "omitted intermediate")
        .unwrap();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(200).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&name.build()).unwrap();
    cert.set_issuer_name(root.certificate().subject_name())
        .unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    cert.append_extension(&BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    cert.append_extension(&KeyUsage::new().critical().key_cert_sign().build().unwrap())
        .unwrap();
    cert.sign(&root_key, MessageDigest::sha256()).unwrap();
    ProxyCa::from_pem(
        &cert.build().to_pem().unwrap(),
        &key.private_key_to_pem_pkcs8().unwrap(),
    )
    .unwrap()
}

pub fn acceptor(ca: &ProxyCa, matched_id: Option<u8>, available_id: Option<u8>) -> SslAcceptor {
    let leaf = ca
        .issue(
            transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
            1,
        )
        .unwrap();
    let mut context = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    context
        .set_min_proto_version(Some(SslVersion::TLS1_3))
        .unwrap();
    if let Some(id) = matched_id {
        transmog_tls_native::add_server_credential_for_trust_anchor(
            &mut context,
            &[leaf.certificate],
            &leaf.private_key,
            &[id],
        )
        .unwrap();
    } else {
        context.set_certificate(&leaf.certificate).unwrap();
        context.set_private_key(&leaf.private_key).unwrap();
    }
    if let Some(id) = available_id {
        transmog_tls_native::configure_available_trust_anchors(&mut context, &[1, id]).unwrap();
    }
    context.build()
}
