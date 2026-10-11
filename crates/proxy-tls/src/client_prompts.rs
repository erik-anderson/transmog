//! Coalesced transient decisions, scoped to caller, origin, epoch and actual request.
use crate::client_authentication::{
    CertificateRequest, ClientAuthenticationContext, ClientAuthenticationDecision,
    ClientAuthenticationFuture, ClientAuthenticationPrompt, ClientCredential,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{oneshot, watch};

/// Public choice without a private provider or native key reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientCertificateChoice {
    /// Stable public identity identifier.
    pub id: String,
    /// SHA-256 leaf fingerprint.
    pub fingerprint: String,
    /// Bounded public leaf subject for human selection.
    pub subject: String,
    /// Bounded public leaf issuer.
    pub issuer: String,
    /// Public certificate expiration.
    pub valid_until: String,
}
/// One transient decision displayed by an embedding controller.
#[derive(Clone, Debug)]
pub struct PendingClientCertificateRequest {
    /// Nonreused broker identifier.
    pub id: u64,
    /// Immutable caller and origin authorization.
    pub context: ClientAuthenticationContext,
    /// Actual verified server constraints.
    pub request: CertificateRequest,
    /// Eligible public identities.
    pub choices: Vec<ClientCertificateChoice>,
    /// Bounded generic inventory status.
    pub warning: Option<String>,
    /// Physical handshakes waiting for this same scoped decision.
    pub connections: usize,
}
/// Explicit connection-scoped response; it never silently changes saved rules.
pub enum ClientCertificateAnswer {
    /// Select an eligible identity by exact public identifier.
    Identity(String),
    /// Try the same handshake without a certificate.
    NoCertificate,
    /// Close these handshakes.
    Cancel,
}
struct Entry {
    public: PendingClientCertificateRequest,
    candidates: Vec<Arc<ClientCredential>>,
    waiters: BTreeMap<u64, oneshot::Sender<ClientAuthenticationDecision>>,
    bytes: usize,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<u64, Entry>,
    next: u64,
    closed: bool,
    bytes: usize,
}
struct Inner {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
}
/// Bounded queue shared with a nonmodal controller. Dropped owners unregister themselves.
#[derive(Clone)]
pub struct ClientCertificateBroker(Arc<Inner>);
impl Default for ClientCertificateBroker {
    fn default() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            changes: watch::channel(0).0,
        }))
    }
}
impl ClientCertificateBroker {
    /// Reads only public choices and immutable connection context.
    pub fn pending(&self) -> Vec<PendingClientCertificateRequest> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .values()
            .map(|entry| {
                let mut public = entry.public.clone();
                public.connections = entry.waiters.len();
                public
            })
            .collect()
    }
    /// Subscribes to queue changes; consumers reread a complete bounded snapshot.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changes.subscribe()
    }
    /// Applies an explicit choice if the transient request is still present.
    /// Returns false for an expired identifier or an ineligible/ambiguous identity.
    pub fn answer(&self, id: u64, answer: ClientCertificateAnswer) -> bool {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.entries.get(&id) else {
            return false;
        };
        let decision = match answer {
            ClientCertificateAnswer::Identity(id) => {
                let mut matches = entry.candidates.iter().filter(|c| c.id == id);
                let (Some(credential), None) = (matches.next(), matches.next()) else {
                    return false;
                };
                ClientAuthenticationDecision::Certificate(credential.clone())
            }
            ClientCertificateAnswer::NoCertificate => {
                ClientAuthenticationDecision::NoCertificate("explicit user decision".into())
            }
            ClientCertificateAnswer::Cancel => {
                ClientAuthenticationDecision::Cancelled("explicit user decision".into())
            }
        };
        let Some(entry) = state.entries.remove(&id) else {
            return false;
        };
        state.bytes -= entry.bytes;
        drop(state);
        for (_, waiter) in entry.waiters {
            let _ = waiter.send(decision.clone());
        }
        self.notify();
        true
    }
    /// Cancels all pending decisions and rejects new ones for this controller run.
    pub fn shutdown(&self) {
        self.clear(true);
    }
    /// Cancels current decisions while permitting later connections in this run.
    pub fn cancel_pending(&self) {
        self.clear(false);
    }
    fn clear(&self, close: bool) {
        let entries = {
            let mut state = self
                .0
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed |= close;
            state.bytes = 0;
            std::mem::take(&mut state.entries)
        };
        drop(entries);
        self.notify();
    }
    fn notify(&self) {
        self.0.changes.send_modify(|n| *n = n.wrapping_add(1));
    }
}
struct Waiter {
    broker: Weak<Inner>,
    request: u64,
    id: u64,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        let Some(broker) = self.broker.upgrade() else {
            return;
        };
        let (removed, removed_waiter) = {
            let mut state = broker
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut removed_waiter = None;
            let empty = state.entries.get_mut(&self.request).is_some_and(|entry| {
                removed_waiter = entry.waiters.remove(&self.id);
                entry.waiters.is_empty()
            });
            let removed = if empty {
                state.entries.remove(&self.request)
            } else {
                None
            };
            if let Some(entry) = &removed {
                state.bytes -= entry.bytes;
            }
            (removed, removed_waiter)
        };
        drop(removed);
        drop(removed_waiter);
        broker.changes.send_modify(|n| *n = n.wrapping_add(1));
    }
}
impl ClientAuthenticationPrompt for ClientCertificateBroker {
    fn prompt(
        &self,
        context: ClientAuthenticationContext,
        request: CertificateRequest,
        candidates: Vec<Arc<ClientCredential>>,
        warning: Option<String>,
    ) -> ClientAuthenticationFuture {
        let broker = self.clone();
        Box::pin(async move {
            let mut candidates = Some(candidates);
            let Some((choices, bytes)) =
                prepare_choices(candidates.as_ref().expect("owned choices"))
            else {
                return ClientAuthenticationDecision::Cancelled(
                    "certificate decision capacity reached".into(),
                );
            };
            let (sender, receiver) = oneshot::channel();
            let (id, waiter) = {
                let mut state = broker
                    .0
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.closed || choices.len() > 1024 || state.next == u64::MAX {
                    return ClientAuthenticationDecision::Cancelled(
                        "certificate controller is unavailable".into(),
                    );
                }
                state.next += 1;
                let waiter = state.next;
                let existing = state
                    .entries
                    .iter()
                    .find(|(_, e)| {
                        e.public.context == context
                            && e.public.request == request
                            && e.public.choices == choices
                    })
                    .map(|(id, _)| *id);
                let id = if let Some(id) = existing {
                    if state.entries[&id].waiters.len() >= 64 {
                        return ClientAuthenticationDecision::Cancelled(
                            "certificate decision capacity reached".into(),
                        );
                    }
                    id
                } else {
                    if state.entries.len() >= 64 || state.bytes + bytes > 32 * 1024 * 1024 {
                        return ClientAuthenticationDecision::Cancelled(
                            "certificate decision capacity reached".into(),
                        );
                    }
                    state.bytes += bytes;
                    state.entries.insert(
                        waiter,
                        Entry {
                            public: PendingClientCertificateRequest {
                                id: waiter,
                                context,
                                request,
                                choices,
                                warning: warning.map(|s| s.chars().take(512).collect()),
                                connections: 0,
                            },
                            candidates: candidates.take().expect("owned choices"),
                            waiters: BTreeMap::new(),
                            bytes,
                        },
                    );
                    waiter
                };
                state
                    .entries
                    .get_mut(&id)
                    .expect("registered decision")
                    .waiters
                    .insert(waiter, sender);
                (id, waiter)
            };
            drop(candidates);
            let _owner = Waiter {
                broker: Arc::downgrade(&broker.0),
                request: id,
                id: waiter,
            };
            broker.notify();
            receiver.await.unwrap_or_else(|_| {
                ClientAuthenticationDecision::Cancelled("certificate decision ended".into())
            })
        })
    }
}

fn prepare_choices(
    candidates: &[Arc<ClientCredential>],
) -> Option<(Vec<ClientCertificateChoice>, usize)> {
    if candidates.len() > 1024 || candidates.iter().any(|c| c.id.len() > 4096) {
        return None;
    }
    let mut choices = candidates
        .iter()
        .map(|c| {
            Some(ClientCertificateChoice {
                id: c.id.clone(),
                fingerprint: c.fingerprint()?,
                subject: certificate_name(c.chain.first()?.subject_name()),
                issuer: certificate_name(c.chain.first()?.issuer_name()),
                valid_until: c.chain.first()?.not_after().to_string(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    choices.sort_by(|a, b| (&a.id, &a.fingerprint).cmp(&(&b.id, &b.fingerprint)));
    let bytes = candidates
        .iter()
        .flat_map(|c| &c.chain)
        .try_fold(0usize, |n, c| n.checked_add(c.to_der().ok()?.len()))?;
    (bytes <= 32 * 1024 * 1024).then_some((choices, bytes))
}

/// Formats a public certificate name without native provider metadata.
pub fn certificate_name(name: &boring::x509::X509NameRef) -> String {
    name.entries()
        .map(|entry| {
            let label = entry.object().nid().short_name().unwrap_or("Attribute");
            let value = entry
                .data()
                .as_utf8()
                .map(|value| value.to_string())
                .unwrap_or_default();
            format!("{label}={value}")
        })
        .collect::<Vec<_>>()
        .join(", ")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn context(caller: &str) -> ClientAuthenticationContext {
        ClientAuthenticationContext {
            caller: caller.into(),
            host: "localhost".into(),
            port: 443,
            generation: 1,
        }
    }
    async fn wait(broker: &ClientCertificateBroker, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if broker
                    .pending()
                    .iter()
                    .map(|p| p.connections)
                    .sum::<usize>()
                    == count
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn groups_only_identical_callers_origins_epochs_and_server_constraints() {
        let broker = ClientCertificateBroker::default();
        let mut contexts = vec![
            context("a"),
            context("b"),
            context("a"),
            context("a"),
            context("a"),
            context("a"),
        ];
        contexts[2].host = "other".into();
        contexts[3].port = 8443;
        contexts[4].generation = 2;
        let mut tasks = Vec::new();
        for (i, context) in contexts.into_iter().enumerate() {
            let request = CertificateRequest {
                signature_algorithms: if i == 5 { vec![0x0403] } else { vec![] },
                ..Default::default()
            };
            tasks.push(tokio::spawn(broker.prompt(context, request, vec![], None)));
        }
        wait(&broker, 6).await;
        assert_eq!(broker.pending().len(), 6);
        broker.shutdown();
        for task in tasks {
            assert!(matches!(
                task.await.unwrap(),
                ClientAuthenticationDecision::Cancelled(_)
            ));
        }
        assert!(matches!(
            broker
                .prompt(context("a"), CertificateRequest::default(), vec![], None)
                .await,
            ClientAuthenticationDecision::Cancelled(_)
        ));
    }
    #[tokio::test]
    async fn limits_groups_and_waiters_and_reclaims_capacity_on_owner_drop() {
        for grouped in [false, true] {
            let broker = ClientCertificateBroker::default();
            let mut tasks = Vec::new();
            for i in 0..64 {
                tasks.push(tokio::spawn(broker.prompt(
                    context(&if grouped {
                        "same".into()
                    } else {
                        i.to_string()
                    }),
                    CertificateRequest::default(),
                    vec![],
                    None,
                )));
            }
            wait(&broker, 64).await;
            assert!(matches!(
                broker
                    .prompt(
                        context(if grouped { "same" } else { "overflow" }),
                        CertificateRequest::default(),
                        vec![],
                        None
                    )
                    .await,
                ClientAuthenticationDecision::Cancelled(_)
            ));
            let stale = broker.pending()[0].id;
            for task in tasks {
                task.abort();
                let _ = task.await;
            }
            wait(&broker, 0).await;
            assert!(!broker.answer(stale, ClientCertificateAnswer::NoCertificate));
            let next = tokio::spawn(broker.prompt(
                context("same"),
                CertificateRequest::default(),
                vec![],
                None,
            ));
            wait(&broker, 1).await;
            let id = broker.pending()[0].id;
            assert!(id > stale);
            assert!(!broker.answer(id, ClientCertificateAnswer::Identity("missing".into())));
            assert!(broker.answer(id, ClientCertificateAnswer::NoCertificate));
            assert!(matches!(
                next.await.unwrap(),
                ClientAuthenticationDecision::NoCertificate(_)
            ));
        }
    }
}
