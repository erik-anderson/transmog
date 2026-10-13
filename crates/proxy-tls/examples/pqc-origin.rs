//! Loopback-only TLS fixture for real Chromium algorithm interoperability.
use boring::ssl::SslVersion;
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, SystemTime},
};
use transmog_tls::{
    CachedMitmCertificateResolver, DownstreamCertificateResolver, DownstreamTlsContextFactory,
    DownstreamTlsPolicy, EndpointIdentity, ProxyCa, TlsCapture,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 3 {
        return Err("expected PUBLIC_CA_PATH SIGNATURE GROUP_POLICY".into());
    }
    let ca = ProxyCa::generate("Transmog ephemeral PQC browser fixture", 1)?;
    std::fs::write(&arguments[0], ca.certificate_pem()?)?;
    let resolver = CachedMitmCertificateResolver::new(ca, 1, 1)?;
    let mut leaves =
        resolver.resolve_credentials(&EndpointIdentity::parse("localhost")?, SystemTime::now())?;
    let index = match arguments[1].as_str() {
        "automatic" => None,
        "mldsa44" => Some(0),
        "mldsa65" => Some(1),
        "mldsa87" => Some(2),
        "classical" => Some(3),
        _ => return Err("unsupported fixture signature".into()),
    };
    if let Some(index) = index {
        leaves = vec![leaves.remove(index)];
    }
    let capture = TlsCapture::new(transmog_core::TlsEvidence::default());
    let factory = DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
        minimum_version: SslVersion::TLS1_3,
        ..DownstreamTlsPolicy::default()
    });
    let acceptor = factory.acceptor_credentials_captured(&leaves, Some(&capture))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    println!("LISTEN_ADDR={}", listener.local_addr()?);
    std::io::stdout().flush()?;
    let (socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    socket.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut ssl = boring::ssl::Ssl::new(acceptor.context())?;
    // Include the fixture root on the legacy path too, so Chrome's narrowly
    // scoped test-root SPKI allowance can identify it without OS trust changes.
    for issuer in &leaves
        .last()
        .ok_or("missing fixture credential")?
        .issuer_chain
    {
        ssl.add_chain_cert(issuer)?;
    }
    match arguments[2].as_str() {
        "hybrid" => {}
        "classical" => ssl.set_curves_list("X25519:P-256:P-384")?,
        _ => return Err("unsupported fixture group policy".into()),
    }
    let mut tls = boring::ssl::SslStream::new(ssl, socket)?;
    tls.accept()?;
    let mut request = [0; 8192];
    let mut count = 0;
    while !request[..count]
        .windows(4)
        .any(|bytes| bytes == b"\r\n\r\n")
    {
        if count == request.len() {
            return Err("fixture request head exceeds its bound".into());
        }
        let read = tls.read(&mut request[count..])?;
        if read == 0 {
            return Err("fixture request head was incomplete".into());
        }
        count += read;
    }
    if !request[..count].starts_with(b"GET ") {
        return Err("expected fixture GET".into());
    }
    tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")?;
    capture.completed(tls.ssl(), true);
    println!(
        "TLS_FACTS={}",
        serde_json::to_string(&capture.snapshot().details)?
    );
    std::io::stdout().flush()?;
    Ok(())
}
