//! Full TLS handshake measurements; fixture construction is outside the latency samples.
use boring::ssl::{SslAcceptor, SslStream, SslVersion};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use transmog_tls::{
    CachedMitmCertificateResolver, DownstreamCertificateResolver, DownstreamTlsContextFactory,
    DownstreamTlsPolicy, EndpointIdentity, PemTrustSource, ProxyCa, TrustSnapshot,
    UpstreamTlsContextFactory, UpstreamTlsPolicy,
};

#[derive(Debug)]
struct MeasuredStream {
    socket: TcpStream,
    hello_bytes: usize,
}
impl Read for MeasuredStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read(bytes)
    }
}
impl Write for MeasuredStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.hello_bytes == 0 && bytes.len() >= 5 && bytes[0] == 22 {
            self.hello_bytes = 5 + usize::from(u16::from_be_bytes([bytes[3], bytes[4]]));
        }
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
fn socket(socket: &TcpStream) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    socket.set_nodelay(true)
}

fn serve(listener: &TcpListener, acceptor: &SslAcceptor, operations: usize) {
    for _ in 0..operations {
        let (tcp, _) = listener.accept().unwrap();
        socket(&tcp).unwrap();
        let mut tls = acceptor.accept(tcp).unwrap();
        let mut byte = [0];
        tls.read_exact(&mut byte).unwrap();
        tls.write_all(&byte).unwrap();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 3 {
        return Err("expected SIGNATURE GROUP OPERATIONS".into());
    }
    let operations: usize = arguments[2].parse()?;
    if !(10..=10_000).contains(&operations) {
        return Err("operations must be 10-10000".into());
    }
    let hybrid = match arguments[1].as_str() {
        "hybrid" => true,
        "classical" => false,
        _ => return Err("unsupported exchange group".into()),
    };
    let ca = ProxyCa::generate("PQC handshake benchmark", 1)?;
    let trust = PemTrustSource::from_pem(&ca.certificate_pem()?, "benchmark root")?;
    let resolver = CachedMitmCertificateResolver::new(ca, 1, 1)?;
    let mut leaves =
        resolver.resolve_credentials(&EndpointIdentity::parse("localhost")?, SystemTime::now())?;
    let index = match arguments[0].as_str() {
        "automatic" => None,
        "mldsa44" => Some(0),
        "mldsa65" => Some(1),
        "mldsa87" => Some(2),
        "classical" => Some(3),
        _ => return Err("unsupported signature".into()),
    };
    if let Some(index) = index {
        leaves = vec![leaves.remove(index)];
    }
    let acceptor = DownstreamTlsContextFactory::new(DownstreamTlsPolicy {
        minimum_version: SslVersion::TLS1_3,
        ..DownstreamTlsPolicy::default()
    })
    .acceptor_credentials_captured(&leaves, None)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = std::thread::spawn(move || serve(&listener, &acceptor, operations));
    let factory = UpstreamTlsContextFactory::new(
        Arc::new(TrustSnapshot::load(&trust, 1)?),
        UpstreamTlsPolicy::default(),
    );
    let mut context = factory.synchronous_connector_builder(b"\x08http/1.1")?;
    if !hybrid {
        context.set_curves_list("X25519:P-256:P-384")?;
    }
    let context = context.build();
    let mut samples = Vec::with_capacity(operations);
    let mut hello_bytes = 0;
    let expected_signature = match arguments[0].as_str() {
        "automatic" => "mldsa44",
        "classical" => "ecdsa_secp256r1_sha256",
        other => other,
    };
    for _ in 0..operations {
        let tcp = TcpStream::connect(address)?;
        socket(&tcp)?;
        let ssl = context.configure()?.into_ssl("localhost")?;
        let mut tls = SslStream::new(
            ssl,
            MeasuredStream {
                socket: tcp,
                hello_bytes: 0,
            },
        )?;
        let began = Instant::now();
        tls.connect()?;
        samples.push(u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
        assert!(!tls.ssl().session_reused());
        assert_eq!(tls.ssl().curve(), Some(if hybrid { 0x11ec } else { 29 }));
        assert_eq!(
            tls.ssl().peer_signature_algorithm().unwrap().to_string(),
            expected_signature
        );
        tls.write_all(&[7])?;
        let mut byte = [0];
        tls.read_exact(&mut byte)?;
        assert_eq!(byte, [7]);
        if hello_bytes != 0 {
            assert_eq!(hello_bytes, tls.get_ref().hello_bytes);
        }
        hello_bytes = tls.get_ref().hello_bytes;
    }
    server.join().map_err(|_| "benchmark server failed")?;
    samples.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "signature": arguments[0], "group": arguments[1], "operations": operations,
            "p50HandshakeNanos": samples[operations / 2], "p95HandshakeNanos": samples[operations * 95 / 100],
            "clientHelloRecordBytes": hello_bytes, "resumed": false,
        })
    );
    Ok(())
}
