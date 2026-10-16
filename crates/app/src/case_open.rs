use super::{AppError, Application, CaseEvidenceLink, OpenedCaseEvidence, check_cancel, invalid};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
use transmog_portable as wire;

pub(super) async fn open(
    app: &Application,
    case: &wire::PreparedCase,
    source_id: &str,
    password: Option<transmog_capture::CapturePassword>,
    canceled: Arc<AtomicBool>,
) -> Result<OpenedCaseEvidence, AppError> {
    check_cancel(&canceled)?;
    let source = case
        .manifest()
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| invalid("Case source is not included"))?;
    if !matches!(source.dependency.state.as_str(), "present" | "locked") {
        return Err(invalid("Case source evidence is unavailable"));
    }
    let sha = source.artifact_sha256.clone();
    let file = case
        .open_object(&sha)
        .map_err(crate::rule_interchange::portable_error)?;
    let keys = case
        .manifest()
        .sources
        .iter()
        .filter(|s| s.artifact_sha256 == sha)
        .map(|s| s.source_key.clone())
        .collect::<Vec<_>>();
    let mut contexts = BTreeMap::new();
    for context in &case.manifest().contexts {
        check_cancel(&canceled)?;
        if context.kind != "capture-metadata" || context.content.state != "present" {
            continue;
        }
        let Some(reference) = &context.origin.source else {
            continue;
        };
        if reference.artifact_sha256 != sha {
            continue;
        }
        let value = case
            .open_object(context.content.sha256.as_deref().unwrap_or(""))
            .map_err(crate::rule_interchange::portable_error)?;
        if contexts
            .insert(reference.source_key.clone(), value)
            .is_some()
        {
            return Err(invalid("Conflicting source context objects"));
        }
    }
    let worker = app.clone();
    let flag = canceled.clone();
    let request = crate::TraceImportRequest {
        path: source.label.clone().into(),
        password,
        operation_id: crate::rule_interchange::fresh_id("case-open")?,
        max_file_bytes: u64::MAX,
    };
    let mut pending = CancelOnDrop(Some(canceled.clone()));
    let result = tokio::task::spawn_blocking(move || {
        worker
            .traces
            .import_case_file(file, &request, &worker, contexts, &flag)
    })
    .await;
    pending.0 = None;
    let imported = result
        .map_err(|_| invalid("Case evidence worker failed"))?
        .map_err(|error| {
            if canceled.load(std::sync::atomic::Ordering::Acquire) {
                AppError::new(
                    crate::ErrorCategory::Canceled,
                    "Case evidence opening canceled",
                    false,
                )
            } else {
                error
            }
        })?;
    let mut occurrences = BTreeMap::new();
    let mut links = Vec::new();
    for (key, id, entry) in app.traces.case_entries(&imported.trace.id, &keys) {
        let count = occurrences
            .entry((key.clone(), entry.original_id.clone()))
            .or_insert(0_u64);
        links.push(CaseEvidenceLink {
            reference: wire::EvidenceReference {
                artifact_sha256: sha.clone(),
                source_key: key,
                exchange_id: Some(entry.original_id),
                occurrence: count.to_string(),
                boundary: "client-response".into(),
            },
            exchange_id: id,
        });
        *count += 1;
    }
    Ok(OpenedCaseEvidence { imported, links })
}

struct CancelOnDrop(Option<Arc<AtomicBool>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(flag) = &self.0 {
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}
