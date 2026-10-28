use super::*;
use crate::{GrantInput, Permission, Scope};
use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Echo(Arc<AtomicUsize>);
impl Handler for Echo {
    fn call(&self, grant: Grant, operation: String, input: Value) -> CallFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if operation == "slow" {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            grant.require(Permission::Inspect)?;
            Ok(serde_json::json!({"grant":grant.id,"input":input}))
        })
    }
}

fn input(name: &str) -> GrantInput {
    GrantInput {
        omit_sensitive_values: false,
        name: name.into(),
        permissions: BTreeSet::from([Permission::Inspect]),
        scope: Scope::All,
        expires_at: None,
    }
}

#[test]
fn corrupt_discovery_is_unavailable_rather_than_an_auto_launch_signal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(DISCOVERY_FILE), "not endpoint JSON").unwrap();
    assert_eq!(
        LocalClient::new(root.path().into())
            .endpoint()
            .unwrap_err()
            .code,
        "unavailable"
    );
}

#[tokio::test]
async fn loopback_calls_require_each_callers_key_and_recheck_revocation() {
    let root = tempfile::tempdir().unwrap();
    let manager = AccessManager::open(None).unwrap();
    let (a, key_a) = manager.create(input("Codex")).unwrap();
    let (b, key_b) = manager.create(input("Copilot")).unwrap();
    manager.set_enabled(true).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let server = LocalServer::start(
        root.path().into(),
        manager.clone(),
        Arc::new(Echo(calls.clone())),
    )
    .await
    .unwrap();
    assert_eq!(
        server.endpoint().address.ip(),
        std::net::Ipv4Addr::LOCALHOST
    );
    let client = LocalClient::new(root.path().into());
    assert!(client.call("", "echo", Value::Null).await.is_err());
    assert_eq!(manager.active_agents().len(), 0);
    assert_eq!(
        client
            .call(key_a.expose(), "echo", Value::Null)
            .await
            .unwrap()["grant"],
        a.id
    );
    assert_eq!(
        client
            .call(key_b.expose(), "echo", Value::Null)
            .await
            .unwrap()["grant"],
        b.id
    );
    assert!(client.call("", "echo", Value::Null).await.is_err());
    assert!(client.call(&a.id, "echo", Value::Null).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let agents = manager.active_agents();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["name"], "Codex");
    assert_eq!(agents[1]["name"], "Copilot");
    manager.revoke(Some((&a.id, a.revision))).unwrap();
    assert_eq!(manager.active_agents().len(), 1);
    assert!(
        client
            .call(key_a.expose(), "echo", Value::Null)
            .await
            .is_err()
    );
    assert!(
        client
            .call(key_b.expose(), "echo", Value::Null)
            .await
            .is_ok()
    );
    drop(server);
    assert_eq!(manager.active_agents().len(), 0);
    assert!(!root.path().join(DISCOVERY_FILE).exists());
}

#[tokio::test]
async fn certificate_mismatch_is_rejected_before_sending_credentials_or_dispatching() {
    let root = tempfile::tempdir().unwrap();
    let manager = AccessManager::open(None).unwrap();
    let (_, key) = manager.create(input("client")).unwrap();
    manager.set_enabled(true).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let server = LocalServer::start(root.path().into(), manager, Arc::new(Echo(calls.clone())))
        .await
        .unwrap();
    let mut endpoint = server.endpoint().clone();
    endpoint.certificate_sha256[0] ^= 1;
    publish(root.path(), &endpoint).unwrap();
    let client = LocalClient::new(root.path().into());
    assert!(
        client
            .call(key.expose(), "echo", Value::Null)
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    endpoint.address = "192.168.1.1:1234".parse().unwrap();
    publish(root.path(), &endpoint).unwrap();
    assert!(client.endpoint().is_err());
}

#[tokio::test]
async fn revoke_during_a_read_cancels_it_without_publishing_its_result() {
    let root = tempfile::tempdir().unwrap();
    let manager = AccessManager::open(None).unwrap();
    let (grant, key) = manager.create(input("client")).unwrap();
    manager.set_enabled(true).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let _server = LocalServer::start(
        root.path().into(),
        manager.clone(),
        Arc::new(Echo(calls.clone())),
    )
    .await
    .unwrap();
    let client = LocalClient::new(root.path().into());
    let task = tokio::spawn(async move { client.call(key.expose(), "slow", Value::Null).await });
    timeout(Duration::from_secs(3), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.revoke(Some((&grant.id, grant.revision))).unwrap();
    let result = timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn revoking_disclosure_cancels_an_inflight_read_even_without_a_key_revision_change() {
    let root = tempfile::tempdir().unwrap();
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let mut protected = input("Codex");
    protected.omit_sensitive_values = true;
    let (grant, key) = manager.create(protected).unwrap();
    let pending = manager
        .request_sensitive_access(&grant, "Inspect authentication values.")
        .unwrap();
    let id = pending["request"]["id"].as_str().unwrap();
    manager.decide_sensitive_access(id, "minute").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let _server = LocalServer::start(
        root.path().into(),
        manager.clone(),
        Arc::new(Echo(calls.clone())),
    )
    .await
    .unwrap();
    let client = LocalClient::new(root.path().into());
    let work = tokio::spawn(async move { client.call(key.expose(), "slow", Value::Null).await });
    timeout(Duration::from_secs(3), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.revoke_sensitive_access(id).unwrap();
    assert_eq!(manager.status().grants[0].revision, grant.revision);
    assert!(
        timeout(Duration::from_secs(2), work)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn restart_accepts_a_persistent_key_but_changes_endpoint_generation() {
    let root = tempfile::tempdir().unwrap();
    let manager = AccessManager::open(Some(root.path().join("access.json"))).unwrap();
    let (_, key) = manager.create(input("client")).unwrap();
    manager.set_enabled(true).unwrap();
    let handler: Arc<dyn Handler> = Arc::new(Echo(Arc::new(AtomicUsize::new(0))));
    let first = LocalServer::start(root.path().into(), manager, handler.clone())
        .await
        .unwrap();
    let instance = first.endpoint().instance_id.clone();
    drop(first);
    let manager = AccessManager::open(Some(root.path().join("access.json"))).unwrap();
    let second = LocalServer::start(root.path().into(), manager, handler)
        .await
        .unwrap();
    assert_ne!(instance, second.endpoint().instance_id);
    assert!(
        LocalClient::new(root.path().into())
            .call(key.expose(), "echo", Value::Null)
            .await
            .is_ok()
    );
}
