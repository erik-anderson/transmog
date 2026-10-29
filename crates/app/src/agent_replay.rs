//! Explicit rule-aware sends use the running native proxy, including its observers and hooks.
use crate::{AppError, Application, ComposerRequest, ComposerResult, ErrorCategory, composer};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_util::rt::TokioIo;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use transmog_session::ReplayLimits;

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn failed(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}

struct ConnectionTask(tokio::task::JoinHandle<()>);
impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Application {
    pub(crate) fn replay_origin(&self, id: &str) -> crate::ComposerOrigin {
        let trace = self.traces.entry(id);
        crate::ComposerOrigin {
            trace_name: trace
                .as_ref()
                .and_then(|entry| self.traces.metadata(&entry.trace_id))
                .map(|metadata| metadata.name),
            entry_id: id.into(),
            trace_id: trace.as_ref().map(|entry| entry.trace_id.clone()),
            original_id: trace.map(|entry| entry.original_id),
        }
    }
    /// Validates a send before operation admission, without sending or creating evidence.
    /// # Errors
    /// Returns input, acknowledgement, scope-independent body or unavailable-mode errors.
    pub fn validate_agent_request(
        &self,
        request: &ComposerRequest,
        live_rules: bool,
    ) -> Result<(), AppError> {
        let (built, _) = composer::build_request(request)?;
        let built = built
            .validate(ReplayLimits::default())
            .map_err(|error| invalid(&error.to_string()))?;
        if !live_rules && !self.composer.has_executor() {
            return Err(failed("The request sender is unavailable."));
        }
        if live_rules {
            let engine = self.live_parent.as_deref().unwrap_or(self);
            engine.agent_proxy_address()?;
            if matches!(
                request.body_source,
                Some(crate::ComposerBodySource::File { .. })
            ) {
                return Err(invalid(
                    "Live-rules sends support inline or captured bodies.",
                ));
            }
            if built.target().scheme == "https"
                && (!engine.product_state().preferences.decrypt_https
                    || engine
                        .proxy_certificate
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_none())
            {
                return Err(invalid(
                    "HTTPS live-rules replay requires HTTPS decryption and the running proxy's interception certificate. Configure it in the app, then start collection.",
                ));
            }
            if engine.body_store.is_none() {
                return Err(invalid(
                    "Body retention is required for live-rules replay context",
                ));
            }
        }
        if let Some(crate::ComposerBodySource::Captured { entry_id }) = &request.body_source {
            let file = self.prepare_request_file(entry_id)?;
            if live_rules
                && file.byte_length().is_none_or(|bytes| {
                    bytes > ReplayLimits::default().max_request_body_bytes as u64
                })
            {
                return Err(invalid(
                    "Live-rules captured bodies require a known complete length within the 4 MiB request budget.",
                ));
            }
        }
        Ok(())
    }

    fn agent_proxy_address(&self) -> Result<SocketAddr, AppError> {
        if self.status().lifecycle != crate::AppLifecycle::Running {
            return Err(invalid(
                "Live-rules replay requires collection to be running.",
            ));
        }
        let mut address: SocketAddr = self
            .status()
            .listener
            .ok_or_else(|| {
                invalid(
                    "Live-rules replay requires the proxy to be running. Start collection first.",
                )
            })?
            .parse()
            .map_err(|_| failed("The running proxy endpoint is unavailable"))?;
        if address.ip().is_unspecified() {
            address.set_ip(if address.is_ipv4() {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
            });
        }
        if !address.ip().is_loopback() {
            return Err(invalid(
                "Live-rules replay requires a local loopback proxy endpoint",
            ));
        }
        Ok(address)
    }

    async fn agent_request_body(
        &self,
        request: &ComposerRequest,
        inline: Bytes,
    ) -> Result<Bytes, AppError> {
        let mut body = inline;
        if let Some(crate::ComposerBodySource::Captured { entry_id }) = &request.body_source {
            let (mut file, length) = self
                .prepare_request_file(entry_id)?
                .into_replay_file()
                .await?;
            if length > ReplayLimits::default().max_request_body_bytes as u64 {
                return Err(invalid(
                    "The captured replay request exceeds the 4 MiB request budget",
                ));
            }
            body = tokio::task::spawn_blocking(move || {
                use std::io::Read as _;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|_| failed("The captured replay body could not be read"))?;
                Ok::<_, AppError>(Bytes::from(bytes))
            })
            .await
            .map_err(|_| failed("The replay body worker failed"))??;
        }
        Ok(body)
    }

    /// Sends through the running rule pipeline and retains all observed message boundaries.
    /// # Errors
    /// Returns preflight errors or an execution/retention error after admission.
    pub async fn execute_agent_request(
        &self,
        request: ComposerRequest,
        live_rules: bool,
    ) -> Result<ComposerResult, AppError> {
        if !live_rules {
            return self.execute_composer(request).await;
        }
        self.validate_agent_request(&request, true)?;
        let engine = self.live_parent.as_deref().unwrap_or(self);
        let address = engine.agent_proxy_address()?;
        let (validated, _) = composer::build_request(&request)?;
        let body = self
            .agent_request_body(&request, validated.body.clone())
            .await?;
        let watermark = engine
            .service
            .catalog()
            .project_retained(|snapshot| snapshot.request_number)
            .into_iter()
            .max()
            .unwrap_or(0);
        let certificate = engine
            .proxy_certificate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let expected = validated.target.clone();
        let expected_method = request.method.clone();
        let (status, local) =
            send_through_proxy(address, &request, validated, body, certificate).await?;
        let mut updates = engine.subscribe_session_updates()?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if let Some(snapshot) = engine
                .service
                .catalog()
                .project_retained(|snapshot| {
                    (snapshot.request_number > watermark
                        && snapshot.metadata.client_addr == local
                        && snapshot.terminal.is_some()
                        && snapshot.request_heads.iter().any(|head| {
                            head.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest
                                && head.head.target == expected
                                && head.head.method == expected_method
                        }))
                    .then(|| snapshot.clone())
                })
                .into_iter()
                .flatten()
                .next()
            {
                break snapshot;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(failed(
                    "Replay completed but its observed context is unavailable",
                ));
            }
            tokio::select! {result=updates.recv()=>{result?;},()=tokio::time::sleep_until(deadline)=>{}}
        };
        let source = request
            .source_entry_id
            .as_ref()
            .map(|id| self.replay_origin(id));
        let retained_source = source.clone();
        let store = engine
            .body_store
            .clone()
            .ok_or_else(|| failed("Body retention is required for live-rules replay context"))?;
        let retained = tokio::task::spawn_blocking(move || {
            crate::replay_context::freeze_exchange(snapshot, &store, retained_source)
        })
        .await
        .map_err(|_| failed("The replay context worker failed"))??;
        let detail = retained.detail()?;
        let id = self
            .composer
            .record(request.method, request.url, Some(status), source.clone());
        self.composer.retain_response(retained);
        Ok(ComposerResult {
            detail,
            id,
            status,
            headers: Vec::new(),
            body: String::new(),
            body_is_hex: false,
            truncated: false,
            attribution: "transmog.app.live-rules-replay",
            source,
        })
    }
}

async fn send_through_proxy(
    address: SocketAddr,
    request: &ComposerRequest,
    validated: transmog_session::ReplayRequest,
    body: Bytes,
    certificate: Option<boring::x509::X509>,
) -> Result<(u16, SocketAddr), AppError> {
    let socket = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|_| failed("The running proxy could not be reached"))?;
    let local = socket
        .local_addr()
        .map_err(|_| failed("The replay connection identity is unavailable"))?;
    let socket: Box<dyn ReplayIo> = if validated.target.scheme == "https" {
        Box::new(
            connect_tls(
                socket,
                &validated.target,
                certificate
                    .ok_or_else(|| failed("The proxy interception certificate is unavailable"))?,
            )
            .await?,
        )
    } else {
        Box::new(socket)
    };
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(socket))
        .await
        .map_err(|_| failed("The local replay connection failed"))?;
    let _connection = ConnectionTask(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let mut outgoing = hyper::Request::builder()
        .method(request.method.as_str())
        .uri(if validated.target.scheme == "https" {
            format!(
                "{}{}",
                validated.target.path,
                validated
                    .target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}"))
            )
        } else {
            request.url.as_str().to_owned()
        });
    let nominated: Vec<_> = validated
        .headers
        .values("connection")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(|name| String::from_utf8_lossy(name).trim().to_owned())
        .collect();
    for field in validated.headers.iter() {
        if ![
            "host",
            "content-length",
            "transfer-encoding",
            "connection",
            "proxy-connection",
        ]
        .iter()
        .any(|name| field.name_eq(name))
            && !nominated.iter().any(|name| field.name_eq(name))
        {
            outgoing = outgoing.header(field.name(), field.value());
        }
    }
    outgoing = outgoing
        .header("Host", validated.target.authority)
        .header("Content-Length", body.len())
        .header("Connection", "close");
    let outgoing = outgoing
        .body(Full::new(body))
        .map_err(|_| invalid("The replay request could not be encoded"))?;
    let response = tokio::time::timeout(Duration::from_secs(30), sender.send_request(outgoing))
        .await
        .map_err(|_| failed("Live-rules replay timed out"))?
        .map_err(|_| failed("Live-rules replay failed after connection admission"))?;
    let status = response.status().as_u16();
    tokio::time::timeout(
        Duration::from_secs(30),
        Limited::new(
            response.into_body(),
            ReplayLimits::default().max_response_body_bytes,
        )
        .collect(),
    )
    .await
    .map_err(|_| failed("Live-rules response timed out"))?
    .map_err(|_| failed("The live-rules response exceeded its budget or was interrupted"))?;
    Ok((status, local))
}

trait ReplayIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> ReplayIo for T {}
async fn connect_tls(
    mut socket: tokio::net::TcpStream,
    target: &transmog_core::Target,
    certificate: boring::x509::X509,
) -> Result<tokio_boring::SslStream<tokio::net::TcpStream>, AppError> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let authority = if target.host.contains(':') {
        format!("[{}]:{}", target.host, target.port)
    } else {
        format!("{}:{}", target.host, target.port)
    };
    let connect = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    let exchange = async {
        socket
            .write_all(connect.as_bytes())
            .await
            .map_err(|_| failed("The local CONNECT request failed"))?;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= 16384 {
                return Err(failed(
                    "The local CONNECT response exceeded its header budget",
                ));
            }
            head.push(
                socket
                    .read_u8()
                    .await
                    .map_err(|_| failed("The local CONNECT response was interrupted"))?,
            );
        }
        if !head.starts_with(b"HTTP/1.1 200 ") && !head.starts_with(b"HTTP/1.0 200 ") {
            return Err(failed("The local proxy did not accept the HTTPS tunnel"));
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(10), exchange)
        .await
        .map_err(|_| failed("The local CONNECT request timed out"))??;
    let mut store = boring::x509::store::X509StoreBuilder::new()
        .map_err(|_| failed("Local replay TLS initialization failed"))?;
    store
        .add_cert(certificate)
        .map_err(|_| failed("Local replay trust initialization failed"))?;
    let mut connector = boring::ssl::SslConnector::builder(boring::ssl::SslMethod::tls())
        .map_err(|_| failed("Local replay TLS initialization failed"))?;
    connector.set_cert_store(store.build());
    connector.set_verify(boring::ssl::SslVerifyMode::PEER);
    connector
        .set_alpn_protos(b"\x08http/1.1")
        .map_err(|_| failed("Local replay TLS protocol initialization failed"))?;
    let configuration = connector
        .build()
        .configure()
        .map_err(|_| failed("Local replay TLS initialization failed"))?;
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio_boring::connect(configuration, &target.host, socket),
    )
    .await
    .map_err(|_| failed("Local replay TLS handshake timed out"))?
    .map_err(|_| failed("The local proxy's interception certificate could not be verified"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    #[tokio::test]
    async fn local_tls_rejects_another_ca_before_sending_request_bytes() {
        let trusted = transmog_tls::ProxyCa::generate("Expected proxy", 2).unwrap();
        let other = transmog_tls::ProxyCa::generate("Unexpected proxy", 2).unwrap();
        let leaf = other
            .issue(
                transmog_tls::EndpointIdentity::parse("localhost").unwrap(),
                1,
            )
            .unwrap();
        let mut acceptor =
            boring::ssl::SslAcceptor::mozilla_intermediate_v5(boring::ssl::SslMethod::tls())
                .unwrap();
        acceptor.set_certificate(&leaf.certificate).unwrap();
        acceptor.set_private_key(&leaf.private_key).unwrap();
        let acceptor = acceptor.build();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(socket.read_u8().await.unwrap());
            }
            socket
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            if let Ok(mut tls) = tokio_boring::accept(&acceptor, socket).await {
                assert!(tls.read_u8().await.is_err());
            }
        });
        let target = transmog_core::Target {
            scheme: "https".into(),
            authority: "localhost:443".into(),
            host: "localhost".into(),
            port: 443,
            path: "/".into(),
            query: None,
        };
        assert!(
            connect_tls(
                tokio::net::TcpStream::connect(address).await.unwrap(),
                &target,
                trusted.certificate().to_owned()
            )
            .await
            .is_err()
        );
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
}
