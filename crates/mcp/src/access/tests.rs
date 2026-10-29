use super::*;

#[cfg(windows)]
#[test]
fn protected_configuration_reopens_after_restart_and_never_leaks_through_status() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    let mut config = input("Agent");
    config
        .permissions
        .extend([Permission::ReadBodies, Permission::ReadCredentials]);
    let (grant, key) = manager.create(config).unwrap();
    assert!(boring::memcmp::eq(
        manager
            .configuration_key(&grant.id, grant.revision)
            .unwrap()
            .expose()
            .as_bytes(),
        key.expose().as_bytes()
    ));
    let bytes = std::fs::read_to_string(&path).unwrap();
    assert!(!bytes.contains(key.expose()));
    assert!(
        !serde_json::to_string(&manager.status())
            .unwrap()
            .contains("protectedKey")
    );
    let reloaded = AccessManager::open(Some(path)).unwrap();
    assert!(boring::memcmp::eq(
        reloaded
            .configuration_key(&grant.id, grant.revision)
            .unwrap()
            .expose()
            .as_bytes(),
        key.expose().as_bytes()
    ));
    let new_key = reloaded.replace(&grant.id, grant.revision).unwrap();
    assert!(
        reloaded
            .configuration_key(&grant.id, grant.revision)
            .is_err()
    );
    assert!(boring::memcmp::eq(
        reloaded
            .configuration_key(&grant.id, grant.revision + 1)
            .unwrap()
            .expose()
            .as_bytes(),
        new_key.expose().as_bytes()
    ));
    reloaded
        .revoke(Some((&grant.id, grant.revision + 1)))
        .unwrap();
    assert!(
        reloaded
            .configuration_key(&grant.id, grant.revision + 2)
            .is_err()
    );
    assert!(reloaded.lock().records[0].protected_key.is_none());
}

#[test]
fn legacy_configuration_requires_explicit_replacement_without_changing_authority() {
    let manager = AccessManager::open(None).unwrap();
    let (grant, _) = manager.create(input("Legacy agent")).unwrap();
    manager.lock().records[0].protected_key = None;
    assert_eq!(
        manager
            .configuration_key(&grant.id, grant.revision)
            .unwrap_err()
            .code,
        "configuration_unavailable"
    );
    assert_eq!(manager.status().grants[0].revision, grant.revision);
}

#[test]
fn restart_migrates_partial_sensitive_grants_to_redaction_without_expanding_scope() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    let mut config = input("Partial legacy access");
    config.permissions.insert(Permission::ReadBodies);
    config.scope = Scope::Hosts {
        hosts: vec!["example.test".into()],
    };
    let (grant, key) = manager.create(config).unwrap();
    manager.set_enabled(true).unwrap();
    let loaded = AccessManager::open(Some(path.clone())).unwrap();
    let current = loaded.authenticate(key.expose()).unwrap();
    assert!(current.omit_sensitive_values);
    assert!(!current.allows(Permission::ReadBodies));
    assert!(!current.allows(Permission::ReadCredentials));
    assert_eq!(current.scope, grant.scope);
    assert_eq!(current.revision, grant.revision + 1);
    assert_eq!(
        AccessManager::open(Some(path)).unwrap().status().grants[0].revision,
        current.revision
    );
}

#[cfg(windows)]
#[test]
fn configuration_rejects_another_keys_protected_blob_or_expired_grant() {
    let manager = AccessManager::open(None).unwrap();
    let (grant, _) = manager.create(input("First")).unwrap();
    manager.create(input("Second")).unwrap();
    {
        let mut store = manager.lock();
        store.records[0].protected_key = store.records[1].protected_key.clone();
    }
    assert!(
        manager
            .configuration_key(&grant.id, grant.revision)
            .is_err()
    );
    manager.lock().records[0].grant.expires_at = Some(now());
    assert!(
        manager
            .configuration_key(&grant.id, grant.revision)
            .is_err()
    );
}

#[test]
fn replacement_invalidates_old_secret_and_preserves_named_grant() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, old_key) = manager.create(input("Codex")).unwrap();
    let new_key = manager.replace(&grant.id, grant.revision).unwrap();
    assert!(manager.authenticate(old_key.expose()).is_err());
    let replaced = manager.authenticate(new_key.expose()).unwrap();
    assert_eq!(replaced.id, grant.id);
    assert_eq!(replaced.name, grant.name);
    assert!(replaced.revision > grant.revision);
    assert!(manager.replace(&grant.id, grant.revision).is_err());
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
