//! In-memory disclosure approvals never survive app restart or change a traffic scope.
use super::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(super) struct Disclosure {
    id: String,
    revision: u64,
    reason: String,
    phase: &'static str,
    requested_at: u64,
    expires_at: u64,
    deadline: Instant,
}

fn valid(state: &Store, grant: &Grant) -> bool {
    state.enabled && !grant.revoked && grant.expires_at.is_none_or(|expiry| expiry > now())
}

pub(super) fn prune(state: &mut Store) {
    state.disclosures.retain(|id, entry| {
        let Some(record) = state.records.iter().find(|record| record.grant.id == *id) else {
            return false;
        };
        if matches!(entry.phase, "pending" | "temporary") {
            if !valid_grant(state.enabled, &record.grant, entry.revision) {
                entry.phase = "cancelled";
            } else if entry.deadline <= Instant::now() {
                entry.phase = "expired";
            }
        }
        true
    });
}

fn valid_grant(enabled: bool, grant: &Grant, revision: u64) -> bool {
    enabled
        && grant.revision == revision
        && !grant.revoked
        && grant.expires_at.is_none_or(|expiry| expiry > now())
}

pub(super) fn effective(state: &Store, grant: &Grant) -> Grant {
    let mut result = grant.clone();
    if result.omit_sensitive_values {
        result.permissions.remove(&Permission::ReadBodies);
        result.permissions.remove(&Permission::ReadCredentials);
    }
    if let Some(entry) = state.disclosures.get(&grant.id).filter(|entry| {
        entry.phase == "temporary"
            && entry.revision == grant.revision
            && entry.deadline > Instant::now()
    }) {
        result
            .permissions
            .extend([Permission::ReadBodies, Permission::ReadCredentials]);
        result.disclosure_id = Some(entry.id.clone());
        result.sensitive_deadline = Some(entry.deadline);
    }
    result
}

fn view(grant: &Grant, entry: &Disclosure) -> Value {
    let remaining = entry.deadline.saturating_duration_since(Instant::now());
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    json!({"id":entry.id,"grantId":grant.id,"revision":entry.revision,"name":grant.name,"scope":grant.scope,"reason":entry.reason,"state":entry.phase,"requestedAt":entry.requested_at,"expiresAt":entry.expires_at,"remainingSeconds":seconds})
}

fn status(state: &Store, grant: &Grant) -> Value {
    let current = effective(state, grant);
    let entry = state
        .disclosures
        .get(&grant.id)
        .filter(|entry| entry.revision == grant.revision);
    json!({"omitSensitiveValues":grant.omit_sensitive_values,"headersAllowed":current.allows(Permission::ReadCredentials),"bodiesAllowed":current.allows(Permission::ReadBodies),"request":entry.map(|entry|view(grant,entry)),"instructions":"Ask the user to review the MCP access request near the top of Transmog. Poll status after their response; never treat a pending request as approval."})
}

impl AccessManager {
    /// Requests human approval for original headers/bodies for this key's existing scope.
    /// # Errors
    /// Rejects stale, inactive or oversized requests and briefly rate-limits denied requests.
    pub fn request_sensitive_access(&self, grant: &Grant, reason: &str) -> Result<Value, Error> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > 800 || reason.chars().any(char::is_control)
        {
            return Err(invalid(
                "Explain why original authentication headers and bodies are needed in 1–800 characters.",
            ));
        }
        let mut state = self.lock();
        prune(&mut state);
        let current = state
            .records
            .iter()
            .find(|record| record.grant.id == grant.id && record.grant.revision == grant.revision)
            .map(|record| record.grant.clone())
            .ok_or_else(unauthorized)?;
        if !valid(&state, &current) {
            return Err(unauthorized());
        }
        let approved = effective(&state, &current);
        if approved.allows(Permission::ReadBodies) && approved.allows(Permission::ReadCredentials) {
            return Ok(status(&state, &current));
        }
        if let Some(entry) = state.disclosures.get(&grant.id) {
            if entry.phase == "pending" {
                return Ok(status(&state, &current));
            }
            if entry.phase == "denied" && entry.deadline > Instant::now() {
                return Err(Error::new(
                    "request_denied",
                    "The user denied original-value access. Wait before making another explicit request.",
                ));
            }
        }
        state.disclosures.insert(
            grant.id.clone(),
            Disclosure {
                id: random(18)?,
                revision: grant.revision,
                reason: reason.into(),
                phase: "pending",
                requested_at: now(),
                expires_at: now() + 300,
                deadline: Instant::now() + Duration::from_secs(300),
            },
        );
        let result = status(&state, &current);
        drop(state);
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(result)
    }

    /// Reads only this authenticated grant's current approval, without creating a new prompt.
    /// # Errors
    /// Rejects inactive or stale grant references.
    pub fn sensitive_access_status(&self, grant: &Grant) -> Result<Value, Error> {
        let mut state = self.lock();
        prune(&mut state);
        let current = state
            .records
            .iter()
            .find(|record| record.grant.id == grant.id && record.grant.revision == grant.revision)
            .map(|record| &record.grant)
            .ok_or_else(unauthorized)?;
        if !valid(&state, current) {
            return Err(unauthorized());
        }
        Ok(status(&state, current))
    }

    /// Pending requests and active temporary approvals for the trusted desktop indicator.
    pub fn sensitive_access_requests(&self) -> Vec<Value> {
        let mut state = self.lock();
        prune(&mut state);
        state
            .records
            .iter()
            .filter_map(|record| {
                state
                    .disclosures
                    .get(&record.grant.id)
                    .filter(|entry| matches!(entry.phase, "pending" | "temporary"))
                    .map(|entry| view(&record.grant, entry))
            })
            .collect()
    }

    /// Responds from trusted desktop UI. Always durably changes the omission setting and read permissions.
    /// # Errors
    /// Rejects expired/replaced requests and failed persistence without publishing permanent consent.
    pub fn decide_sensitive_access(&self, id: &str, choice: &str) -> Result<(), Error> {
        if !matches!(choice, "minute" | "always" | "deny") {
            return Err(invalid("Choose For next 1 minute, Always or Deny."));
        }
        if choice == "always" {
            return self.mutate(|state| {
                prune(state);
                let (grant_id, revision) = pending(state, id)?;
                let grant = &mut find_record(state, &grant_id, revision)?.grant;
                grant.omit_sensitive_values = false;
                grant
                    .permissions
                    .extend([Permission::ReadBodies, Permission::ReadCredentials]);
                grant.revision = grant.revision.checked_add(1).ok_or_else(storage)?;
                state.disclosures.remove(&grant_id);
                Ok(())
            });
        }
        let mut state = self.lock();
        prune(&mut state);
        let (grant_id, _) = pending(&state, id)?;
        let entry = state.disclosures.get_mut(&grant_id).ok_or_else(|| {
            Error::new(
                "conflict",
                "This access request changed. Refresh agent access.",
            )
        })?;
        let seconds = if choice == "minute" { 60 } else { 30 };
        entry.phase = if choice == "minute" {
            "temporary"
        } else {
            "denied"
        };
        entry.expires_at = now() + seconds;
        entry.deadline = Instant::now() + Duration::from_secs(seconds);
        drop(state);
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    /// Revokes only the exact temporary approval shown in the desktop panel.
    /// # Errors
    /// Rejects a stale or already ended approval.
    pub fn revoke_sensitive_access(&self, id: &str) -> Result<(), Error> {
        let mut state = self.lock();
        prune(&mut state);
        let entry = state
            .disclosures
            .values_mut()
            .find(|entry| entry.id == id && entry.phase == "temporary")
            .ok_or_else(|| {
                Error::new(
                    "conflict",
                    "This temporary approval has already ended. Refresh agent access.",
                )
            })?;
        entry.phase = "revoked";
        drop(state);
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    /// Checks disclosure-dependent recording/control work without widening the key's scope.
    pub fn has_sensitive_access(&self, id: &str, revision: u64) -> bool {
        let mut state = self.lock();
        prune(&mut state);
        state
            .records
            .iter()
            .find(|record| {
                record.grant.id == id
                    && record.grant.revision == revision
                    && valid(&state, &record.grant)
            })
            .is_some_and(|record| {
                let grant = effective(&state, &record.grant);
                grant.allows(Permission::ReadBodies) && grant.allows(Permission::ReadCredentials)
            })
    }

    /// Ends all in-memory approvals and requests before a main workspace closes.
    pub fn clear_sensitive_access(&self) {
        self.lock().disclosures.clear();
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

fn pending(state: &Store, id: &str) -> Result<(String, u64), Error> {
    state
        .disclosures
        .iter()
        .find(|(_, entry)| entry.id == id && entry.phase == "pending")
        .map(|(grant_id, entry)| (grant_id.clone(), entry.revision))
        .ok_or_else(|| {
            Error::new(
                "conflict",
                "This access request expired or changed. Ask the agent to request access again.",
            )
        })
}

#[cfg(test)]
mod tests;
