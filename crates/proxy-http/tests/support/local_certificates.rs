//! Certificates for local trust role and constraint interoperability checks.
use boring::{
    asn1::Asn1Time,
    bn::BigNum,
    hash::MessageDigest,
    pkey::{PKey, Private},
    x509::{
        X509, X509NameBuilder,
        extension::{
            AuthorityKeyIdentifier, BasicConstraints, ExtendedKeyUsage, KeyUsage,
            SubjectAlternativeName, SubjectKeyIdentifier,
        },
    },
};
use transmog_tls::ProxyCa;

pub fn certificate(
    name: &str,
    ca: bool,
    expired: bool,
    issuer: Option<&ProxyCa>,
    path_length: Option<u32>,
    issuer_name: Option<&str>,
) -> (X509, PKey<Private>) {
    let key = super::key();
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", name).unwrap();
    let subject = subject.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_serial_number(&BigNum::from_u32(331).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    cert.set_subject_name(&subject).unwrap();
    if let Some(name) = issuer_name {
        let mut issuer = X509NameBuilder::new().unwrap();
        issuer.append_entry_by_text("CN", name).unwrap();
        cert.set_issuer_name(&issuer.build()).unwrap();
    } else if let Some(issuer) = issuer {
        cert.set_issuer_name(issuer.certificate().subject_name())
            .unwrap();
    } else {
        cert.set_issuer_name(&subject).unwrap();
    }
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::from_unix(1).unwrap())
        .unwrap();
    let not_after = if expired {
        Asn1Time::from_unix(2).unwrap()
    } else {
        Asn1Time::days_from_now(1).unwrap()
    };
    cert.set_not_after(&not_after).unwrap();
    let mut constraints = BasicConstraints::new();
    constraints.critical();
    if ca {
        constraints.ca();
    }
    if let Some(length) = path_length {
        constraints.pathlen(length);
    }
    cert.append_extension(&constraints.build().unwrap())
        .unwrap();
    let mut usage = KeyUsage::new();
    usage.digital_signature();
    if ca {
        usage.key_cert_sign();
    }
    cert.append_extension(&usage.build().unwrap()).unwrap();
    cert.append_extension(&ExtendedKeyUsage::new().server_auth().build().unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .dns(name)
        .build(&cert.x509v3_context(None, None))
        .unwrap();
    cert.append_extension(&san).unwrap();
    cert.append_extension(
        &SubjectKeyIdentifier::new()
            .build(&cert.x509v3_context(None, None))
            .unwrap(),
    )
    .unwrap();
    if let Some(issuer) = issuer {
        cert.append_extension(
            &AuthorityKeyIdentifier::new()
                .keyid(false)
                .build(&cert.x509v3_context(Some(issuer.certificate()), None))
                .unwrap(),
        )
        .unwrap();
    }
    let signer = issuer.map_or_else(
        || key.clone(),
        |issuer| PKey::private_key_from_pem(&issuer.private_key_pem_pkcs8().unwrap()).unwrap(),
    );
    cert.sign(&signer, MessageDigest::sha256()).unwrap();
    (cert.build(), key)
}
