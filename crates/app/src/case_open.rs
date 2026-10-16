use super::{AppError, Application, CaseEvidenceLink, OpenedCaseEvidence, check_cancel, invalid};
use crate::trace_body::SourceReader;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
use transmog_portable as wire;

pub(super) enum CaseInput<'a> {
    Prepared(&'a wire::PreparedCase),
    Package(&'a wire::Package),
}
impl CaseInput<'_> {
    fn manifest(&self) -> Result<&wire::CaseManifest, AppError> {
        match self {
            Self::Prepared(p) => Ok(p.manifest()),
            Self::Package(p) => match p.manifest() {
                wire::PackageManifest::Case(m) => Ok(m),
                wire::PackageManifest::Rules(_) => Err(invalid("This package contains rules only")),
            },
        }
    }
    fn object(&self, sha: &str) -> Result<Box<dyn std::io::Read + Send>, AppError> {
        match self {
            Self::Prepared(p) => Ok(Box::new(
                p.open_object(sha)
                    .map_err(crate::rule_interchange::portable_error)?,
            )),
            Self::Package(p) => Ok(Box::new(
                p.open_object(sha)
                    .map_err(crate::rule_interchange::portable_error)?,
            )),
        }
    }
    fn source(&self, sha: &str) -> Result<SourceReader, AppError> {
        match self {
            Self::Prepared(p) => SourceReader::new(
                p.open_object(sha)
                    .map_err(crate::rule_interchange::portable_error)?,
            )
            .map_err(|_| invalid("Case evidence is unavailable")),
            Self::Package(p) => {
                let object = p
                    .manifest()
                    .objects()
                    .iter()
                    .find(|o| o.sha256 == sha)
                    .ok_or_else(|| invalid("Missing case source object"))?;
                Ok(SourceReader::package(
                    p.open_object(sha)
                        .map_err(crate::rule_interchange::portable_error)?,
                    wire::decimal(&object.bytes, true)
                        .map_err(crate::rule_interchange::portable_error)?,
                    sha.into(),
                ))
            }
        }
    }
}

pub(super) async fn open(
    app: &Application,
    case: CaseInput<'_>,
    source_id: &str,
    password: Option<transmog_capture::CapturePassword>,
    canceled: Arc<AtomicBool>,
) -> Result<OpenedCaseEvidence, AppError> {
    check_cancel(&canceled)?;
    let manifest = case.manifest()?;
    let source = manifest
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| invalid("Case source is not included"))?;
    if !matches!(source.dependency.state.as_str(), "present" | "locked") {
        return Err(invalid("Case source evidence is unavailable"));
    }
    let sha = source.artifact_sha256.clone();
    let reader = case.source(&sha)?;
    let keys = manifest
        .sources
        .iter()
        .filter(|s| {
            s.artifact_sha256 == sha && matches!(s.dependency.state.as_str(), "present" | "locked")
        })
        .map(|s| s.source_key.clone())
        .collect::<Vec<_>>();
    let mut contexts = BTreeMap::new();
    for context in &manifest.contexts {
        check_cancel(&canceled)?;
        if context.kind != "capture-metadata" || context.content.state != "present" {
            continue;
        }
        let Some(reference) = &context.origin.source else {
            continue;
        };
        if reference.artifact_sha256 != sha || !keys.contains(&reference.source_key) {
            continue;
        }
        let value = case.object(context.content.sha256.as_deref().unwrap_or(""))?;
        if contexts
            .insert(reference.source_key.clone(), value)
            .is_some()
        {
            return Err(invalid("Conflicting source context objects"));
        }
    }
    let worker = app.clone();
    let expected_keys = keys.clone();
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
            .import_case_source(reader, &request, &worker, contexts, &expected_keys, &flag)
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
