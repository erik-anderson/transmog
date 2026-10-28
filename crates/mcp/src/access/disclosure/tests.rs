use super::*;

fn input() -> GrantInput {
    serde_json::from_value(json!({"name":"Codex","permissions":["inspect","read-bodies","read-credentials"],"scope":{"kind":"hosts","hosts":["example.test"]},"expiresAt":null})).unwrap()
}
fn request(manager: &AccessManager, grant: &Grant) -> String {
    manager
        .request_sensitive_access(
            grant,
            "Inspect the authentication failure for this request.",
        )
        .unwrap()["request"]["id"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn default_omission_blocks_even_preselected_sensitive_permissions_without_hiding_rules() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let input = input();
    assert!(input.omit_sensitive_values);
    let (grant, key) = manager.create(input).unwrap();
    assert!(!grant.allows(Permission::ReadBodies));
    let current = manager.authenticate(key.expose()).unwrap();
    assert!(current.allows(Permission::Inspect));
    assert!(!current.allows(Permission::ReadBodies));
    assert!(!current.allows(Permission::ReadCredentials));
    let first = request(&manager, &grant);
    assert_eq!(first, request(&manager, &grant));
    assert!(!manager.has_sensitive_access(&grant.id, grant.revision));
    assert!(
        manager
            .decide_sensitive_access(&first, "unsupported")
            .is_err()
    );
    assert_eq!(manager.sensitive_access_requests().len(), 1);
}

#[test]
fn minute_approval_is_key_scoped_monotonic_and_not_durable() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, key) = manager.create(input()).unwrap();
    let (peer, _) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    manager.decide_sensitive_access(&id, "minute").unwrap();
    let approved = manager.authenticate(key.expose()).unwrap();
    assert!(approved.allows(Permission::ReadBodies));
    assert!(approved.allows(Permission::ReadCredentials));
    assert_eq!(approved.revision, grant.revision);
    assert_eq!(approved.scope, grant.scope);
    assert!(!approved.allows(Permission::Modify));
    assert!(!manager.has_sensitive_access(&peer.id, peer.revision));
    let view = manager.sensitive_access_requests();
    assert_eq!(view[0]["remainingSeconds"], 60);
    manager.flush().unwrap();
    let reloaded = AccessManager::open(Some(path)).unwrap();
    assert!(
        !reloaded
            .authenticate(key.expose())
            .unwrap()
            .allows(Permission::ReadBodies)
    );
    assert_eq!(reloaded.sensitive_access_requests(), Vec::<Value>::new());
    manager
        .lock()
        .disclosures
        .get_mut(&grant.id)
        .unwrap()
        .deadline = Instant::now();
    let expired = manager.authenticate(key.expose()).unwrap();
    assert!(!expired.allows(Permission::ReadBodies));
    assert!(!expired.same_authority(&approved));
    assert!(manager.status().grants[0].omit_sensitive_values);
}

#[test]
fn early_revocation_invalidates_an_admitted_authority_without_revoking_the_key() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, key) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    manager.decide_sensitive_access(&id, "minute").unwrap();
    let approved = manager.authenticate(key.expose()).unwrap();
    manager.revoke_sensitive_access(&id).unwrap();
    let current = manager.authenticate(key.expose()).unwrap();
    assert!(!approved.same_authority(&current));
    assert!(current.allows(Permission::Inspect));
    assert!(!current.allows(Permission::ReadBodies));
    assert!(manager.revoke_sensitive_access(&id).is_err());
}

#[test]
fn always_persists_the_setting_and_read_permissions_but_not_other_capabilities_or_scope() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    manager.set_enabled(true).unwrap();
    let mut minimal = input();
    minimal.permissions = BTreeSet::from([Permission::Inspect]);
    let (grant, key) = manager.create(minimal).unwrap();
    let id = request(&manager, &grant);
    manager.decide_sensitive_access(&id, "always").unwrap();
    let restarted = AccessManager::open(Some(path)).unwrap();
    let current = restarted.authenticate(key.expose()).unwrap();
    assert!(!current.omit_sensitive_values);
    assert!(current.revision > grant.revision);
    assert_eq!(current.scope, grant.scope);
    assert!(current.allows(Permission::ReadBodies));
    assert!(current.allows(Permission::ReadCredentials));
    assert!(!current.allows(Permission::Send));
    assert!(manager.decide_sensitive_access(&id, "always").is_err());
    let mut protected = input();
    protected.permissions = current.permissions.clone();
    restarted
        .update(&current.id, current.revision, protected)
        .unwrap();
    assert!(
        !restarted
            .authenticate(key.expose())
            .unwrap()
            .allows(Permission::ReadBodies)
    );
}

#[test]
fn denied_expired_and_replaced_requests_never_allow_stale_approval() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, _) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    manager.decide_sensitive_access(&id, "deny").unwrap();
    assert_eq!(
        manager
            .request_sensitive_access(&grant, "Try again")
            .unwrap_err()
            .code,
        "request_denied"
    );
    assert!(manager.decide_sensitive_access(&id, "minute").is_err());
    manager
        .lock()
        .disclosures
        .get_mut(&grant.id)
        .unwrap()
        .deadline = Instant::now();
    let next = request(&manager, &grant);
    assert_ne!(next, id);
    manager.replace(&grant.id, grant.revision).unwrap();
    assert!(manager.decide_sensitive_access(&next, "always").is_err());
}

#[test]
fn master_off_ends_temporary_access_and_pending_requests_without_resuming_them() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, key) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    manager.decide_sensitive_access(&id, "minute").unwrap();
    manager.set_enabled(false).unwrap();
    manager.set_enabled(true).unwrap();
    assert!(
        !manager
            .authenticate(key.expose())
            .unwrap()
            .allows(Permission::ReadBodies)
    );
    assert_eq!(manager.sensitive_access_requests(), Vec::<Value>::new());
}

#[test]
fn expired_pending_requests_cannot_be_approved_and_status_never_reprompts() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, _) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    manager
        .lock()
        .disclosures
        .get_mut(&grant.id)
        .unwrap()
        .deadline = Instant::now();
    assert!(manager.decide_sensitive_access(&id, "minute").is_err());
    assert_eq!(
        manager.sensitive_access_status(&grant).unwrap()["request"]["state"],
        "expired"
    );
    assert_eq!(manager.sensitive_access_requests(), Vec::<Value>::new());
}

#[test]
fn failed_permanent_approval_keeps_omission_and_the_pending_request() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, key) = manager.create(input()).unwrap();
    let id = request(&manager, &grant);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(manager.decide_sensitive_access(&id, "always").is_err());
    assert!(
        !manager
            .authenticate(key.expose())
            .unwrap()
            .allows(Permission::ReadBodies)
    );
    assert_eq!(manager.sensitive_access_requests()[0]["state"], "pending");
}

#[test]
fn preexisting_grants_without_the_setting_load_with_omission_enabled() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    manager.set_enabled(true).unwrap();
    let mut legacy = input();
    legacy.omit_sensitive_values = false;
    let (_, key) = manager.create(legacy).unwrap();
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["records"][0]["grant"]
        .as_object_mut()
        .unwrap()
        .remove("omitSensitiveValues");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let reopened = AccessManager::open(Some(path)).unwrap();
    assert!(
        !reopened
            .authenticate(key.expose())
            .unwrap()
            .allows(Permission::ReadBodies)
    );
}
