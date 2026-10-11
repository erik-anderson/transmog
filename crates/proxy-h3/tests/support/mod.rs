//! Shared local QUIC server that distinguishes authenticated responses.
#![allow(dead_code)]
use super::support::Fixture;
use boring::{
    ssl::{SslContext, SslMethod, SslVerifyMode},
    x509::store::X509StoreBuilder,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use transmog_tls::EndpointIdentity;
pub struct Origin {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
    pub count: Arc<AtomicUsize>,
    pub requests: Arc<AtomicUsize>,
    pub release: tokio::sync::watch::Sender<bool>,
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn config(f: &Fixture) -> quiche::Config {
    let identity =
        f.ca.issue(EndpointIdentity::parse("localhost").unwrap(), 1)
            .unwrap();
    let mut tls = SslContext::builder(SslMethod::tls()).unwrap();
    tls.set_certificate(&identity.certificate).unwrap();
    tls.set_private_key(&identity.private_key).unwrap();
    let mut trust = X509StoreBuilder::new().unwrap();
    trust
        .add_cert(f.source.credential.chain.last().unwrap().clone())
        .unwrap();
    tls.set_cert_store(trust.build());
    tls.set_verify(SslVerifyMode::PEER);
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, tls).unwrap();
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(3000);
    config.set_max_recv_udp_payload_size(1350);
    config.set_max_send_udp_payload_size(1350);
    config.set_initial_max_data(1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(256 * 1024);
    config.set_initial_max_stream_data_bidi_remote(256 * 1024);
    config.set_initial_max_stream_data_uni(256 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    config
}
pub async fn origin(f: &Fixture) -> Origin {
    origin_at(f, "127.0.0.1:0").await
}
pub async fn origin_at(f: &Fixture, bind: &str) -> Origin {
    origin_options(f, bind, true).await
}
pub async fn paused_origin(f: &Fixture) -> Origin {
    origin_options(f, "127.0.0.1:0", false).await
}
#[allow(clippy::too_many_lines)]
async fn origin_options(f: &Fixture, bind: &str, released: bool) -> Origin {
    let mut config = config(f);
    let socket = tokio::net::UdpSocket::bind(bind).await.unwrap();
    let local = socket.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let counts = count.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    let (release, released) = tokio::sync::watch::channel(released);
    let task = tokio::spawn(async move {
        let mut peers = HashMap::<
            std::net::SocketAddr,
            (
                quiche::Connection,
                Option<quiche::h3::Connection>,
                usize,
                HashMap<u64, Vec<u8>>,
            ),
        >::new();
        let mut input = vec![0; 65535];
        let mut output = [0; 1350];
        loop {
            tokio::select! { packet = socket.recv_from(&mut input) => {
                let (n, from) = match packet {
                    Ok(packet) => packet,
                    // Windows reports ICMP from a retired client's UDP port on
                    // the next receive; the server still owns other live peers.
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => continue,
                    Err(error) => panic!("QUIC fixture receive failed: {error}"),
                };
                if let std::collections::hash_map::Entry::Vacant(entry) = peers.entry(from) {
                    let header = quiche::Header::from_slice(&mut input[..n], quiche::MAX_CONN_ID_LEN).unwrap(); if header.ty != quiche::Type::Initial { continue; }
                    let id = counts.fetch_add(1, Ordering::SeqCst) + 1; let source = id.to_be_bytes();
                    entry.insert((quiche::accept(&quiche::ConnectionId::from_ref(&source), None, local, from, &mut config).unwrap(), None, id, HashMap::new()));
                }
                let (connection, _, _, _) = peers.get_mut(&from).unwrap();
                let _ = connection.recv(&mut input[..n], quiche::RecvInfo { from, to: local });
            }, () = tokio::time::sleep(Duration::from_millis(2)) => {} }
            for (connection, http3, id, pending) in peers.values_mut() {
                if connection.is_established() && http3.is_none() {
                    *http3 = Some(
                        quiche::h3::Connection::with_transport(
                            connection,
                            &quiche::h3::Config::new().unwrap(),
                        )
                        .unwrap(),
                    );
                }
                if let Some(http3) = http3.as_mut() {
                    loop {
                        match http3.poll(connection) {
                            Ok((stream, quiche::h3::Event::Headers { .. })) => {
                                let auth = connection
                                    .peer_cert_chain()
                                    .is_some_and(|chain| !chain.is_empty());
                                let body = format!("{id}:{auth}");
                                http3
                                    .send_response(
                                        connection,
                                        stream,
                                        &[
                                            quiche::h3::Header::new(b":status", b"200"),
                                            quiche::h3::Header::new(
                                                b"content-length",
                                                body.len().to_string().as_bytes(),
                                            ),
                                        ],
                                        false,
                                    )
                                    .unwrap();
                                pending.insert(stream, body.into_bytes());
                                request_count.fetch_add(1, Ordering::SeqCst);
                            }
                            Ok((stream, quiche::h3::Event::Data)) => {
                                while http3.recv_body(connection, stream, &mut input).is_ok() {}
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    if *released.borrow() {
                        pending.retain(|stream, body| {
                            match http3.send_body(connection, *stream, body, true) {
                                Ok(n) => {
                                    body.drain(..n);
                                    !body.is_empty()
                                }
                                Err(quiche::h3::Error::Done) => true,
                                Err(error) => panic!("QUIC fixture response failed: {error}"),
                            }
                        });
                    }
                }
                if connection.timeout().is_some_and(|d| d.is_zero()) {
                    connection.on_timeout();
                }
                while let Ok((n, info)) = connection.send(&mut output) {
                    socket.send_to(&output[..n], info.to).await.unwrap();
                }
            }
        }
    });
    Origin {
        port: local.port(),
        task,
        count,
        requests,
        release,
    }
}
