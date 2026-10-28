use super::*;

fn input(name: &str) -> GrantInput {
    GrantInput {
        name: name.into(),
        permissions: BTreeSet::from([Permission::Inspect]),
        scope: Scope::All,
        expires_at: None,
    }
}

#[test]
fn independent_callers_must_represent_their_own_approved_secret() {
    let manager = AccessManager::open(None).unwrap();
    let (a, key_a) = manager.create(input("Codex")).unwrap();
    let (b, key_b) = manager.create(input("Copilot")).unwrap();
    assert!(manager.authenticate(key_a.expose()).is_err());
    manager.set_enabled(true).unwrap();
    assert_eq!(manager.authenticate(key_a.expose()).unwrap().id, a.id);
    assert_eq!(manager.authenticate(key_b.expose()).unwrap().id, b.id);
    assert!(manager.authenticate("Codex").is_err());
    assert!(manager.authenticate(&a.id).is_err());
    assert!(manager.authenticate("").is_err());
    manager.revoke(Some((&a.id, a.revision))).unwrap();
    assert!(manager.authenticate(key_a.expose()).is_err());
    assert_eq!(manager.authenticate(key_b.expose()).unwrap().id, b.id);
    assert!(!format!("{key_b:?}").contains(key_b.expose()));
    assert!(
        !serde_json::to_string(&manager.status())
            .unwrap()
            .contains(key_b.expose())
    );
}

#[test]
fn restart_retains_enable_permissions_and_revocation_without_plaintext_secrets() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    let (a, key_a) = manager.create(input("first")).unwrap();
    let (_, key_b) = manager.create(input("second")).unwrap();
    manager.set_enabled(true).unwrap();
    manager.revoke(Some((&a.id, a.revision))).unwrap();
    manager.authenticate(key_b.expose()).unwrap();
    manager.flush().unwrap();
    let bytes = std::fs::read_to_string(&path).unwrap();
    assert!(!bytes.contains(key_a.expose()));
    assert!(!bytes.contains(key_b.expose()));
    drop(manager);
    let manager = AccessManager::open(Some(path)).unwrap();
    assert!(manager.authenticate(key_a.expose()).is_err());
    assert!(manager.authenticate(key_b.expose()).is_ok());
    manager.set_enabled(false).unwrap();
    assert!(manager.authenticate(key_b.expose()).is_err());
    manager.set_enabled(true).unwrap();
    assert!(manager.authenticate(key_b.expose()).is_ok());
}

#[test]
fn corrupt_latest_state_never_resurrects_a_grant_or_overwrites_evidence() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    std::fs::write(&path, b"broken").unwrap();
    assert!(AccessManager::open(Some(path.clone())).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"broken");
}

#[test]
fn stale_edits_cannot_reenable_revoked_keys() {
    let manager = AccessManager::open(None).unwrap();
    let (grant, _) = manager.create(input("first")).unwrap();
    manager.revoke(Some((&grant.id, 1))).unwrap();
    assert!(manager.update(&grant.id, 1, input("stale")).is_err());
    assert!(manager.update(&grant.id, 2, input("also stale")).is_err());
    assert!(manager.status().grants[0].revoked);
}

#[test]
fn grants_expire_and_scopes_do_not_expand_to_other_hosts_or_instances() {
    let manager = AccessManager::open(None).unwrap();
    let mut config = input("host");
    config.scope = Scope::Hosts {
        hosts: vec!["Example.COM".into()],
    };
    config.expires_at = Some(now() + 100);
    let (_, key) = manager.create(config).unwrap();
    manager.set_enabled(true).unwrap();
    let grant = manager.authenticate(key.expose()).unwrap();
    assert!(grant.scope.allows_url("https://example.com:8443/test"));
    assert!(!grant.scope.allows_url("https://example.com.evil.test/"));
    assert!(!grant.scope.allows_url("file://example.com/path"));
    assert!(grant.require(Permission::Modify).is_err());
    manager.lock().records[0].grant.expires_at = Some(now() - 1);
    assert!(manager.authenticate(key.expose()).is_err());
    let scope = Scope::Requests {
        instance_id: "a".repeat(32),
        ids: vec!["42".into()],
    };
    assert!(scope.allows_entry(&"a".repeat(32), "42", "https://example.com/"));
    assert!(!scope.allows_entry(&"b".repeat(32), "42", "https://example.com/"));
    assert!(!scope.allows_url("https://example.com/"));
}

#[test]
fn failed_persistence_does_not_publish_a_new_authority() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(manager.create(input("unsaved")).is_err());
    assert!(manager.set_enabled(true).is_err());
    assert!(!manager.status().enabled);
    assert!(manager.status().grants.is_empty());
}

#[test]
fn host_scope_rejects_ports_wildcards_and_url_syntax() {
    let manager = AccessManager::open(None).unwrap();
    for host in [
        "https://example.com",
        "example.com:443",
        "*.example.com",
        "a@b",
        "example.com/path",
        "example.com?x",
    ] {
        let mut config = input("invalid host");
        config.scope = Scope::Hosts {
            hosts: vec![host.into()],
        };
        assert!(manager.create(config).is_err(), "accepted {host}");
    }
    let mut config = input("loopback");
    config.scope = Scope::Hosts {
        hosts: vec!["[::1]".into(), "127.0.0.1".into()],
    };
    let (grant, _) = manager.create(config).unwrap();
    assert!(grant.scope.allows_url("http://[::1]:3000/"));
    assert!(grant.scope.allows_url("http://127.0.0.1:3000/"));
    assert!(!grant.scope.allows_url("http://localhost:3000/"));
}

#[test]
fn authority_changes_notify_connected_consumers_and_last_use_does_not() {
    let manager = AccessManager::open(None).unwrap();
    let mut changed = manager.subscribe();
    let (grant, key) = manager.create(input("client")).unwrap();
    assert!(changed.has_changed().unwrap());
    changed.borrow_and_update();
    manager.set_enabled(true).unwrap();
    changed.borrow_and_update();
    manager.authenticate(key.expose()).unwrap();
    assert!(!changed.has_changed().unwrap());
    manager.revoke(Some((&grant.id, grant.revision))).unwrap();
    assert!(changed.has_changed().unwrap());
}
