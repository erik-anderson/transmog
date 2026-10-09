//! Owns finite decoded-resource and request-signature budgets while preparing
//! an immutable preview away from the browser thread.
use super::{
    Candidate, CapturedPage, MAX_RESOURCE_BYTES, MAX_RESOURCES, MAX_SCENE_BYTES, ResourceVariant,
    browser_headers, error,
};
use crate::{
    AppError, Application, BodyStore, CapturedPageDiagnostics, CapturedPageReport,
    CapturedPageScope, CapturedResource, CapturedResourceDecision,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use transmog_core::intercept::ExchangeId;

pub(super) struct SceneBuilder {
    numeric: ExchangeId,
    root: tempfile::TempDir,
    files: HashMap<(String, String), Vec<ResourceVariant>>,
    bytes: u64,
    signature_bytes: u64,
    resources: usize,
    skipped: usize,
    decisions: Vec<CapturedResourceDecision>,
    primary_url: Option<String>,
    user_agent: Option<String>,
}
impl SceneBuilder {
    pub(super) fn new(numeric: ExchangeId, root: tempfile::TempDir) -> Self {
        Self {
            numeric,
            root,
            files: HashMap::new(),
            bytes: 0,
            signature_bytes: 0,
            resources: 0,
            skipped: 0,
            decisions: Vec::new(),
            primary_url: None,
            user_agent: None,
        }
    }
    fn record(&mut self, mut row: CapturedResourceDecision, reason: &str, bytes: Option<u64>) {
        if bytes.is_none() {
            self.skipped += 1;
        }
        if self.decisions.len() < 256 {
            row.decision = reason.into();
            row.bytes = bytes;
            self.decisions.push(row);
        }
    }
    fn fingerprint(
        &mut self,
        application: &Application,
        id: &str,
        canceled: &AtomicBool,
    ) -> Option<String> {
        application
            .prepare_request_file(id)
            .and_then(|body| {
                body.fingerprint(
                    MAX_RESOURCE_BYTES.min(MAX_SCENE_BYTES - self.signature_bytes),
                    canceled,
                )
            })
            .ok()
            .flatten()
            .map(|(digest, size)| {
                self.signature_bytes += size;
                digest
            })
    }
    pub(super) async fn add(
        &mut self,
        application: &Application,
        store: &BodyStore,
        candidate: Candidate,
        canceled: &AtomicBool,
    ) -> Result<(), AppError> {
        if canceled.load(Ordering::Acquire) {
            return Err(error("Captured page preview canceled"));
        }
        let primary = candidate.id == self.numeric;
        let row = CapturedResourceDecision {
            entry_id: format!("{:032x}", candidate.id.0),
            source: candidate.source.clone(),
            source_available: true,
            method: candidate.method.clone(),
            url: candidate.url.chars().take(2048).collect(),
            unix_millis: (candidate.time > 0).then_some(candidate.time),
            bytes: None,
            decision: String::new(),
        };
        if self.resources >= MAX_RESOURCES || self.bytes >= MAX_SCENE_BYTES {
            self.record(row, "Preview resource/byte budget reached", None);
            return Ok(());
        }
        if !(200..=599).contains(&candidate.head.status) {
            if primary {
                return Err(error("The selected HTML status cannot be rendered"));
            }
            self.record(row, "This HTTP status cannot be rendered", None);
            return Ok(());
        }
        let body_sha256 = self.fingerprint(application, &row.entry_id, canceled);
        if canceled.load(Ordering::Acquire) {
            return Err(error("Captured page preview canceled"));
        }
        if !primary && (body_sha256.is_none() || candidate.vary.is_none()) {
            self.record(
                row,
                "Complete bounded request body or Vary headers unavailable",
                None,
            );
            return Ok(());
        }
        let path = self
            .root
            .path()
            .join(format!("resource-{}.bin", self.resources));
        let result = materialize(
            application,
            store,
            &candidate,
            &path,
            MAX_RESOURCE_BYTES.min(MAX_SCENE_BYTES - self.bytes),
        )
        .await;
        let (resource, size) = match result {
            Ok(value) => value,
            Err(failure) if primary => return Err(failure),
            Err(_) => {
                self.record(row, "Complete bounded response bytes unavailable", None);
                return Ok(());
            }
        };
        self.bytes += size;
        self.resources += 1;
        if primary {
            self.user_agent.clone_from(&candidate.user_agent);
            self.primary_url = Some(candidate.url.clone());
            self.files
                .entry(("GET".into(), candidate.url.clone()))
                .or_default()
                .push(ResourceVariant {
                    resource: resource.clone(),
                    entry_id: row.entry_id.clone(),
                    body_sha256: body_sha256.clone(),
                    vary: candidate.vary.clone(),
                    primary: true,
                });
        }
        self.record(
            row,
            if primary {
                "Selected HTML document; GET navigation uses these exact response bytes"
            } else {
                "Nearest matching body and Vary variant; Accept-Encoding ignored after decoding"
            },
            Some(size),
        );
        if !(primary && candidate.method == "GET") {
            self.files
                .entry((candidate.method, candidate.url))
                .or_default()
                .push(ResourceVariant {
                    resource,
                    entry_id: format!("{:032x}", candidate.id.0),
                    body_sha256,
                    vary: candidate.vary,
                    primary: false,
                });
        }
        Ok(())
    }
    pub(super) fn finish(
        self,
        scope: CapturedPageScope,
        source: String,
    ) -> Result<CapturedPage, AppError> {
        let url = self
            .primary_url
            .ok_or_else(|| error("A complete HTML response is required for preview"))?;
        let diagnostics = CapturedPageDiagnostics::new(CapturedPageReport {
            url: url.clone(),
            scope,
            source,
            scripts_enabled: None,
            available: self.resources,
            skipped: self.skipped,
            bytes: self.bytes,
            hits: 0,
            misses: 0,
            resources: self.decisions,
            requests: Vec::new(),
        });
        Ok(CapturedPage {
            url,
            user_agent: self.user_agent,
            resources: self.resources,
            skipped: self.skipped,
            diagnostics,
            files: self.files,
            _root: self.root,
        })
    }
}
async fn materialize(
    application: &Application,
    store: &BodyStore,
    candidate: &Candidate,
    path: &Path,
    limit: u64,
) -> Result<(CapturedResource, u64), AppError> {
    let body = crate::response_file::prepare(
        &application.service,
        Some(store),
        &format!("{:032x}", candidate.id.0),
        "client-response",
    )?;
    let result = Box::pin(body.write_to(path, limit)).await?;
    let reason = candidate.reason.clone().unwrap_or_else(|| {
        http::StatusCode::from_u16(candidate.head.status)
            .ok()
            .and_then(|status| status.canonical_reason())
            .unwrap_or("")
            .to_owned()
    });
    Ok((
        CapturedResource {
            status: candidate.head.status,
            reason,
            headers: browser_headers(&candidate.head, result.bytes),
            body_path: path.into(),
        },
        result.bytes,
    ))
}
