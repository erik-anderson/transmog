//! Owns finite decoded-resource and request-signature budgets while preparing
//! an immutable preview away from the browser thread.
use super::{
    Candidate, CapturedPage, MAX_RESOURCE_BYTES, MAX_RESOURCES, MAX_SCENE_BYTES, ProcessContext,
    ResourceVariant, browser_headers, error,
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
    process: Option<ProcessContext>,
    navigation_time: u64,
}
impl SceneBuilder {
    pub(super) fn new(numeric: ExchangeId, root: tempfile::TempDir, navigation_time: u64) -> Self {
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
            process: None,
            navigation_time,
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
        let row = decision(&candidate);
        if self.resources >= MAX_RESOURCES || self.bytes >= MAX_SCENE_BYTES {
            self.record(
                row,
                "Preview limit reached (2,048 variants / 256 MiB decoded total)",
                None,
            );
            return Ok(());
        }
        if !(200..=599).contains(&candidate.head.status) {
            if primary {
                return Err(error("The selected HTML status cannot be rendered"));
            }
            self.record(row, "This HTTP status cannot be rendered", None);
            return Ok(());
        }
        if candidate.head.status == 304 {
            if primary {
                return Err(error(
                    "Select a captured HTML response with body bytes; a 304 requires the original browser cache",
                ));
            }
            self.record(
                row,
                "304 has no representation body; use another captured response for this URL",
                None,
            );
            return Ok(());
        }
        let body_sha256 = self.fingerprint(application, &row.entry_id, canceled);
        if canceled.load(Ordering::Acquire) {
            return Err(error("Captured page preview canceled"));
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
            canceled,
        )
        .await;
        let (resource, size) = match result {
            Ok(value) => value,
            Err(failure) if primary => return Err(failure),
            Err(failure) => {
                self.record(
                    row,
                    &format!(
                        "{}; preview limit 32 MiB per resource / 256 MiB total",
                        failure.message
                    ),
                    None,
                );
                return Ok(());
            }
        };
        self.bytes += size;
        self.resources += 1;
        if primary {
            self.add_primary(&candidate, resource.clone(), body_sha256.clone());
        }
        self.record(
            CapturedResourceDecision {sha256: Some(resource.sha256.clone()), ..row},
            if primary {
                "Selected HTML document; GET navigation uses these exact response bytes"
            } else {
                "Available response; body, UA, process, Origin/Referer, Sec- headers, Vary and capture time guide selection"
            },
            Some(size),
        );
        if !(primary && candidate.method == "GET") {
            let variant = Self::variant(
                &candidate,
                resource,
                body_sha256,
                self.process.as_ref(),
                false,
            );
            self.files
                .entry((candidate.method, candidate.url))
                .or_default()
                .push(variant);
        }
        Ok(())
    }
    fn add_primary(
        &mut self,
        candidate: &Candidate,
        resource: CapturedResource,
        body_sha256: Option<String>,
    ) {
        self.user_agent.clone_from(&candidate.user_agent);
        self.process.clone_from(&candidate.process);
        self.primary_url = Some(candidate.url.clone());
        self.files
            .entry(("GET".into(), candidate.url.clone()))
            .or_default()
            .push(Self::variant(
                candidate,
                resource,
                body_sha256,
                self.process.as_ref(),
                true,
            ));
    }
    fn variant(
        candidate: &Candidate,
        resource: CapturedResource,
        body_sha256: Option<String>,
        process: Option<&ProcessContext>,
        primary: bool,
    ) -> ResourceVariant {
        ResourceVariant {
            resource,
            entry_id: format!("{:032x}", candidate.id.0),
            body_sha256,
            vary: candidate.vary.clone(),
            primary,
            user_agent: candidate.user_agent.clone(),
            same_process: candidate
                .process
                .as_ref()
                .is_some_and(|candidate| Some(candidate) == process),
            origin: candidate.origin.clone(),
            referer: candidate.referer.clone(),
            sec_headers: candidate.sec_headers.clone(),
            time: candidate.time,
            order: candidate.id.0,
        }
    }
    pub(super) fn finish(
        self,
        scope: CapturedPageScope,
        source: String,
        source_trace_id: Option<String>,
    ) -> Result<CapturedPage, AppError> {
        let url = self
            .primary_url
            .ok_or_else(|| error("A complete HTML response is required for preview"))?;
        let diagnostics = CapturedPageDiagnostics::new(CapturedPageReport {
            url: url.clone(),
            scope,
            source,
            source_trace_id,
            scripts_enabled: None,
            selected_entry_id: format!("{:032x}", self.numeric.0),
            runtime: "WebView2 (version unavailable)".into(),
            bytes_available: true,
            rules: Vec::new(),
            variant: None,
            case_origin: None,
            saved_demo: None,
            available: self.resources,
            skipped: self.skipped,
            bytes: self.bytes,
            hits: 0,
            misses: 0,
            failures: 0,
            resources: self.decisions,
            requests: Vec::new(),
        });
        Ok(CapturedPage {
            url,
            user_agent: self.user_agent,
            resources: self.resources,
            skipped: self.skipped,
            diagnostics,
            navigation_time: self.navigation_time,
            positions: std::sync::Mutex::new(HashMap::new()),
            files: std::sync::Arc::new(self.files),
            overlay: crate::captured_page_overlay::Overlay::default(),
            root: std::sync::Arc::new(self.root),
        })
    }
}
fn decision(candidate: &Candidate) -> CapturedResourceDecision {
    CapturedResourceDecision {
        entry_id: format!("{:032x}", candidate.id.0),
        source: candidate.source.clone(),
        source_available: true,
        method: candidate.method.clone(),
        url: candidate.url.chars().take(2048).collect(),
        unix_millis: (candidate.time > 0).then_some(candidate.time),
        bytes: None,
        sha256: None,
        decision: String::new(),
    }
}
async fn materialize(
    application: &Application,
    store: &BodyStore,
    candidate: &Candidate,
    path: &Path,
    limit: u64,
    canceled: &AtomicBool,
) -> Result<(CapturedResource, u64), AppError> {
    let body = crate::response_file::prepare(
        &application.service,
        Some(store),
        &format!("{:032x}", candidate.id.0),
        "client-response",
    )?;
    let result = Box::pin(body.write_to_cancellable(path, limit, canceled)).await?;
    let reason = candidate.reason.clone().unwrap_or_else(|| {
        http::StatusCode::from_u16(candidate.head.status)
            .ok()
            .and_then(|status| status.canonical_reason())
            .unwrap_or("")
            .to_owned()
    });
    Ok((
        CapturedResource {
            sha256: {
                use sha2::{Digest, Sha256};
                format!(
                    "{:x}",
                    Sha256::digest(
                        std::fs::read(path)
                            .map_err(|_| error("Frozen response could not be hashed"))?
                    )
                )
            },
            status: candidate.head.status,
            reason,
            headers: browser_headers(&candidate.head, result.bytes),
            body_path: path.into(),
        },
        result.bytes,
    ))
}
