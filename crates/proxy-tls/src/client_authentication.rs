//! Request-driven client identity selection and per-connection suspension.
use boring::{
    asn1::Asn1Time,
    error::ErrorStack,
    hash::MessageDigest,
    nid::Nid,
    pkey::{Id, PKeyRef, Public},
    ssl::{
        PrivateKeyMethod, PrivateKeyMethodError, SslContextBuilder, SslRef, SslSignatureAlgorithm,
        SslVersion,
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
    /// Supported TLS signature algorithms for this key. Eligibility also checks
    /// the leaf key and negotiated version (TLS 1.2 does not bind ECDSA curves).
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
    /// May query a native key provider; call from a blocking worker.
    pub fn compatible(&self, request: &CertificateRequest) -> bool {
        self.public_compatible(request) && self.key.available()
    }
    fn public_compatible(&self, request: &CertificateRequest) -> bool {
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
            && transmog_tls_native::permits_client_authentication(leaf)
            && leaf
                .not_before()
                .compare(&now)
                .is_ok_and(|order| !order.is_gt())
            && leaf
                .not_after()
                .compare(&now)
                .is_ok_and(std::cmp::Ordering::is_gt)
            && !self.signing_algorithms(request).is_empty()
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
    fn signing_algorithms(&self, request: &CertificateRequest) -> Vec<u16> {
        let Some(version) = request.tls_version else {
            return Vec::new();
        };
        let Some(key) = self.chain.first().and_then(|leaf| leaf.public_key().ok()) else {
            return Vec::new();
        };
        let mut algorithms = Vec::new();
        for &algorithm in self.key.algorithms() {
            if !algorithms.contains(&algorithm)
                && (request.signature_algorithms.is_empty()
                    || request.signature_algorithms.contains(&algorithm))
                && key_supports_algorithm(&key, version, algorithm)
            {
                algorithms.push(algorithm);
            }
        }
        algorithms
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
// Keep these eligibility rules aligned with the pinned BoringSSL's
// ssl_pkey_supports_algorithm. Unsupported schemes and versions fail closed.
fn key_supports_algorithm(key: &PKeyRef<Public>, version: SslVersion, algorithm: u16) -> bool {
    if !matches!(version, SslVersion::TLS1_2 | SslVersion::TLS1_3) {
        return false;
    }
    match algorithm {
        0x0201 | 0x0401 | 0x0501 | 0x0601 => version == SslVersion::TLS1_2 && key.id() == Id::RSA,
        0x0420 => version == SslVersion::TLS1_3 && key.id() == Id::RSA,
        0x0804..=0x0806 => {
            let hash_size = match algorithm {
                0x0804 => 32,
                0x0805 => 48,
                _ => 64,
            };
            key.id() == Id::RSA && key.size() >= 2 * hash_size + 2
        }
        0x0203 => version == SslVersion::TLS1_2 && key.id() == Id::EC,
        0x0403 | 0x0503 | 0x0603 => {
            key.id() == Id::EC
                && (version == SslVersion::TLS1_2
                    || key.ec_key().is_ok_and(|ec| {
                        ec.group().curve_name()
                            == Some(match algorithm {
                                0x0403 => Nid::X9_62_PRIME256V1,
                                0x0503 => Nid::SECP384R1,
                                _ => Nid::SECP521R1,
                            })
                    }))
        }
        0x0807 => key.id() == Id::ED25519,
        0x0904..=0x0906 => {
            version == SslVersion::TLS1_3
                && transmog_tls_native::MlDsaVariant::for_key(key).is_some_and(|variant| {
                    variant.signature_algorithm() == SslSignatureAlgorithm::from(algorithm)
                })
        }
        _ => false,
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
    /// Monotonically increasing authorization generation (initial zero is valid),
    /// used to retire old pools and stale choices. Never reuse an earlier epoch.
    fn generation(&self) -> u64;
    /// Whether this origin's choice is deterministic for the current generation.
    /// Interactive or connection-specific embedding decisions must not enter a reusable pool.
    fn permits_connection_reuse(&self, _host: &str, _port: u16) -> bool {
        false
    }
    /// Confirms that the actual selected identity may enter this origin's reusable pool.
    fn permits_selected_connection_reuse(
        &self,
        _host: &str,
        _port: u16,
        _identity: Option<&str>,
    ) -> bool {
        false
    }
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
    fn permits_connection_reuse(&self, host: &str, port: u16) -> bool {
        let state = self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.1.matching_rule(host, port).map_or(
            state.1.default == ClientCertificateDefault::NoCertificate,
            |rule| match &rule.mode {
                ClientCertificateAction::NoCertificate => true,
                ClientCertificateAction::Automatic { selector } => selector.identity.is_some(),
                ClientCertificateAction::Prompt => false,
            },
        )
    }
    fn permits_selected_connection_reuse(
        &self,
        host: &str,
        port: u16,
        identity: Option<&str>,
    ) -> bool {
        let state = self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.1.matching_rule(host, port).map_or(
            state.1.default == ClientCertificateDefault::NoCertificate && identity.is_none(),
            |rule| match &rule.mode {
                ClientCertificateAction::NoCertificate => identity.is_none(),
                ClientCertificateAction::Automatic { selector } => selector
                    .identity
                    .as_deref()
                    .is_some_and(|expected| identity == Some(expected)),
                ClientCertificateAction::Prompt => false,
            },
        )
    }
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
            changes.clone(),
            resolve_policy_action(mode, source, prompt, context, request, changes),
        )
    }
}

fn resolve_policy_action(
    mode: ClientCertificateAction,
    source: Arc<dyn ClientCredentialSource>,
    prompt: Option<Arc<dyn ClientAuthenticationPrompt>>,
    context: ClientAuthenticationContext,
    request: CertificateRequest,
    changes: tokio::sync::watch::Receiver<u64>,
) -> ClientAuthenticationFuture {
    Box::pin(async move {
        let inventory = tokio::time::timeout(Duration::from_secs(5), source.credentials()).await;
        let (candidates, warning) = match inventory {
            Ok(Ok(identities)) if identities.len() <= 1024 => {
                let constraints = request.clone();
                let generation = context.generation;
                match crate::blocking_job::run(move |cancelled| {
                    identities
                        .into_iter()
                        .take_while(|_| {
                            !cancelled.load(Ordering::Acquire) && *changes.borrow() == generation
                        })
                        .filter(|credential| credential.compatible(&constraints))
                        .collect::<Vec<_>>()
                })
                .await
                {
                    Ok(candidates) => (candidates, None),
                    Err(_) => (
                        Vec::new(),
                        Some("Certificate inventory is unavailable".into()),
                    ),
                }
            }
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

enum SelectionState {
    NotStarted,
    // The SSL callback owns the future while invoking or polling embedding code.
    // Cancellation can mark Finished without touching that in-flight future.
    Polling,
    Waiting(ClientAuthenticationFuture),
    Finished,
}
struct AttemptState {
    selected_identity: Mutex<Option<String>>,
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
    /// Exact public source identity installed for this physical connection.
    pub fn selected_identity(&self) -> Option<String> {
        self.0
            .selected_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// Whether the initial handshake is awaiting a decision or private operation.
    pub fn is_waiting(&self) -> bool {
        matches!(
            *self
                .0
                .selection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            SelectionState::Waiting(_) | SelectionState::Polling
        ) || self
            .0
            .signing_job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|job| !job.is_finished())
    }
    /// Creates an attempt. No resolver or inventory is queried until the peer asks.
    pub fn new(
        resolver: Arc<dyn ClientAuthenticationResolver>,
        context: ClientAuthenticationContext,
        capture: crate::TlsCapture,
    ) -> Self {
        Self(Arc::new(AttemptState {
            selected_identity: Mutex::new(None),
            resolver,
            context,
            capture,
            selection: Mutex::new(SelectionState::NotStarted),
            waker: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            signing_job: Mutex::new(None),
        }))
    }
    /// Registers the reactor's current task before polling the handshake driver.
    pub fn register(&self, waker: &Waker) {
        if self.0.cancelled.load(Ordering::Acquire) {
            return;
        }
        let waker = waker.clone();
        let previous = {
            let mut registration = self
                .0
                .waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.0.cancelled.load(Ordering::Acquire) {
                Some(waker)
            } else {
                registration.replace(waker)
            }
        };
        drop(previous);
    }
    /// Invalidates a paused handshake and drops its owned resolver future.
    /// Call when the driver detects peer closure, caller cancellation or shutdown.
    pub fn cancel(&self, reason: &str) {
        if self.0.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let previous = {
            let mut state = self
                .0
                .selection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::mem::replace(&mut *state, SelectionState::Finished);
            if matches!(
                previous,
                SelectionState::Polling | SelectionState::Waiting(_)
            ) {
                self.0
                    .capture
                    .authentication_detail("Client certificate", &format!("Cancelled: {reason}"));
            }
            previous
        };
        let job = self
            .0
            .signing_job
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(job) = job {
            job.abort();
        }
        let waker = self
            .0
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(waker) = waker {
            waker.wake();
        }
        // Finish essential cancellation before invoking embedding cleanup.
        // It can reenter cancel or panic, including during an owner's unwinding.
        // No selection/job/waker lock is held while its future is destroyed.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(previous))).is_err() {
            self.0.capture.authentication_detail(
                "Client certificate cleanup",
                "Pending request cleanup failed",
            );
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
        let matches_peer = transmog_tls_native::peer_verified_for_name(ssl, host);
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
        match self.poll_decision(request) {
            Err(()) => SelectionResult::Failed,
            Ok(None) => SelectionResult::Pending,
            Ok(Some(decision)) => {
                let result = self.finish_selection(ssl, request, decision);
                if self.0.cancelled.load(Ordering::Acquire)
                    || self.0.resolver.generation() != self.0.context.generation
                {
                    clear_client_credentials(ssl);
                    SelectionResult::Failed
                } else {
                    result
                }
            }
        }
    }
    fn start_decision(&self, request: &CertificateRequest) -> ClientAuthenticationFuture {
        let decision = self
            .0
            .resolver
            .resolve(self.0.context.clone(), request.clone());
        let request = request.clone();
        // The parked future is owned by AttemptState. A strong reference back
        // to that state would keep an abandoned pending request alive forever.
        let attempt = Arc::downgrade(&self.0);
        Box::pin(async move {
            let prepared = async move {
                match decision.await {
                    ClientAuthenticationDecision::Certificate(credential) => {
                        // Windows acquisition and pairing checks belong on a worker,
                        // like Chromium's platform-key task runner. No native key
                        // lookup happens in the synchronous SSL selection callback.
                        match crate::blocking_job::run(move |cancelled| {
                            let attempt = attempt.upgrade()?;
                            if cancelled.load(Ordering::Acquire)
                                || attempt.resolver.generation() != attempt.context.generation
                                || attempt.cancelled.load(Ordering::Acquire)
                            {
                                return None;
                            }
                            credential.compatible(&request).then_some(credential)
                        })
                        .await
                        {
                            Ok(Some(credential)) => {
                                ClientAuthenticationDecision::Certificate(credential)
                            }
                            Ok(None) => ClientAuthenticationDecision::NoCertificate(
                                "selected identity is unavailable or incompatible".into(),
                            ),
                            Err(_) => ClientAuthenticationDecision::Cancelled(
                                "client certificate provider failed".into(),
                            ),
                        }
                    }
                    decision => decision,
                }
            };
            tokio::time::timeout(CLIENT_CERTIFICATE_DECISION_TIMEOUT, prepared)
                .await
                .unwrap_or_else(|_| {
                    ClientAuthenticationDecision::Cancelled("certificate decision timed out".into())
                })
        })
    }
    fn poll_decision(
        &self,
        request: &CertificateRequest,
    ) -> Result<Option<ClientAuthenticationDecision>, ()> {
        let previous = {
            let mut state = self
                .0
                .selection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match *state {
                SelectionState::Finished => return Err(()),
                SelectionState::Polling => return Ok(None),
                _ => (),
            }
            if matches!(*state, SelectionState::NotStarted) {
                if request.constraints_omitted {
                    self.0.capture.authentication_detail(
                        "Client certificate request",
                        "Request constraints exceed supported limits; no identity can be selected",
                    );
                }
                self.0
                    .capture
                    .authentication_detail("Client certificate", "Requested");
            }
            std::mem::replace(&mut *state, SelectionState::Polling)
        };
        if self.0.cancelled.load(Ordering::Acquire) {
            return Err(());
        }
        let mut future = match previous {
            SelectionState::NotStarted => self.start_decision(request),
            SelectionState::Waiting(future) => future,
            _ => unreachable!("selection claimed only an idle or waiting decision"),
        };
        if self.0.cancelled.load(Ordering::Acquire) {
            return Err(());
        }
        let waker = self
            .0
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Waker::noop().clone());
        let decision = future.as_mut().poll(&mut Context::from_waker(&waker));
        match decision {
            Poll::Pending => {
                let mut future = Some(future);
                let parked = {
                    let mut state = self
                        .0
                        .selection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if matches!(*state, SelectionState::Polling)
                        && !self.0.cancelled.load(Ordering::Acquire)
                    {
                        *state = SelectionState::Waiting(future.take().expect("polled future"));
                        self.0
                            .capture
                            .authentication_detail("Client certificate", "Waiting for decision");
                        true
                    } else {
                        false
                    }
                };
                // If cancellation won while polling, cleanup still runs unlocked.
                drop(future);
                if parked { Ok(None) } else { Err(()) }
            }
            Poll::Ready(decision) => {
                // Cleanup can cancel a ready certificate decision too. Check the
                // cancellation flag only after destroying the completed future.
                drop(future);
                if self.0.resolver.generation() != self.0.context.generation {
                    self.cancel("authorization changed");
                    return Err(());
                }
                let mut state = self
                    .0
                    .selection
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if self.0.cancelled.load(Ordering::Acquire)
                    || !matches!(*state, SelectionState::Polling)
                {
                    return Err(());
                }
                *state = SelectionState::Finished;
                Ok(Some(decision))
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
                if credential.public_compatible(request) =>
            {
                let Some(_) = credential.chain.first() else {
                    return SelectionResult::Failed;
                };
                // Preserve the complete local path for issuer matching and
                // monitoring. Like Chromium, omit only a verified terminal root
                // from the wire; self-issued intermediates and the leaf remain.
                let chain = credential.chain.as_slice();
                let chain = match chain.split_last() {
                    Some((root, path))
                        if !path.is_empty() && transmog_tls_native::is_self_signed_ca(root) =>
                    {
                        path
                    }
                    _ => chain,
                };
                if transmog_tls_native::set_signing_algorithms(
                    ssl,
                    &credential.signing_algorithms(request),
                )
                .is_err()
                {
                    return SelectionResult::Failed;
                }
                let signer = Arc::new(BackgroundSigner {
                    key: credential.key.clone(),
                    result: Arc::new(Mutex::new(None)),
                    waker: self.0.clone(),
                });
                if transmog_tls_native::set_client_chain_and_private_key_method(ssl, chain, signer)
                    .is_err()
                {
                    return SelectionResult::Failed;
                }
                *self
                    .0
                    .selected_identity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(credential.id.clone());
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
            // A provider panic must publish failure and wake the handshake just
            // like an ordinary error. Never expose its panic payload as evidence.
            let signed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if !waker.cancelled.load(Ordering::Acquire)
                    && key.available()
                    && waker.resolver.generation() == waker.context.generation
                    // Provider checks may block or reenter cancellation. Admit
                    // signing only after those checks have returned.
                    && !waker.cancelled.load(Ordering::Acquire)
                {
                    key.sign(algorithm, &input)
                } else {
                    Err("selected identity is no longer authorized or available".into())
                }
            }))
            .unwrap_or_else(|_| Err("client certificate signing provider failed".into()));
            *result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(signed);
            let waker = waker
                .waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(waker) = waker {
                waker.wake();
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

#[cfg(test)]
mod tests {
    use super::*;

    struct PendingResolver;
    impl ClientAuthenticationResolver for PendingResolver {
        fn generation(&self) -> u64 {
            1
        }
        fn resolve(
            &self,
            _: ClientAuthenticationContext,
            _: CertificateRequest,
        ) -> ClientAuthenticationFuture {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    fn cancellation_releases_the_task_wake_handle_and_rejects_later_registration() {
        struct Task(std::sync::atomic::AtomicUsize);
        impl std::task::Wake for Task {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let task = Arc::new(Task(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(task.clone());
        let attempt = ClientAuthenticationAttempt::new(
            Arc::new(PendingResolver),
            ClientAuthenticationContext {
                host: "localhost".into(),
                port: 443,
                caller: "test".into(),
                generation: 1,
            },
            crate::TlsCapture::new(transmog_core::TlsEvidence::default()),
        );
        attempt.register(&waker);
        assert_eq!(Arc::strong_count(&task), 3);
        attempt.cancel("owner ended");
        assert_eq!(task.0.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&task), 2);
        attempt.register(&waker);
        assert_eq!(Arc::strong_count(&task), 2);
    }

    #[tokio::test]
    async fn waiting_selection_does_not_keep_its_owner_alive() {
        let attempt = ClientAuthenticationAttempt::new(
            Arc::new(PendingResolver),
            ClientAuthenticationContext {
                host: "localhost".into(),
                port: 443,
                caller: "test".into(),
                generation: 1,
            },
            crate::TlsCapture::new(transmog_core::TlsEvidence::default()),
        );
        let state = Arc::downgrade(&attempt.0);
        assert!(
            attempt
                .poll_decision(&CertificateRequest::default())
                .unwrap()
                .is_none()
        );
        drop(attempt);
        assert!(
            state.upgrade().is_none(),
            "pending selection retained its abandoned owner"
        );
    }
}
