use super::common::{Error, Hello, Shared, hello, hex, record_hello, tls};
use boring::{
    rand::rand_bytes,
    ssl::{SslContext, SslMethod},
};
use quiche::h3::NameValue;
use serde_json::json;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use transmog_tls::IssuedLeaf;

struct Peer {
    connection: quiche::Connection,
    http: Option<quiche::h3::Connection>,
    hello: Hello,
    initial: Vec<u8>,
    pending_headers: Option<Vec<(String, String)>>,
}
fn peer(
    leaf: &IssuedLeaf,
    local: SocketAddr,
    from: SocketAddr,
    initial: Vec<u8>,
) -> Result<Peer, Error> {
    let hello = hello();
    let mut builder = SslContext::builder(SslMethod::tls())?;
    builder.set_certificate(&leaf.certificate)?;
    builder.set_private_key(&leaf.private_key)?;
    for issuer in &leaf.issuer_chain {
        builder.add_extra_chain_cert(issuer.clone())?;
    }
    record_hello(&mut builder, &hello);
    let mut config =
        quiche::Config::with_boring_ssl_ctx_builder(quiche::PROTOCOL_VERSION, builder)?;
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
    config.set_max_idle_timeout(10000);
    config.set_max_recv_udp_payload_size(65535);
    config.set_max_send_udp_payload_size(1350);
    config.set_initial_max_data(4 * 1024 * 1024);
    config.set_initial_max_stream_data_bidi_local(1024 * 1024);
    config.set_initial_max_stream_data_bidi_remote(1024 * 1024);
    config.set_initial_max_stream_data_uni(1024 * 1024);
    config.set_initial_max_streams_bidi(16);
    config.set_initial_max_streams_uni(16);
    let mut cid = [0; 16];
    rand_bytes(&mut cid)?;
    let connection = quiche::accept(
        &quiche::ConnectionId::from_ref(&cid),
        None,
        local,
        from,
        &mut config,
    )?;
    Ok(Peer {
        connection,
        http: None,
        hello,
        initial,
        pending_headers: None,
    })
}
async fn run(
    socket: tokio::net::UdpSocket,
    leaf: IssuedLeaf,
    shared: Arc<Shared>,
) -> Result<(), Error> {
    let local = socket.local_addr()?;
    let mut peers = HashMap::<SocketAddr, Peer>::new();
    let mut input = vec![0; 65535];
    let mut output = [0; 1350];
    loop {
        tokio::select! {
            packet = socket.recv_from(&mut input) => {
                let (n, from) = packet?;
                if let std::collections::hash_map::Entry::Vacant(entry) = peers.entry(from) {
                    let initial = input[..n].to_vec();
                    let header = quiche::Header::from_slice(&mut input[..n], quiche::MAX_CONN_ID_LEN)?;
                    if header.ty != quiche::Type::Initial { continue; }
                    entry.insert(peer(&leaf, local, from, initial)?);
                }
                let connection = &mut peers.get_mut(&from).ok_or("missing QUIC peer")?.connection;
                let _ = connection.recv(&mut input[..n], quiche::RecvInfo {from, to:local});
            },
            () = tokio::time::sleep(Duration::from_millis(2)) => {},
        }
        for (from, peer) in &mut peers {
            let connection = &mut peer.connection;
            if connection.is_established() && peer.http.is_none() {
                peer.http = Some(quiche::h3::Connection::with_transport(
                    connection,
                    &quiche::h3::Config::new()?,
                )?);
            }
            if let Some(http) = &mut peer.http {
                loop {
                    match http.poll(connection) {
                        Ok((stream, quiche::h3::Event::Headers { list, .. })) => {
                            if list
                                .iter()
                                .any(|h| h.name() == b":path" && h.value() == b"/probe")
                            {
                                let headers = list
                                    .iter()
                                    .map(|h| {
                                        (
                                            String::from_utf8_lossy(h.name()).into_owned(),
                                            String::from_utf8_lossy(h.value()).into_owned(),
                                        )
                                    })
                                    .collect::<Vec<_>>();
                                peer.pending_headers = Some(headers);
                            }
                            let body = shared.body();
                            http.send_response(
                                connection,
                                stream,
                                &[
                                    quiche::h3::Header::new(b":status", b"200"),
                                    quiche::h3::Header::new(b"content-type", b"text/html"),
                                    quiche::h3::Header::new(
                                        b"content-length",
                                        body.len().to_string().as_bytes(),
                                    ),
                                    quiche::h3::Header::new(
                                        b"x-local-probe",
                                        shared.proof.as_bytes(),
                                    ),
                                ],
                                false,
                            )?;
                            http.send_body(connection, stream, body.as_bytes(), true)?;
                        }
                        Ok(_) => {}
                        Err(quiche::h3::Error::Done) => break,
                        Err(error) => return Err(error.into()),
                    }
                }
                // QUIC streams can arrive out of order: the request may precede
                // the peer's control stream. Never turn that into empty SETTINGS.
                if let Some(settings) = http.peer_settings_raw()
                    && let Some(headers) = peer.pending_headers.take()
                {
                    shared.save(&json!({"protocol":"h3", "peer":from.to_string(), "local":local.to_string(), "tls":tls(connection.as_mut(), &peer.hello), "headers":headers, "settings":settings, "quic_initial":hex(&peer.initial)}))?;
                }
            }
            if connection.timeout().is_some_and(|d| d.is_zero()) {
                connection.on_timeout();
            }
            while let Ok((n, info)) = connection.send(&mut output) {
                socket.send_to(&output[..n], info.to).await?;
            }
        }
        peers.retain(|_, peer| !peer.connection.is_closed());
        if peers.len() > 32 {
            return Err("too many fixture QUIC peers".into());
        }
    }
}
pub async fn serve(socket: tokio::net::UdpSocket, leaf: IssuedLeaf, shared: Arc<Shared>) {
    if let Err(error) = run(socket, leaf, shared).await {
        eprintln!("QUIC origin: {error}");
    }
}
