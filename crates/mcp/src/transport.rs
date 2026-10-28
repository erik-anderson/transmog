//! Pinned TLS over an explicitly bound loopback listener. MCP credentials never enter proxy traffic.
use std::{
    convert::Infallible,
    future::Future,
    io::{Read as _, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use boring::{
    hash::MessageDigest,
    ssl::{SslAcceptor, SslConnector, SslMethod, SslVerifyMode},
};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use transmog_tls::{EndpointIdentity, ProxyCa};

use crate::{AccessManager, Error, Grant};

const MAX_REQUEST: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(60);
const DISCOVERY_FILE: &str = "mcp-endpoint.json";

/// Public instance discovery. It contains no client secrets or private TLS material.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    /// Exact explicitly bound IPv4/IPv6 loopback address.
    pub address: SocketAddr,
    /// Fresh generation; retained handles from earlier runs are invalid.
    pub instance_id: String,
    /// SHA-256 of this run's ephemeral server certificate, checked before sending a credential.
    pub certificate_sha256: Vec<u8>,
}

impl Endpoint {
    fn validate(&self) -> Result<(), Error> {
        if !matches!(self.address.ip(), std::net::IpAddr::V4(ip) if ip == std::net::Ipv4Addr::LOCALHOST)
            && !matches!(self.address.ip(), std::net::IpAddr::V6(ip) if ip == std::net::Ipv6Addr::LOCALHOST)
        {
            return Err(unavailable(
                "The discovered MCP endpoint is not a supported loopback address.",
            ));
        }
        if self.address.port() == 0
            || self.certificate_sha256.len() != 32
            || self.instance_id.len() != 32
            || !self
                .instance_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(unavailable(
                "The local MCP endpoint information is invalid.",
            ));
        }
        Ok(())
    }
}

/// One authenticated operation supplied by an application adapter.
pub type CallFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, Error>> + Send + 'a>>;

/// Application operations, independent of HTTP or MCP framing.
pub trait Handler: Send + Sync + 'static {
    /// Runs an operation under an already authenticated grant.
    fn call(&self, grant: Grant, operation: String, input: Value) -> CallFuture<'_>;
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Call {
    operation: String,
    input: Value,
}

/// Owner of one local listener. Dropping it cancels admitted work and unpublishes its endpoint.
pub struct LocalServer {
    endpoint: Endpoint,
    root: PathBuf,
    task: JoinHandle<()>,
}

impl LocalServer {
    /// Binds IPv4 loopback only. No caller-supplied bind address exists.
    /// # Errors
    /// Returns disabled, bind, TLS or discovery-publication failure.
    pub async fn start(
        root: PathBuf,
        access: AccessManager,
        handler: Arc<dyn Handler>,
    ) -> Result<Self, Error> {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| tls_error())?;
        Self::start_for_instance(
            root,
            format!("{:032x}", u128::from_le_bytes(nonce)),
            access,
            handler,
        )
        .await
    }

    /// Binds a listener for an application-owned instance generation.
    /// # Errors
    /// Returns disabled, invalid generation, bind, TLS or discovery-publication failure.
    pub async fn start_for_instance(
        root: PathBuf,
        instance_id: String,
        access: AccessManager,
        handler: Arc<dyn Handler>,
    ) -> Result<Self, Error> {
        if !access.status().enabled {
            return Err(Error::new(
                "mcp_disabled",
                "Enable MCP in Transmog Settings → Integrations → MCP.",
            ));
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| unavailable("The MCP loopback listener could not be started."))?;
        let ca = ProxyCa::generate("Transmog local agent endpoint", 2).map_err(|_| tls_error())?;
        let leaf = ca
            .issue(
                EndpointIdentity::parse("localhost").map_err(|_| tls_error())?,
                1,
            )
            .map_err(|_| tls_error())?;
        let mut tls =
            SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).map_err(|_| tls_error())?;
        tls.set_certificate(&leaf.certificate)
            .map_err(|_| tls_error())?;
        tls.set_private_key(&leaf.private_key)
            .map_err(|_| tls_error())?;
        tls.check_private_key().map_err(|_| tls_error())?;
        let tls = Arc::new(tls.build());
        let endpoint = Endpoint {
            address: listener.local_addr().map_err(|_| tls_error())?,
            instance_id,
            certificate_sha256: leaf
                .certificate
                .digest(MessageDigest::sha256())
                .map_err(|_| tls_error())?
                .to_vec(),
        };
        endpoint.validate()?;
        publish(&root, &endpoint)?;
        let mut authority = access.subscribe();
        let address = endpoint.address;
        let task = tokio::spawn(async move {
            let slots = Arc::new(Semaphore::new(16));
            let mut tasks = JoinSet::new();
            loop {
                if !access.status().enabled {
                    break;
                }
                tokio::select! {
                    changed = authority.changed() => {
                        if changed.is_err() || !access.status().enabled { break; }
                    },
                    result = listener.accept() => {
                        let Ok((stream, peer)) = result else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else { continue; };
                        let tls = Arc::clone(&tls);
                        let handler = Arc::clone(&handler);
                        let access = access.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let _ = timeout(DEADLINE, async move {
                                let Ok(stream) = tokio_boring::accept(&tls, stream).await else { return; };
                                let service = service_fn(move |request| serve(request, address, access.clone(), Arc::clone(&handler)));
                                let _ = hyper::server::conn::http1::Builder::new().keep_alive(false)
                                    .timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5)).max_buf_size(16 * 1024)
                                    .serve_connection(TokioIo::new(stream), service).await;
                            }).await;
                        });
                    },
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            root,
            task,
        })
    }

    /// Safe endpoint information for the desktop settings view.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
        if read_endpoint(&self.root)
            .is_ok_and(|endpoint| endpoint.instance_id == self.endpoint.instance_id)
        {
            let _ = std::fs::remove_file(self.root.join(DISCOVERY_FILE));
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep request validation and authorization checks beside the bounded dispatch."
)]
async fn serve(
    request: Request<Incoming>,
    address: SocketAddr,
    access: AccessManager,
    handler: Arc<dyn Handler>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if request.headers().contains_key(hyper::header::ORIGIN)
        || request
            .headers()
            .get_all(hyper::header::HOST)
            .iter()
            .count()
            != 1
        || request
            .headers()
            .get(hyper::header::HOST)
            .and_then(|value| value.to_str().ok())
            != Some(address.to_string().as_str())
    {
        return Ok(response(
            StatusCode::FORBIDDEN,
            &serde_json::json!({"error":{"code":"invalid_origin","message":"Only the local Transmog connector may access this endpoint."}}),
        ));
    }
    if request.method() != hyper::Method::POST
        || request.uri().path() != "/call"
        || request.uri().query().is_some()
    {
        return Ok(response(
            StatusCode::NOT_FOUND,
            &serde_json::json!({"error":{"code":"not_found","message":"Unknown local operation endpoint."}}),
        ));
    }
    let key = if request
        .headers()
        .get_all(hyper::header::AUTHORIZATION)
        .iter()
        .count()
        == 1
    {
        request
            .headers()
            .get(hyper::header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.strip_prefix("Bearer "))
            .unwrap_or_default()
    } else {
        ""
    };
    let grant = match access.authenticate(key) {
        Ok(grant) => grant,
        Err(error) => {
            return Ok(response(
                StatusCode::UNAUTHORIZED,
                &serde_json::json!({"error":error}),
            ));
        }
    };
    let key = zeroize::Zeroizing::new(key.to_owned());
    let bytes = match Limited::new(request.into_body(), MAX_REQUEST)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => {
            return Ok(response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &serde_json::json!({"error":{"code":"limit","message":"The operation exceeds the request size limit."}}),
            ));
        }
    };
    let call: Call = match serde_json::from_slice(&bytes) {
        Ok(call) => call,
        Err(_) => {
            return Ok(response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({"error":{"code":"invalid_input","message":"The operation arguments are invalid."}}),
            ));
        }
    };
    // Recheck after body admission and monitor changes while an asynchronous operation runs.
    let mut changed = access.subscribe();
    let authority = grant.clone();
    let authorized = access
        .authenticate(&key)
        .is_ok_and(|current| current.same_authority(&authority));
    if !authorized {
        return Ok(response(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({"error":{"code":"authorization_changed","message":"Access changed. Reconnect before continuing."}}),
        ));
    }
    let operation = handler.call(grant, call.operation, call.input);
    tokio::pin!(operation);
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            _ = changed.changed() => {
                if !access.authenticate(&key).is_ok_and(|current| current.same_authority(&authority)) {
                    break Err(Error::new("authorization_changed", "Access was revoked, disabled or changed while this operation was running."));
                }
            },
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                if !access.authenticate(&key).is_ok_and(|current| current.same_authority(&authority)) { break Err(Error::new("unauthorized", "This access key is no longer valid.")); }
            },
        }
    };
    // A completed read must not publish data after its grant was revoked.
    if !access
        .authenticate(&key)
        .is_ok_and(|current| current.same_authority(&authority))
    {
        return Ok(response(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({"error":{"code":"authorization_changed","message":"Access changed before the result could be returned."}}),
        ));
    }
    Ok(match result {
        Ok(result) => response(StatusCode::OK, &serde_json::json!({"result":result})),
        Err(error) => response(StatusCode::OK, &serde_json::json!({"error":error})),
    })
}

fn response(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    let mut bytes = serde_json::to_vec(value).unwrap_or_default();
    if bytes.len() > MAX_RESPONSE {
        bytes = br#"{"error":{"code":"limit","message":"The result exceeds the response limit. Request a smaller page."}}"#.to_vec();
    }
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(bytes)))
        .expect("constant response headers")
}

struct Abort(JoinHandle<()>);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Direct local connector. It never uses environment/system proxies or follows redirects.
#[derive(Clone)]
pub struct LocalClient {
    root: PathBuf,
}

impl LocalClient {
    /// Uses discovery from the selected local product profile.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Reads bounded public discovery and validates its address before any network operation.
    /// # Errors
    /// Returns not-running or invalid local endpoint information.
    pub fn endpoint(&self) -> Result<Endpoint, Error> {
        read_endpoint(&self.root)
    }

    /// Presents the caller's key on this invocation, after verifying the endpoint certificate.
    /// # Errors
    /// Returns safe connection/authentication/application errors. Mutations are never retried.
    pub async fn call(&self, key: &str, operation: &str, input: Value) -> Result<Value, Error> {
        let endpoint = self.endpoint()?;
        timeout(DEADLINE, Self::call_endpoint(&endpoint, key, operation, input)).await
            .map_err(|_| unavailable("The MCP operation timed out. Check Transmog before retrying a change or sending another request."))?
    }

    async fn call_endpoint(
        endpoint: &Endpoint,
        key: &str,
        operation: &str,
        input: Value,
    ) -> Result<Value, Error> {
        let socket = TcpStream::connect(endpoint.address)
            .await
            .map_err(|_| not_running())?;
        let mut connector = SslConnector::builder(SslMethod::tls()).map_err(|_| tls_error())?;
        // The exact run certificate is checked before sending any HTTP bytes; no OS trust grant is installed.
        connector.set_verify(SslVerifyMode::NONE);
        let mut configuration = connector.build().configure().map_err(|_| tls_error())?;
        configuration.set_verify_hostname(false);
        let stream = tokio_boring::connect(configuration, "localhost", socket)
            .await
            .map_err(|_| tls_error())?;
        let certificate = stream.ssl().peer_certificate().ok_or_else(tls_error)?;
        let actual = certificate
            .digest(MessageDigest::sha256())
            .map_err(|_| tls_error())?;
        if !boring::memcmp::eq(&actual, &endpoint.certificate_sha256) {
            return Err(tls_error());
        }
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| not_running())?;
        let connection = tokio::spawn(async move {
            let _ = connection.await;
        });
        let _connection = Abort(connection);
        let body = serde_json::to_vec(&Call {
            operation: operation.into(),
            input,
        })
        .map_err(|_| unavailable("The operation could not be encoded."))?;
        if body.len() > MAX_REQUEST {
            return Err(Error::new(
                "limit",
                "The operation exceeds the request size limit.",
            ));
        }
        let mut authorization = hyper::header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| Error::new("unauthorized", "The supplied access key is malformed."))?;
        authorization.set_sensitive(true);
        let request = Request::builder()
            .method(hyper::Method::POST)
            .uri("/call")
            .header(hyper::header::HOST, endpoint.address.to_string())
            .header(hyper::header::AUTHORIZATION, authorization)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| unavailable("The local request could not be created."))?;
        let response = sender.send_request(request).await.map_err(|_| unavailable("The connection ended before the operation result arrived. Check its outcome before retrying a change."))?;
        let body = Limited::new(response.into_body(), MAX_RESPONSE)
            .collect()
            .await
            .map_err(|_| {
                unavailable("The local response was incomplete or exceeded its size limit.")
            })?
            .to_bytes();
        let mut value: Value = serde_json::from_slice(&body)
            .map_err(|_| unavailable("The local response was invalid."))?;
        if let Some(error) = value.get("error") {
            let code = match error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "mcp_disabled" => "mcp_disabled",
                "unauthorized" => "unauthorized",
                "permission_denied" => "permission_denied",
                "sensitive_access_required" => "sensitive_access_required",
                "request_denied" => "request_denied",
                "authorization_changed" => "authorization_changed",
                "limit" => "limit",
                "conflict" => "conflict",
                "invalid_input" => "invalid_input",
                "not_found" => "not_found",
                _ => "operation_failed",
            };
            return Err(Error::new(
                code,
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The operation failed."),
            ));
        }
        value
            .get_mut("result")
            .map(Value::take)
            .ok_or_else(|| unavailable("The local response contained no result."))
    }
}

fn publish(root: &Path, endpoint: &Endpoint) -> Result<(), Error> {
    std::fs::create_dir_all(root)
        .map_err(|_| unavailable("MCP endpoint information could not be saved."))?;
    let mut file = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| unavailable("MCP endpoint information could not be saved."))?;
    file.write_all(&serde_json::to_vec(endpoint).map_err(|_| tls_error())?)
        .map_err(|_| tls_error())?;
    file.as_file().sync_all().map_err(|_| tls_error())?;
    file.persist(root.join(DISCOVERY_FILE))
        .map_err(|_| unavailable("MCP endpoint information could not be published."))?;
    Ok(())
}

fn read_endpoint(root: &Path) -> Result<Endpoint, Error> {
    let file = std::fs::File::open(root.join(DISCOVERY_FILE)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound { not_running() }
        else { unavailable("Local MCP discovery could not be read. Check Transmog's user profile before reconnecting.") }
    })?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable("Local MCP discovery could not be read."))?;
    if bytes.len() > 4096 {
        return Err(unavailable("Local MCP discovery exceeds its size limit."));
    }
    let endpoint: Endpoint = serde_json::from_slice(&bytes).map_err(|_| {
        unavailable("Local MCP discovery is invalid. Restart Transmog before reconnecting.")
    })?;
    endpoint.validate()?;
    Ok(endpoint)
}

fn unavailable(message: &str) -> Error {
    Error::new("unavailable", message)
}
fn not_running() -> Error {
    Error::new(
        "not_running",
        "Transmog is not accepting local MCP connections. Start Transmog and enable MCP in Settings → Integrations → MCP.",
    )
}
fn tls_error() -> Error {
    unavailable(
        "The local Transmog endpoint could not be authenticated. Reconnect after checking that Transmog is running.",
    )
}

#[cfg(test)]
mod tests;
