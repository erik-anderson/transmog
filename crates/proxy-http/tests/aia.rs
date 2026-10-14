//! Missing-issuer discovery never grants trust or bypasses certificate checks.
#[path = "support/aia.rs"]
mod aia;
#[path = "support/trust_anchors.rs"]
mod anchors;
mod support;
use std::{collections::HashMap, sync::Arc};
use transmog_tls::issuer_source::{HttpIssuerSource, IssuerSource};

#[tokio::test]
async fn missing_issuer_fetches_der_and_caches_only_verified_intermediate() {
    let root = aia::root("AIA root");
    let issuer = anchors::intermediate(&root);
    let origin = aia::issuer(&issuer.certificate().to_der().unwrap()).await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!("http://127.0.0.1:{}/issuer", origin.port)],
        "localhost",
    );
    let cache = Arc::new(HttpIssuerSource::default());
    let factory = aia::factory(&root).with_issuer_source(cache.clone());
    aia::probe(&factory, &leaf, &key, true).await;
    assert_eq!(
        cache.cached_issuers(),
        vec![issuer.certificate().to_der().unwrap()]
    );
    assert_eq!(origin.requests.lock().unwrap().len(), 1);
    drop(origin);
    // New origin/pool and no reachable AIA server: the cached candidate builds the path.
    aia::probe(&factory, &leaf, &key, true).await;
    let wrong_root = aia::root("Other AIA root");
    aia::probe(
        &aia::factory(&wrong_root).with_issuer_source(cache),
        &leaf,
        &key,
        false,
    )
    .await;
}

#[tokio::test]
async fn failed_hostname_does_not_populate_intermediate_cache() {
    let root = aia::root("AIA hostname root");
    let issuer = anchors::intermediate(&root);
    let origin = aia::issuer(&issuer.certificate().to_der().unwrap()).await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!("http://127.0.0.1:{}/issuer", origin.port)],
        "other.example",
    );
    let cache = Arc::new(HttpIssuerSource::default());
    aia::probe(
        &aia::factory(&root).with_issuer_source(cache.clone()),
        &leaf,
        &key,
        false,
    )
    .await;
    assert!(cache.cached_issuers().is_empty());
}

#[tokio::test]
async fn pem_first_block_and_http_redirect_work_without_caller_credentials() {
    let root = aia::root("AIA PEM root");
    let issuer = anchors::intermediate(&root);
    let mut pem = issuer.certificate().to_pem().unwrap();
    pem.extend(root.certificate().to_pem().unwrap());
    let origin = aia::responses(HashMap::from([
        (
            "/start".into(),
            b"HTTP/1.1 302 Found\r\nLocation: /issuer\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ),
        ("/issuer".into(), aia::ok(&pem)),
    ]))
    .await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!(
            "http://user:password@127.0.0.1:{}/start#secret",
            origin.port
        )],
        "localhost",
    );
    let cache = Arc::new(HttpIssuerSource::default());
    aia::probe(
        &aia::factory(&root).with_issuer_source(cache.clone()),
        &leaf,
        &key,
        true,
    )
    .await;
    assert_eq!(
        cache.cached_issuers(),
        vec![issuer.certificate().to_der().unwrap()]
    );
    let requests = origin.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let lower = request.to_ascii_lowercase();
        assert!(
            !lower.contains("authorization:")
                && !lower.contains("cookie:")
                && !lower.contains("referer:")
        );
        assert!(!lower.contains("secret") && !lower.contains("password"));
    }
}

#[tokio::test]
async fn oversized_response_untrusted_issuer_and_https_redirect_fail_closed() {
    let root = aia::root("AIA failure root");
    let issuer = anchors::intermediate(&root);
    let unrelated = aia::root("Untrusted AIA issuer");
    for body in [
        aia::ok(&vec![0; 65537]),
        aia::ok(&unrelated.certificate().to_der().unwrap()),
        b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1:1/issuer\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
    ] {
        let origin = aia::responses(HashMap::from([("/issuer".into(), body)])).await;
        let (leaf, key) = aia::leaf(
            &issuer,
            &[format!("http://127.0.0.1:{}/issuer", origin.port)],
            "localhost",
        );
        aia::probe(&aia::factory(&root), &leaf, &key, false).await;
    }
}

#[tokio::test]
async fn parallel_urls_skip_bad_results_but_do_not_fetch_sixth_url() {
    let root = aia::root("AIA URL limit root");
    let issuer = anchors::intermediate(&root);
    let origin = aia::responses(HashMap::from([
        ("/bad".into(), aia::ok(b"unusable issuer")),
        (
            "/issuer".into(),
            aia::ok(&issuer.certificate().to_der().unwrap()),
        ),
    ]))
    .await;
    let bad = format!("http://127.0.0.1:{}/bad", origin.port);
    let good = format!("http://127.0.0.1:{}/issuer", origin.port);
    let (leaf, key) = aia::leaf(&issuer, &[bad.clone(), good.clone()], "localhost");
    aia::probe(&aia::factory(&root), &leaf, &key, true).await;
    origin.requests.lock().unwrap().clear();
    let mut urls = vec![bad; 5];
    urls.push(good);
    let (leaf, key) = aia::leaf(&issuer, &urls, "localhost");
    aia::probe(&aia::factory(&root), &leaf, &key, false).await;
    let requests = origin.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(
        requests
            .iter()
            .all(|request| request.starts_with("GET /bad "))
    );
}

fn tlv(tag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut result = vec![tag];
    if bytes.len() < 128 {
        result.push(u8::try_from(bytes.len()).unwrap());
    } else {
        let len = u16::try_from(bytes.len()).unwrap();
        result.push(0x82);
        result.extend(len.to_be_bytes());
    }
    result.extend(bytes);
    result
}
#[tokio::test]
async fn certs_only_cms_is_an_untrusted_issuer_container() {
    let root = aia::root("AIA CMS root");
    let issuer = anchors::intermediate(&root);
    let oid = [
        0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01,
    ];
    let mut signed = vec![0x02, 0x01, 0x01, 0x31, 0x00];
    signed.extend(tlv(0x30, &oid));
    signed.extend(tlv(0xa0, &issuer.certificate().to_der().unwrap()));
    signed.extend([0x31, 0x00]);
    let mut content = oid;
    content[10] = 0x02;
    let mut outer = content.to_vec();
    outer.extend(tlv(0xa0, &tlv(0x30, &signed)));
    let origin = aia::issuer(&tlv(0x30, &outer)).await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!("http://127.0.0.1:{}/issuer", origin.port)],
        "localhost",
    );
    aia::probe(&aia::factory(&root), &leaf, &key, true).await;
}

#[tokio::test]
async fn pem_does_not_search_past_a_decodable_but_invalid_first_certificate() {
    let root = aia::root("AIA first PEM root");
    let issuer = anchors::intermediate(&root);
    let mut pem = b"-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----\n".to_vec();
    pem.extend(issuer.certificate().to_pem().unwrap());
    let origin = aia::issuer(&pem).await;
    let (leaf, key) = aia::leaf(
        &issuer,
        &[format!("http://127.0.0.1:{}/issuer", origin.port)],
        "localhost",
    );
    aia::probe(&aia::factory(&root), &leaf, &key, false).await;
}
