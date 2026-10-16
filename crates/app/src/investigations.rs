//! UI-neutral working investigations; archive/dialog and CLI adapters remain separate.
use crate::{AppError, Application, ErrorCategory, RuleExportRequest, TraceImportResult};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_portable as wire;
#[path = "case_native.rs"]
mod native;
#[path = "case_open.rs"]
mod open;

#[cfg(test)]
#[path = "investigation_tests.rs"]
mod investigation_tests;

/// An authored note, optionally linked to an explicitly selected exchange.
#[derive(Clone, Debug)]
pub struct CaseNoteInput {
    /// New case-local note identity.
    pub id: String,
    /// Inert authored text, at most 64 KiB.
    pub text: String,
    /// Workspace exchange to rebind to the new derived evidence artifact.
    pub exchange_id: Option<String>,
}
/// Selected saved experiments and an optional frozen evidence review.
#[derive(Clone, Debug)]
pub struct CaseExportRequest {
    /// Explicit saved rule/revision selections and case identity/title.
    pub rules: RuleExportRequest,
    /// A review produced by `review_traffic_export`; None includes no traffic.
    pub evidence_review_id: Option<String>,
    /// Explicit consent to a new plaintext derived native capture of selected retained evidence.
    pub derived_plaintext: bool,
    /// Authored notes; no captured fact is inferred from their contents.
    pub notes: Vec<CaseNoteInput>,
}
/// One source-stable reference to a newly admitted runtime row.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseEvidenceLink {
    /// Actual case object/source/original-entry identity.
    pub reference: wire::EvidenceReference,
    /// Runtime-only navigation handle, never saved into a case.
    pub exchange_id: String,
}
/// Imported inert evidence and navigation links; no automation is imported or activated.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedCaseEvidence {
    /// Existing Traffic/source inspection result.
    pub imported: TraceImportResult,
    /// Source-local original identities, including repeated entry occurrences.
    pub links: Vec<CaseEvidenceLink>,
}
impl OpenedCaseEvidence {
    /// Resolves a note/run reference to this open evidence instance, without guessing missing IDs.
    pub fn resolve(&self, reference: &wire::EvidenceReference) -> Option<&str> {
        self.links
            .iter()
            .find(|link| {
                let source = &link.reference;
                source.artifact_sha256 == reference.artifact_sha256
                    && source.source_key == reference.source_key
                    && source.exchange_id == reference.exchange_id
                    && source.occurrence == reference.occurrence
            })
            .map(|link| link.exchange_id.as_str())
    }
}
impl Application {
    /// Freezes a minimal case from selected retained evidence and immutable saved rules.
    /// # Errors
    /// Rejects stale selections, unavailable dependencies, missing plaintext consent or cancellation.
    pub fn prepare_investigation(
        &self,
        request: &CaseExportRequest,
        staging: &Path,
        canceled: &AtomicBool,
    ) -> Result<wire::PreparedCase, AppError> {
        self.prepare_investigation_inner(request, staging, canceled)
            .map_err(|error| {
                if canceled.load(Ordering::Acquire) {
                    AppError::new(ErrorCategory::Canceled, "Case operation canceled", false)
                } else {
                    error
                }
            })
    }
    fn prepare_investigation_inner(
        &self,
        request: &CaseExportRequest,
        staging: &Path,
        canceled: &AtomicBool,
    ) -> Result<wire::PreparedCase, AppError> {
        check_cancel(canceled)?;
        if request.evidence_review_id.is_some() && !request.derived_plaintext {
            return Err(invalid(
                "Explicitly select a plaintext derived evidence copy before saving a working case",
            ));
        }
        let rules = self.export_rule_collection(&request.rules, staging, canceled)?;
        let collection = rules.manifest();
        let mut manifest = wire::CaseManifest {
            format: collection.format.clone(),
            schema_version: 0,
            kind: "case".into(),
            id: collection.id.clone(),
            revision: collection.revision.clone(),
            title: collection.title.clone(),
            created_at: collection.created_at.clone(),
            producer: collection.producer.clone(),
            required_features: collection.required_features.clone(),
            objects: collection.objects.clone(),
            sources: vec![],
            assets: collection.assets.clone(),
            rules: collection.rules.clone(),
            experiments: collection
                .experiments
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<_, _>>()
                .map_err(|_| invalid("Experiment serialization failed"))?,
            notes: vec![],
            markers: vec![],
            contexts: vec![],
            runs: vec![],
            extensions: collection.extensions.clone(),
        };
        let mut files = BTreeMap::new();
        let mut links = BTreeMap::new();
        if let Some(id) = &request.evidence_review_id {
            let evidence = native::snapshot(self, id, staging, canceled)?;
            manifest.required_features.push("native-evidence/1".into());
            manifest.objects.extend(evidence.objects);
            manifest.sources = evidence.sources;
            manifest.contexts = evidence.contexts;
            manifest.markers = evidence.markers;
            files = evidence.files;
            links = evidence.links;
        }
        for note in &request.notes {
            let reference = note
                .exchange_id
                .as_ref()
                .map(|id| {
                    links.get(id).cloned().ok_or_else(|| {
                        invalid("A note refers to evidence outside the selected review")
                    })
                })
                .transpose()?;
            manifest.notes.push(wire::CaseNote {
                id: note.id.clone(),
                text: note.text.clone(),
                source: reference,
            });
        }
        let bytes = manifest
            .encode()
            .map_err(crate::rule_interchange::portable_error)?;
        wire::PreparedCase::prepare(
            &bytes,
            staging,
            |sha| {
                if let Some(file) = files.get(sha) {
                    Ok(Box::new(file.reopen()?) as Box<dyn Read + Send>)
                } else {
                    Ok(Box::new(rules.open_object(sha)?))
                }
            },
            canceled,
        )
        .map_err(crate::rule_interchange::portable_error)
    }
    /// Opens one case capture through the ordinary inert trace inspection path.
    /// # Errors
    /// Returns locked/missing/invalid sources, cancellation or existing viewer limits.
    pub async fn open_investigation_evidence(
        &self,
        case: &wire::PreparedCase,
        source_id: &str,
        password: Option<transmog_capture::CapturePassword>,
        canceled: Arc<AtomicBool>,
    ) -> Result<OpenedCaseEvidence, AppError> {
        open::open(
            self,
            open::CaseInput::Prepared(case),
            source_id,
            password,
            canceled,
        )
        .await
    }
    /// Inspects a package source using pinned authenticated random reads, without extraction.
    /// # Errors
    /// Rejects unavailable sources/passwords and preserves atomic inert import.
    pub async fn open_package_evidence(
        &self,
        package: &wire::Package,
        source_id: &str,
        password: Option<transmog_capture::CapturePassword>,
        canceled: Arc<AtomicBool>,
    ) -> Result<OpenedCaseEvidence, AppError> {
        open::open(
            self,
            open::CaseInput::Package(package),
            source_id,
            password,
            canceled,
        )
        .await
    }
}
pub(super) fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
pub(super) fn check_cancel(canceled: &AtomicBool) -> Result<(), AppError> {
    if canceled.load(Ordering::Acquire) {
        Err(AppError::new(
            ErrorCategory::Canceled,
            "Case operation canceled",
            false,
        ))
    } else {
        Ok(())
    }
}
