//! Request-driven client identity selection and per-connection suspension.
use boring::{
    asn1::Asn1Time,
    error::ErrorStack,
    hash::MessageDigest,
    nid::Nid,
    pkey::Id,
    ssl::{
        PrivateKeyMethod, PrivateKeyMethodError, SslContextBuilder, SslRef, SslSignatureAlgorithm,
    },
    x509::{X509, X509NameRef},
};
use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use transmog_core::client_authentication::{
    ClientCertificateAction, ClientCertificateDefault, ClientCertificatePolicy,
    ClientCertificateSelector,
};
pub use transmog_tls_native::CertificateRequest;
use transmog_tls_native::{SelectionResult, clear_client_credentials};

/// Safe provider of a certificate's private signing operations.
/// Implementations may hold an OS key reference or protected local file path.
pub trait ClientSigningKey: Send + Sync + 'static {
    /// Supported TLS signature algorithms for this particular key and curve.
    fn algorithms(&self) -> &[u16];
    /// Rechecks whether the exact selected identity still exists.
    fn available(&self) -> bool;
    /// Signs the full TLS input. Called on a blocking worker, never on the reactor.
    /// # Errors
    /// Returns a redaction-safe explanation if the exact key cannot sign.
    fn sign(&self, algorithm: u16, input: &[u8]) -> Result<Vec<u8>, String>;
}

/// Public identity with a private operation provider, without exportable key bytes.
#[derive(Clone)]
pub struct ClientCredential {
    /// Stable source identifier plus certificate fingerprint.
    pub id: String,
    /// Leaf-first public chain; only this selected chain becomes capture evidence.
    pub chain: Vec<X509>,
    /// Signing provider. Never serialized, logged or stored in preferences.
    pub key: Arc<dyn ClientSigningKey>,
}
impl fmt::Debug for ClientCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientCredential")
            .field("id", &self.id)
            .field("chain_length", &self.chain.len())
            .finish_non_exhaustive()
    }
}
impl ClientCredential {
    /// Matches exact public attributes and an optional pinned identity.
    pub fn matches(&self, selector: &ClientCertificateSelector) -> bool {
        self.chain.first().is_some_and(|leaf| {
            selector.identity.as_ref().is_none_or(|id| id == &self.id)
                && attributes_match(leaf.subject_name(), &selector.subject)
                && attributes_match(leaf.issuer_name(), &selector.issuer)
        })
    }
    /// Checks time, EKU, key usage, issuer constraints, and supported signing algorithms.
    /// Repeated just before installation to reject disappeared or expired choices.
    pub fn compatible(&self, request: &CertificateRequest) -> bool {
        if request.constraints_omitted {
            return false;
        }
        let Some(leaf) = self.chain.first() else {
            return false;
        };
        let Ok(now) = Asn1Time::days_from_now(0) else {
            return false;
        };
        self.chain.len() <= 32
            && self.key.available()
            && transmog_tls_native::permits_client_authentication(leaf)
            && leaf
                .not_before()
                .compare(&now)
                .is_ok_and(|order| !order.is_gt())
            && leaf
                .not_after()
                .compare(&now)
                .is_ok_and(std::cmp::Ordering::is_gt)
            && self.key.algorithms().iter().any(|algorithm| {
                request.signature_algorithms.is_empty()
                    || request.signature_algorithms.contains(algorithm)
            })
            && (request.certificate_types.is_empty()
                || leaf.public_key().is_ok_and(|key| match key.id() {
                    Id::RSA => request.certificate_types.contains(&1),
                    Id::EC | Id::ED25519 => request.certificate_types.contains(&64),
                    _ => false,
                }))
            && (request.issuers.is_empty()
                || self.chain.iter().enumerate().any(|(index, cert)| {
                    cert.issuer_name()
                        .to_der()
                        .is_ok_and(|der| request.issuers.contains(&der))
                        || (index != 0
                            && cert
                                .subject_name()
                                .to_der()
                                .is_ok_and(|der| request.issuers.contains(&der)))
                }))
    }
    /// Lowercase SHA-256 public leaf fingerprint.
    pub fn fingerprint(&self) -> Option<String> {
        self.chain
            .first()?
            .digest(MessageDigest::sha256())
            .ok()
            .map(|bytes| {
                bytes.iter().fold(String::new(), |mut text, byte| {
                    use std::fmt::Write as _;
                    let _ = write!(text, "{byte:02x}");
                    text
                })
            })
    }
}
fn attributes_match(name: &X509NameRef, attributes: &BTreeMap<String, String>) -> bool {
    attributes.iter().all(|(attribute, expected)| {
        let nid = match attribute.as_str() {
            "CN" => Nid::COMMONNAME,
            "O" => Nid::ORGANIZATIONNAME,
            "OU" => Nid::ORGANIZATIONALUNITNAME,
            "C" => Nid::COUNTRYNAME,
            "L" => Nid::LOCALITYNAME,
            "ST" => Nid::STATEORPROVINCENAME,
            "emailAddress" => Nid::PKCS9_EMAILADDRESS,
            _ => return false,
        };
        name.entries_by_nid(nid).any(|entry| {
            entry
                .data()
                .as_utf8()
                .is_ok_and(|value| value.to_string() == *expected)
        })
    })
}

/// Verified TLS destination and immutable caller partition of one physical connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientAuthenticationContext {
    /// Canonical origin host; no path or proxy authority enters authorization.
    pub host: String,
    /// HTTPS origin port.
    pub port: u16,
    /// Immutable caller identity; a run grant must never cross this boundary.
    pub caller: String,
    /// Authorization generation captured before handshake admission.
    pub generation: u64,
}

/// Explicit outcome of a server certificate request.
#[derive(Clone, Debug)]
pub enum ClientAuthenticationDecision {
    /// Send the uniquely selected compatible public identity and prove key possession.
    Certificate(Arc<ClientCredential>),
    /// Try the same handshake anonymously; the server may accept or reject it.
    NoCertificate(String),
    /// End this connection, including when the decision deadline expires.
    Cancelled(String),
}
/// A cancellable owner future for one certificate decision.
pub type ClientAuthenticationFuture =
    Pin<Box<dyn Future<Output = ClientAuthenticationDecision> + Send>>;

/// Embedding callback for actual verified server certificate requests.
pub trait ClientAuthenticationResolver: Send + Sync + 'static {
    /// Current authorization generation, used to retire old pools and stale choices.
    fn generation(&self) -> u64;
    /// Begins a decision. Dropping the future must unregister any pending UI request.
    fn resolve(
        &self,
        context: ClientAuthenticationContext,
        request: CertificateRequest,
    ) -> ClientAuthenticationFuture;
}

/// Public inventory lookup, performed only after a verified server asks for authentication.
pub type ClientCredentialInventoryFuture =
    Pin<Box<dyn Future<Output = Result<Vec<Arc<ClientCredential>>, String>> + Send>>;
/// Read-only identity provider, such as the Windows user store or protected imports.
pub trait ClientCredentialSource: Send + Sync + 'static {
    /// Returns public identities and private operation providers, without signing.
    fn credentials(&self) -> ClientCredentialInventoryFuture;
}
/// Optional embedding controller for explicit, possibly nonmodal, decisions.
pub trait ClientAuthenticationPrompt: Send + Sync + 'static {
    /// Takes ownership of a pending decision. Dropping its future cancels the request.
    /// An empty candidate list still requires an explicit prompt-mode decision.
    fn prompt(
        &self,
        context: ClientAuthenticationContext,
        request: CertificateRequest,
        candidates: Vec<Arc<ClientCredential>>,
        inventory_warning: Option<String>,
    ) -> ClientAuthenticationFuture;
}

/// Host-scoped policy resolver shared by desktop and embedding applications.
/// The first enabled rule wins even when its chosen identity is missing.
pub struct PolicyClientAuthenticationResolver {
    policy: std::sync::RwLock<(u64, ClientCertificatePolicy)>,
    changes: tokio::sync::watch::Sender<u64>,
    source: Arc<dyn ClientCredentialSource>,
    prompt: Option<Arc<dyn ClientAuthenticationPrompt>>,
}
impl PolicyClientAuthenticationResolver {
    /// Creates a resolver with validated non-secret policy and a read-only inventory.
    /// # Errors
    /// Rejects invalid host, port, identifier or selector shapes.
    pub fn new(
        policy: ClientCertificatePolicy,
        source: Arc<dyn ClientCredentialSource>,
        prompt: Option<Arc<dyn ClientAuthenticationPrompt>>,
    ) -> Result<Self, String> {
        if !policy.valid() {
            return Err("Client certificate policy is invalid".into());
        }
        Ok(Self {
            policy: std::sync::RwLock::new((1, policy)),
            changes: tokio::sync::watch::channel(1).0,
            source,
            prompt,
        })
    }
    /// Atomically replaces host authorization and retires its previous generation.
    /// Transport owners must include generation in pool/session reuse authorization.
    /// # Errors
    /// Rejects invalid policy, preserving existing settings and generation.
    pub fn replace_policy(&self, policy: ClientCertificatePolicy) -> Result<u64, String> {
        if !policy.valid() {
            return Err("Client certificate policy is invalid".into());
        }
        let mut state = self
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.1 != policy {
            state.0 = state
                .0
                .checked_add(1)
                .ok_or("Authorization generation exhausted")?;
            state.1 = policy;
        }
        let generation = state.0;
        drop(state);
        self.notify_generation(generation);
        Ok(generation)
    }
    /// Invalidates reusable authenticated connections after a provider identity change.
    /// # Errors
    /// Returns an error only if the monotonic generation is exhausted.
    pub fn inventory_changed(&self) -> Result<u64, String> {
        let mut state = self
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.0 = state
            .0
            .checked_add(1)
            .ok_or("Authorization generation exhausted")?;
        let generation = state.0;
        drop(state);
        self.notify_generation(generation);
        Ok(generation)
    }
    fn notify_generation(&self, generation: u64) {
        self.changes.send_if_modified(|current| {
            if generation > *current {
                *current = generation;
                true
            } else {
                false
            }
        });
    }
}
impl ClientAuthenticationResolver for PolicyClientAuthenticationResolver {
    fn generation(&self) -> u64 {
        self.policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
    }
    fn resolve(
        &self,
        context: ClientAuthenticationContext,
        request: CertificateRequest,
    ) -> ClientAuthenticationFuture {
        let state = self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if context.generation != state.0 {
            return Box::pin(async {
                ClientAuthenticationDecision::Cancelled("authorization changed".into())
            });
        }
        let mode = state
            .1
            .matching_rule(&context.host, context.port)
            .map_or_else(
                || match state.1.default {
                    ClientCertificateDefault::NoCertificate => {
                        ClientCertificateAction::NoCertificate
                    }
                    ClientCertificateDefault::Prompt => ClientCertificateAction::Prompt,
                },
                |rule| rule.mode.clone(),
            );
        drop(state);
        if mode == ClientCertificateAction::NoCertificate {
            return Box::pin(async {
                ClientAuthenticationDecision::NoCertificate("host policy".into())
            });
        }
        let source = self.source.clone();
        let prompt = self.prompt.clone();
        let changes = self.changes.subscribe();
        let generation = context.generation;
        reject_stale_authorization(
            generation,
            changes,
            resolve_policy_action(mode, source, prompt, context, request),
        )
    }
}

fn resolve_policy_action(
    mode: ClientCertificateAction,
    source: Arc<dyn ClientCredentialSource>,
    prompt: Option<Arc<dyn ClientAuthenticationPrompt>>,
    context: ClientAuthenticationContext,
    request: CertificateRequest,
) -> ClientAuthenticationFuture {
    Box::pin(async move {
        let inventory = tokio::time::timeout(Duration::from_secs(5), source.credentials()).await;
        let (candidates, warning) = match inventory {
            Ok(Ok(identities)) if identities.len() <= 1024 => (
                identities
                    .into_iter()
                    .filter(|credential| credential.compatible(&request))
                    .collect::<Vec<_>>(),
                None,
            ),
            Ok(Ok(_)) => (
                Vec::new(),
                Some("Certificate inventory is too large".into()),
            ),
            Ok(Err(_)) => (
                Vec::new(),
                Some("Certificate inventory is unavailable".into()),
            ),
            Err(_) => (
                Vec::new(),
                Some("Certificate inventory lookup timed out".into()),
            ),
        };
        match mode {
            ClientCertificateAction::Automatic { selector } => {
                let mut matches = candidates
                    .into_iter()
                    .filter(|credential| credential.matches(&selector));
                match (matches.next(), matches.next()) {
                    (Some(credential), None) => {
                        ClientAuthenticationDecision::Certificate(credential)
                    }
                    (None, _) => {
                        ClientAuthenticationDecision::NoCertificate(warning.unwrap_or_else(|| {
                            "configured identity is missing or incompatible".into()
                        }))
                    }
                    _ => ClientAuthenticationDecision::NoCertificate(
                        "certificate selector matches multiple identities".into(),
                    ),
                }
            }
            ClientCertificateAction::Prompt => match prompt {
                Some(prompt) => prompt.prompt(context, request, candidates, warning).await,
                None => ClientAuthenticationDecision::Cancelled(
                    "client certificate prompt requires an interactive controller".into(),
                ),
            },
            ClientCertificateAction::NoCertificate => {
                unreachable!("returned before inventory lookup")
            }
        }
    })
}

fn reject_stale_authorization(
    generation: u64,
    mut changes: tokio::sync::watch::Receiver<u64>,
    mut decision: ClientAuthenticationFuture,
) -> ClientAuthenticationFuture {
    Box::pin(async move {
        if *changes.borrow_and_update() > generation {
            return ClientAuthenticationDecision::Cancelled("authorization changed".into());
        }
        loop {
            tokio::select! {
                notification = changes.changed() => {
                    if notification.is_err() || *changes.borrow_and_update() > generation { return ClientAuthenticationDecision::Cancelled("authorization changed".into()); }
                }
                answer = &mut decision => return answer,
            }
        }
    })
}

struct SelectionState {
    future: Option<ClientAuthenticationFuture>,
    finished: bool,
}
struct AttemptState {
    resolver: Arc<dyn ClientAuthenticationResolver>,
    context: ClientAuthenticationContext,
    capture: crate::TlsCapture,
    selection: Mutex<SelectionState>,
    waker: Mutex<Option<Waker>>,
    cancelled: AtomicBool,
    signing_job: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
/// Connection-owned certificate decision and wake registration for TCP or QUIC.
#[derive(Clone)]
pub struct ClientAuthenticationAttempt(Arc<AttemptState>);
impl fmt::Debug for ClientAuthenticationAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientAuthenticationAttempt")
            .field("context", &self.0.context)
            .finish_non_exhaustive()
    }
}
impl ClientAuthenticationAttempt {
    /// Creates an attempt. No resolver or inventory is queried until the peer asks.
    pub fn new(
        resolver: Arc<dyn ClientAuthenticationResolver>,
        context: ClientAuthenticationContext,
        capture: crate::TlsCapture,
    ) -> Self {
        Self(Arc::new(AttemptState {
            resolver,
            context,
            capture,
            selection: Mutex::new(SelectionState {
                future: None,
                finished: false,
            }),
            waker: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            signing_job: Mutex::new(None),
        }))
    }
    /// Registers the reactor's current task before polling the handshake driver.
    pub fn register(&self, waker: &Waker) {
        *self
            .0
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker.clone());
    }
    /// Invalidates a paused handshake and drops its owned resolver future.
    /// Call when the driver detects peer closure, caller cancellation or shutdown.
    pub fn cancel(&self, reason: &str) {
        self.0.cancelled.store(true, Ordering::Release);
        let mut state = self
            .0
            .selection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.finished {
            let waiting = state.future.take().is_some();
            state.finished = true;
            if waiting {
                self.0
                    .capture
                    .authentication_detail("Client certificate", &format!("Cancelled: {reason}"));
            }
        }
        drop(state);
        if let Some(job) = self
            .0
            .signing_job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            job.abort();
        }
        if let Some(waker) = self
            .0
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            waker.wake_by_ref();
        }
    }
    /// Returns bounded public evidence for this physical connection.
    pub fn evidence(&self) -> transmog_core::TlsEvidence {
        self.0.capture.snapshot()
    }
    /// Attaches this attempt to a TCP SSL object.
    /// # Errors
    /// Returns native callback registration errors.
    pub fn attach_ssl(&self, ssl: &mut SslRef) -> Result<(), ErrorStack> {
        let capture = self.0.capture.clone();
        transmog_tls_native::observe_client_authentication(ssl, move |event| {
            capture.client_authentication_event(event);
        })?;
        let attempt = self.clone();
        transmog_tls_native::set_client_certificate_callback(ssl, move |ssl, request| {
            attempt.select(ssl, request)
        })
    }
    /// Attaches this attempt to a connection-specific QUIC context.
    /// # Errors
    /// Returns native callback registration errors.
    pub fn attach_context(&self, context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
        let capture = self.0.capture.clone();
        transmog_tls_native::observe_context_client_authentication(context, move |event| {
            capture.client_authentication_event(event);
        })?;
        let attempt = self.clone();
        transmog_tls_native::set_context_client_certificate_callback(
            context,
            move |ssl, request| attempt.select(ssl, request),
        )
    }
    fn select(&self, ssl: &mut SslRef, request: &CertificateRequest) -> SelectionResult {
        clear_client_credentials(ssl);
        let host = &self.0.context.host;
        let host_is_ip = host.parse::<std::net::IpAddr>().is_ok();
        let matches_peer = ssl.peer_certificate().is_some_and(|certificate| {
            if host_is_ip {
                certificate.check_ip_asc(host).unwrap_or(false)
            } else {
                certificate.check_host(host).unwrap_or(false)
            }
        });
        let matches_sni = ssl
            .servername(boring::ssl::NameType::HOST_NAME)
            .map_or(host_is_ip, |name| name.eq_ignore_ascii_case(host));
        if !matches_peer || !matches_sni {
            self.cancel("authentication origin does not match verified peer");
            return SelectionResult::Failed;
        }
        if self.0.cancelled.load(Ordering::Acquire) {
            return SelectionResult::Failed;
        }
        if self.0.resolver.generation() != self.0.context.generation {
            self.cancel("authorization changed");
            return SelectionResult::Failed;
        }
        let mut state = self
            .0
            .selection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.finished {
            return SelectionResult::Failed;
        }
        if state.future.is_none() {
            if request.constraints_omitted {
                self.0.capture.authentication_detail(
                    "Client certificate request",
                    "Request constraints exceed supported limits; no identity can be selected",
                );
            }
            self.0
                .capture
                .authentication_detail("Client certificate", "Requested");
            let decision = self
                .0
                .resolver
                .resolve(self.0.context.clone(), request.clone());
            state.future = Some(Box::pin(async move {
                tokio::time::timeout(CLIENT_CERTIFICATE_DECISION_TIMEOUT, decision)
                    .await
                    .unwrap_or_else(|_| {
                        ClientAuthenticationDecision::Cancelled(
                            "certificate decision timed out".into(),
                        )
                    })
            }));
        }
        let waker = self
            .0
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Waker::noop().clone());
        let decision = state
            .future
            .as_mut()
            .expect("decision initialized")
            .as_mut()
            .poll(&mut Context::from_waker(&waker));
        match decision {
            Poll::Pending => {
                self.0
                    .capture
                    .authentication_detail("Client certificate", "Waiting for decision");
                SelectionResult::Pending
            }
            Poll::Ready(decision) => {
                state.future = None;
                state.finished = true;
                drop(state);
                if self.0.resolver.generation() != self.0.context.generation {
                    return SelectionResult::Failed;
                }
                self.finish_selection(ssl, request, decision)
            }
        }
    }
    fn finish_selection(
        &self,
        ssl: &mut SslRef,
        request: &CertificateRequest,
        decision: ClientAuthenticationDecision,
    ) -> SelectionResult {
        match decision {
            ClientAuthenticationDecision::Certificate(credential)
                if credential.compatible(request) =>
            {
                let Some(leaf) = credential.chain.first() else {
                    return SelectionResult::Failed;
                };
                if ssl.set_certificate(leaf).is_err()
                    || credential
                        .chain
                        .iter()
                        .skip(1)
                        .any(|issuer| ssl.add_chain_cert(issuer).is_err())
                {
                    return SelectionResult::Failed;
                }
                if transmog_tls_native::set_signing_algorithms(ssl, credential.key.algorithms())
                    .is_err()
                {
                    return SelectionResult::Failed;
                }
                let signer = Arc::new(BackgroundSigner {
                    key: credential.key.clone(),
                    result: Arc::new(Mutex::new(None)),
                    waker: self.0.clone(),
                });
                if transmog_tls_native::set_private_key_method(ssl, signer).is_err() {
                    return SelectionResult::Failed;
                }
                self.0.capture.authentication_detail(
                    "Client certificate",
                    "Selected; transmission not yet confirmed",
                );
                SelectionResult::Ready
            }
            ClientAuthenticationDecision::Certificate(_) => {
                self.0.capture.authentication_detail(
                    "Client certificate decision",
                    "Selected identity is unavailable or incompatible",
                );
                self.0.capture.authentication_detail(
                    "Client certificate",
                    "No certificate: selected identity is unavailable or incompatible",
                );
                SelectionResult::Ready
            }
            ClientAuthenticationDecision::NoCertificate(reason) => {
                self.0
                    .capture
                    .authentication_detail("Client certificate decision", &reason);
                self.0.capture.authentication_detail(
                    "Client certificate",
                    &format!("No certificate: {reason}"),
                );
                SelectionResult::Ready
            }
            ClientAuthenticationDecision::Cancelled(reason) => {
                self.0
                    .capture
                    .authentication_detail("Client certificate", &format!("Cancelled: {reason}"));
                SelectionResult::Failed
            }
        }
    }
    /// Drives a TCP future while registering wakeups for suspended selection/signing.
    pub async fn drive<F: Future>(&self, future: F) -> F::Output {
        let mut guard = AttemptGuard {
            attempt: self.clone(),
            completed: false,
        };
        let mut future = std::pin::pin!(future);
        let result = std::future::poll_fn(|context| {
            self.register(context.waker());
            future.as_mut().poll(context)
        })
        .await;
        guard.completed = true;
        self.cancel("handshake ended before a decision");
        result
    }
}

struct AttemptGuard {
    attempt: ClientAuthenticationAttempt,
    completed: bool,
}
impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.attempt.cancel("handshake owner was cancelled");
        }
    }
}

type SigningResult = Arc<Mutex<Option<Result<Vec<u8>, String>>>>;
struct BackgroundSigner {
    key: Arc<dyn ClientSigningKey>,
    result: SigningResult,
    waker: Arc<AttemptState>,
}
impl PrivateKeyMethod for BackgroundSigner {
    fn sign(
        &self,
        _: &mut SslRef,
        input: &[u8],
        algorithm: SslSignatureAlgorithm,
        _: &mut [u8],
    ) -> Result<usize, PrivateKeyMethodError> {
        let Some(algorithm) = self
            .key
            .algorithms()
            .iter()
            .copied()
            .find(|value| SslSignatureAlgorithm::from(*value) == algorithm)
        else {
            return Err(PrivateKeyMethodError::FAILURE);
        };
        let key = self.key.clone();
        let input = input.to_vec();
        let result = self.result.clone();
        let waker = self.waker.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Err(PrivateKeyMethodError::FAILURE);
        };
        let job = runtime.spawn_blocking(move || {
            let signed = if !waker.cancelled.load(Ordering::Acquire)
                && key.available()
                && waker.resolver.generation() == waker.context.generation
            {
                key.sign(algorithm, &input)
            } else {
                Err("selected identity is no longer authorized or available".into())
            };
            *result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(signed);
            if let Some(waker) = waker
                .waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                waker.wake_by_ref();
            }
        });
        *self
            .waker
            .signing_job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
        Err(PrivateKeyMethodError::RETRY)
    }
    fn decrypt(
        &self,
        _: &mut SslRef,
        _: &[u8],
        _: &mut [u8],
    ) -> Result<usize, PrivateKeyMethodError> {
        Err(PrivateKeyMethodError::FAILURE)
    }
    fn complete(&self, _: &mut SslRef, output: &mut [u8]) -> Result<usize, PrivateKeyMethodError> {
        if self.waker.cancelled.load(Ordering::Acquire)
            || self.waker.resolver.generation() != self.waker.context.generation
        {
            return Err(PrivateKeyMethodError::FAILURE);
        }
        match self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            None => Err(PrivateKeyMethodError::RETRY),
            Some(Ok(signature)) if signature.len() <= output.len() => {
                output[..signature.len()].copy_from_slice(&signature);
                Ok(signature.len())
            }
            _ => Err(PrivateKeyMethodError::FAILURE),
        }
    }
}

/// Maximum time an explicit prompt may retain a physical handshake.
pub const CLIENT_CERTIFICATE_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
