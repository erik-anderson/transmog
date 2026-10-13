//! Real TLS peers verify the ALPS offer, settings application and wire ACKs.
mod support;
use boring::ssl::{AlpnError, ExtensionType, Ssl, SslAcceptor, SslMethod, select_next_proto};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
};
use transmog_core::{
    BodyFrame, CanonicalRequest, HeaderField, network_profile::NetworkProfileSnapshot,
};
use transmog_http::{HyperEgressMode, HyperOriginClient};
use transmog_tls::{
    EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot, UpstreamTlsContextFactory,
    UpstreamTlsPolicy,
};

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
type Offer = Arc<Mutex<Option<Vec<u8>>>>;

async fn fixture(enabled: bool) -> (HyperOriginClient, TcpListener, SslAcceptor, Offer) {
    let ca = ProxyCa::generate("ALPS origin root", 2).unwrap();
    let leaf = ca
        .issue(EndpointIdentity::parse("localhost").unwrap(), 1)
        .unwrap();
    let trust = PemTrustSource::from_pem(&ca.certificate_pem().unwrap(), "ALPS fixture").unwrap();
    let mut profile = NetworkProfileSnapshot::default();
    profile.settings.http2_alps = enabled;
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
    .with_network_profile(profile)
    .unwrap();
    let client = HyperOriginClient::new(&factory).unwrap();
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    acceptor.set_certificate(&leaf.certificate).unwrap();
    acceptor.set_private_key(&leaf.private_key).unwrap();
    acceptor.set_alpn_select_callback(|_, protocols| {
        select_next_proto(b"\x02h2", protocols).ok_or(AlpnError::NOACK)
    });
    let offer = Offer::default();
    let recorder = offer.clone();
    acceptor.set_select_certificate_callback(move |hello| {
        *recorder.lock().unwrap() = hello
            .get_extension(ExtensionType::from(0x44cd))
            .map(<[u8]>::to_vec);
        Ok(())
    });
    (
        client,
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        acceptor.build(),
        offer,
    )
}

fn frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    result.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes()[1..]);
    result.extend_from_slice(&[kind, flags]);
    result.extend_from_slice(&stream.to_be_bytes());
    result.extend_from_slice(payload);
    result
}
fn settings(parameters: &[(u16, u32)]) -> Vec<u8> {
    let mut payload = Vec::new();
    for &(id, value) in parameters {
        payload.extend_from_slice(&id.to_be_bytes());
        payload.extend_from_slice(&value.to_be_bytes());
    }
    frame(4, 0, 0, &payload)
}
async fn read_frame(io: &mut (impl AsyncRead + Unpin)) -> (u8, u8, u32, Vec<u8>) {
    let mut header = [0; 9];
    io.read_exact(&mut header).await.unwrap();
    let length =
        usize::from(header[0]) << 16 | usize::from(header[1]) << 8 | usize::from(header[2]);
    assert!(length <= 65_536);
    let mut payload = vec![0; length];
    io.read_exact(&mut payload).await.unwrap();
    (
        header[3],
        header[4],
        u32::from_be_bytes(header[5..].try_into().unwrap()),
        payload,
    )
}
async fn preface(io: &mut (impl AsyncRead + Unpin)) {
    let mut preface = [0; 24];
    io.read_exact(&mut preface).await.unwrap();
    assert_eq!(preface, PREFACE);
    let (kind, flags, stream, _) = read_frame(io).await;
    assert_eq!((kind, flags, stream), (4, 0, 0));
}
async fn headers_without_alps_ack(io: &mut (impl AsyncRead + Unpin)) -> (u32, Vec<u8>) {
    loop {
        let (kind, flags, stream, payload) = read_frame(io).await;
        if kind == 1 {
            return (stream, payload);
        }
        assert_ne!(
            (kind, flags),
            (4, 1),
            "TLS-delivered settings were ACKed on the wire"
        );
        assert_eq!(kind, 8, "unexpected frame before request headers");
    }
}
async fn respond(io: &mut (impl AsyncWrite + Unpin), stream: u32) {
    io.write_all(&frame(1, 5, stream, &[0x88])).await.unwrap(); // Indexed :status 200, END_HEADERS | END_STREAM.
    io.flush().await.unwrap();
}
async fn execute(
    client: &HyperOriginClient,
    request: CanonicalRequest,
) -> Result<(), transmog_http::HyperOriginError> {
    client
        .execute(
            request,
            HyperEgressMode::Http2Only,
            1024,
            Duration::from_secs(3),
        )
        .await
        .map(|response| assert_eq!(response.head.status, 200))
}

#[tokio::test]
async fn alps_applies_hpack_and_concurrency_before_requests_without_a_wire_settings_frame() {
    let (client, listener, acceptor, offer) = fixture(true).await;
    let port = listener.local_addr().unwrap().port();
    let origin = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ssl = Ssl::new(acceptor.context()).unwrap();
        let mut alps = settings(&[(1, 0), (3, 1), (8, 1), (0x2a2a, 7)]);
        alps.extend(frame(0x89, 0, 0, &[0, 0, 0, 0])); // Optional Client Hint preferences are discarded.
        transmog_tls::configure_h2_application_settings(&mut ssl, &alps).unwrap();
        let mut io = tokio_boring::SslStreamBuilder::new(ssl, tcp)
            .accept()
            .await
            .unwrap();
        assert_eq!(
            transmog_tls::peer_application_settings(io.ssl()),
            Some(&[][..])
        );
        preface(&mut io).await;
        let (first, encoded) = headers_without_alps_ack(&mut io).await;
        assert_eq!(
            encoded[0], 0x20,
            "HPACK table size was not set to zero before first headers"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), read_frame(&mut io))
                .await
                .is_err(),
            "a second request exceeded ALPS concurrency"
        );
        respond(&mut io, first).await;
        let (second, _) = headers_without_alps_ack(&mut io).await;
        assert!(second > first);
        respond(&mut io, second).await;
    });
    let request = support::request(port);
    let second = request.clone();
    let (left, right) = tokio::join!(execute(&client, request), execute(&client, second));
    left.unwrap();
    right.unwrap();
    origin.await.unwrap();
    assert_eq!(*offer.lock().unwrap(), Some(vec![0, 3, 2, b'h', b'2']));
}

#[tokio::test]
async fn alps_zero_window_blocks_data_and_later_wire_settings_are_acknowledged() {
    let (client, listener, acceptor, _) = fixture(true).await;
    let port = listener.local_addr().unwrap().port();
    let origin = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ssl = Ssl::new(acceptor.context()).unwrap();
        transmog_tls::configure_h2_application_settings(&mut ssl, &settings(&[(4, 0)])).unwrap();
        let mut io = tokio_boring::SslStreamBuilder::new(ssl, tcp)
            .accept()
            .await
            .unwrap();
        preface(&mut io).await;
        let (stream, _) = headers_without_alps_ack(&mut io).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), read_frame(&mut io))
                .await
                .is_err(),
            "DATA bypassed the ALPS zero window"
        );
        io.write_all(&settings(&[(4, 3)])).await.unwrap();
        let mut acknowledged = false;
        loop {
            let (kind, flags, id, payload) = read_frame(&mut io).await;
            match kind {
                4 => {
                    assert_eq!((flags, id, payload.len()), (1, 0, 0));
                    assert!(!acknowledged);
                    acknowledged = true;
                }
                0 => {
                    assert_eq!(id, stream);
                    assert_eq!(payload, [1, 2, 3]);
                    break;
                }
                _ => panic!("unexpected frame after window update: {kind}"),
            }
        }
        assert!(acknowledged);
        respond(&mut io, stream).await;
    });
    let mut request = support::request(port);
    request.head.method = "POST".into();
    request.body = vec![BodyFrame::Data(vec![1, 2, 3].into())];
    execute(&client, request).await.unwrap();
    origin.await.unwrap();
}

#[tokio::test]
async fn alps_toggle_and_non_alps_origin_keep_normal_http2_working() {
    for (enabled, server_alps) in [(false, true), (true, false), (true, true)] {
        let (client, listener, acceptor, offer) = fixture(enabled).await;
        let port = listener.local_addr().unwrap().port();
        let origin = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ssl = Ssl::new(acceptor.context()).unwrap();
            if server_alps {
                let origin = format!("https://localhost:{port}");
                let hint = b"Sec-CH-UA-Platform";
                let mut preferences = u16::try_from(origin.len()).unwrap().to_be_bytes().to_vec();
                preferences.extend_from_slice(origin.as_bytes());
                preferences.extend_from_slice(&u16::try_from(hint.len()).unwrap().to_be_bytes());
                preferences.extend_from_slice(hint);
                transmog_tls::configure_h2_application_settings(
                    &mut ssl,
                    &frame(0x89, 0, 0, &preferences),
                )
                .unwrap();
            }
            let io = tokio_boring::SslStreamBuilder::new(ssl, tcp)
                .accept()
                .await
                .unwrap();
            assert_eq!(
                transmog_tls::peer_application_settings(io.ssl()).is_some(),
                enabled && server_alps
            );
            let mut connection = h2::server::handshake(io).await.unwrap();
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.headers()["sec-ch-ua"], "caller-value");
            assert!(!request.headers().contains_key("sec-ch-ua-platform"));
            respond
                .send_response(http::Response::new(()), true)
                .unwrap();
            // Drive the queued response onto the wire before dropping the peer.
            let _ = tokio::time::timeout(Duration::from_millis(100), connection.accept()).await;
        });
        let mut request = support::request(port);
        request
            .head
            .headers
            .push(HeaderField::try_new(b"sec-ch-ua".to_vec(), b"caller-value".to_vec()).unwrap());
        execute(&client, request).await.unwrap();
        origin.await.unwrap();
        assert_eq!(offer.lock().unwrap().is_some(), enabled);
    }
}

#[tokio::test]
async fn invalid_alps_is_rejected_before_any_http_bytes_are_sent() {
    for alps in [
        settings(&[(4, 0x8000_0000)]),
        settings(&[(5, 1)]),
        frame(4, 1, 0, &[]),
        frame(0, 0, 1, &[]),
        vec![0],
    ] {
        let (client, listener, acceptor, _) = fixture(true).await;
        let port = listener.local_addr().unwrap().port();
        let origin = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ssl = Ssl::new(acceptor.context()).unwrap();
            transmog_tls::configure_h2_application_settings(&mut ssl, &alps).unwrap();
            let mut io = tokio_boring::SslStreamBuilder::new(ssl, tcp)
                .accept()
                .await
                .unwrap();
            let mut byte = [0];
            assert_eq!(io.read(&mut byte).await.unwrap_or(0), 0);
        });
        assert!(execute(&client, support::request(port)).await.is_err());
        origin.await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the external Google origin; run explicitly to qualify ALPS interoperability"]
async fn google_negotiates_alps_and_serves_a_verified_http2_request() {
    use transmog_core::{HttpLegVersion, performance::PerformanceRecorder};
    use transmog_tls::SystemTrustSource;
    let recorder =
        PerformanceRecorder::new(std::time::SystemTime::now(), std::time::Instant::now());
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
    .with_network_profile(NetworkProfileSnapshot::default())
    .unwrap();
    let client = HyperOriginClient::new(&factory)
        .unwrap()
        .with_performance(Some(recorder.clone()));
    let mut request = support::request(443);
    request.head.method = "HEAD".into();
    request.head.target.host = "www.google.com".into();
    request.head.target.authority = "www.google.com".into();
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        client.execute(
            request,
            HyperEgressMode::Http2Only,
            4096,
            Duration::from_secs(10),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.head.source_version, HttpLegVersion::Http2);
    assert_eq!(response.head.status, 200);
    let evidence = recorder.snapshot();
    let tls = evidence
        .transports
        .iter()
        .find_map(|connection| connection.tls.as_ref())
        .unwrap();
    assert_eq!(
        tls.verification,
        Some(transmog_core::VerificationResult::Verified)
    );
    println!("Google TLS evidence: {:?}", tls.details);
    assert!(
        tls.details
            .get("HTTP/2 ALPS")
            .is_some_and(|detail| detail.starts_with("Negotiated"))
    );
}

#[tokio::test]
#[ignore = "requires the isolated Docker Caddy origin; run through scripts/test-interop.py"]
async fn docker_caddy_uses_normal_settings_when_it_declines_alps() {
    use transmog_core::performance::PerformanceRecorder;
    let uri: http::Uri = std::env::var("TRANSMOG_CADDY_URL")
        .unwrap()
        .parse()
        .unwrap();
    let ca = std::fs::read(std::env::var("TRANSMOG_TEST_CA_CERT").unwrap()).unwrap();
    let trust = PemTrustSource::from_pem(&ca, "Docker Caddy origin root").unwrap();
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1).unwrap()),
        UpstreamTlsPolicy::default(),
    )
    .with_network_profile(NetworkProfileSnapshot::default())
    .unwrap();
    let recorder =
        PerformanceRecorder::new(std::time::SystemTime::now(), std::time::Instant::now());
    let client = HyperOriginClient::new(&factory)
        .unwrap()
        .with_performance(Some(recorder.clone()));
    let mut request = support::request(uri.port_u16().unwrap());
    request.head.method = "HEAD".into();
    request.head.target.host = uri.host().unwrap().into();
    request.head.target.authority = uri.authority().unwrap().as_str().into();
    execute(&client, request).await.unwrap();
    let evidence = recorder.snapshot();
    let tls = evidence
        .transports
        .iter()
        .find_map(|connection| connection.tls.as_ref())
        .unwrap();
    assert_eq!(
        tls.verification,
        Some(transmog_core::VerificationResult::Verified)
    );
    assert_eq!(
        tls.details.get("HTTP/2 ALPS").map(String::as_str),
        Some("Not negotiated")
    );
    println!("Docker Caddy: verified H2; ALPS not negotiated");
}
