use super::*;
use crate::GrantInput;

fn input(name: &str) -> GrantInput {
    serde_json::from_value(json!({"name":name,"permissions":["inspect"],"scope":{"kind":"all"}}))
        .unwrap()
}

fn expire_quiet_period(manager: &AccessManager) {
    for activity in manager.lock().activity.values_mut() {
        activity.last_finished = Instant::now()
            .checked_sub(QUIET_PERIOD + Duration::from_millis(1))
            .unwrap();
    }
}

#[test]
fn brief_calls_stay_visible_without_authority_changes_and_long_calls_do_not_expire() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (a, _) = manager.create(input("Codex")).unwrap();
    let (b, _) = manager.create(input("Copilot")).unwrap();
    let authority = manager.subscribe();
    let mut activity = manager.subscribe_activity();
    assert_eq!(manager.active_agents().len(), 0);
    let first = manager.begin_call(&a);
    assert!(activity.has_changed().unwrap());
    activity.borrow_and_update();
    let second = manager.begin_call(&a);
    assert_eq!(manager.active_agents().len(), 1);
    assert!(!activity.has_changed().unwrap());
    drop(first);
    expire_quiet_period(&manager);
    assert_eq!(manager.active_agents()[0]["name"], "Codex");
    drop(second);
    assert_eq!(manager.active_agents().len(), 1);
    let rapid = manager.begin_call(&a);
    assert_eq!(manager.active_agents().len(), 1);
    assert!(!activity.has_changed().unwrap());
    drop(rapid);
    let third = manager.begin_call(&b);
    assert_eq!(manager.active_agents().len(), 2);
    drop(third);
    assert!(!authority.has_changed().unwrap());
    expire_quiet_period(&manager);
    assert_eq!(manager.active_agents().len(), 0);
}

#[test]
fn revocation_master_off_and_listener_restart_clear_presence_without_a_grace_period() {
    let manager = AccessManager::open(None).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, _) = manager.create(input("Codex")).unwrap();
    let old = manager.begin_call(&grant);
    manager.clear_activity();
    assert_eq!(manager.active_agents().len(), 0);
    let current = manager.begin_call(&grant);
    // Completion of an old listener's call must not decrement a new listener's count.
    drop(old);
    expire_quiet_period(&manager);
    assert_eq!(manager.active_agents().len(), 1);
    manager.set_enabled(false).unwrap();
    assert_eq!(manager.active_agents().len(), 0);
    manager.set_enabled(true).unwrap();
    assert_eq!(manager.active_agents().len(), 0);
    drop(current);
    let current = manager.begin_call(&grant);
    manager.revoke(Some((&grant.id, grant.revision))).unwrap();
    assert_eq!(manager.active_agents().len(), 0);
    drop(current);
    assert_eq!(manager.active_agents().len(), 0);
}

#[test]
fn presence_does_not_survive_restart_or_an_expired_or_replaced_key() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("access.json");
    let manager = AccessManager::open(Some(path.clone())).unwrap();
    manager.set_enabled(true).unwrap();
    let (grant, _) = manager.create(input("Codex")).unwrap();
    let current = manager.begin_call(&grant);
    manager.flush().unwrap();
    assert_eq!(
        AccessManager::open(Some(path))
            .unwrap()
            .active_agents()
            .len(),
        0
    );
    manager.replace(&grant.id, grant.revision).unwrap();
    assert_eq!(manager.active_agents().len(), 0);
    drop(current);
    let grant = manager.status().grants.remove(0);
    let current = manager.begin_call(&grant);
    manager.lock().records[0].grant.expires_at = Some(now());
    assert_eq!(manager.active_agents().len(), 0);
    drop(current);
}
