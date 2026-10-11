//! Opt-in real desktop acceptance peers. Never compiled into ordinary packages.
#[path = "../../../crates/proxy-h3/tests/support/mod.rs"]
mod quic;
#[path = "../../../crates/proxy-http/tests/support/mod.rs"]
mod support;
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};
use transmog_tls::{
    PemTrustSource, TrustSnapshot, UpstreamTlsContextFactory, UpstreamTlsPolicy,
    client_authentication::ClientCredentialSource,
    windows_client_certificates::WindowsCurrentUserCertificateSource,
};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Info {
    pub authorization_generation: String,
    pub identity: String,
    pub thumbprint: String,
    pub issuer_thumbprint: String,
    pub http1: u16,
    pub http2: u16,
    pub http3: u16,
}
pub(super) struct Fixture {
    _http1: support::Origin,
    _http2: support::Origin,
    _http3: quic::Origin,
    pub trust: Arc<TrustSnapshot>,
    pub info: Info,
    key: support::windows::WindowsFixture,
}
impl Fixture {
    pub async fn create(state_root: &Path) -> Self {
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts")
            .canonicalize()
            .unwrap();
        std::fs::create_dir_all(state_root).unwrap();
        let canonical = state_root.canonicalize().unwrap();
        assert!(
            canonical.starts_with(artifacts),
            "certificate acceptance needs a harness-owned artifacts profile"
        );
        let key = tokio::task::spawn_blocking(|| support::windows::WindowsFixture::new("RSA"))
            .await
            .unwrap();
        let credential = WindowsCurrentUserCertificateSource
            .credentials()
            .await
            .unwrap()
            .into_iter()
            .find(|credential| credential.id == key.id)
            .unwrap();
        let mut fixture = support::Fixture::new();
        fixture.source = Arc::new(support::Source {
            credential,
            reads: Arc::new(AtomicUsize::new(0)),
        });
        let http1 = support::origin(&fixture, false, false, Duration::from_secs(30)).await;
        let http2 = support::origin(&fixture, true, false, Duration::from_secs(30)).await;
        let http3 = quic::origin(&fixture).await;
        let roots = PemTrustSource::from_pem(
            &fixture.ca.certificate_pem().unwrap(),
            "Owned desktop acceptance origin",
        )
        .unwrap();
        let trust = Arc::new(TrustSnapshot::load(&roots, 1).unwrap());
        let (thumbprint, issuer) = key.cleanup_thumbprints();
        let info = Info {
            authorization_generation: "1".into(),
            identity: key.id.clone(),
            thumbprint: thumbprint.into(),
            issuer_thumbprint: issuer.into(),
            http1: http1.port,
            http2: http2.port,
            http3: http3.port,
        };
        // Recovery metadata contains only exact owned public thumbprints. The
        // outer harness can clean up even if the test process is terminated.
        std::fs::write(
            state_root.join("certificate-test-fixture.json"),
            serde_json::to_vec(&info).unwrap(),
        )
        .unwrap();
        Self {
            _http1: http1,
            _http2: http2,
            _http3: http3,
            trust,
            info,
            key,
        }
    }
    pub fn factory(&self) -> UpstreamTlsContextFactory {
        UpstreamTlsContextFactory::new(self.trust.clone(), UpstreamTlsPolicy::default())
    }
    pub fn delete_key(&self) {
        self.key.change("DeleteKey");
    }
}
