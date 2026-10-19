//! UI-neutral rule collection interchange; archive/dialog and CLI flows are later increments.
use crate::{
    AppError, Application, AutomationStatus, ErrorCategory, ExperimentSelection, ResponseAsset,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::Path, sync::atomic::AtomicBool};
use transmog_portable::{Collection, PreparedCollection};

/// Portable identity of the editable current collection, independent of selection order.
pub(crate) const CURRENT_RESPONSE_SET_ID: &str = "current";

/// Original portable definitions and the local identity mapping, persisted atomically.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedCollection {
    /// Exact imported definitions; authored intent never enables live automation.
    pub source: Collection,
    /// Imported assets; their metadata is owned by this automation commit.
    pub assets: Vec<ResponseAsset>,
    /// Original asset ID/revision to newly allocated local reference.
    pub asset_refs: BTreeMap<String, String>,
    /// Original rule identity to local identity.
    pub rule_ids: BTreeMap<String, String>,
    /// Original experiment identity to local identity.
    pub experiment_ids: BTreeMap<String, String>,
}
/// One reviewed identity; all entries receive fresh local identities by default.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleImportItem {
    /// Rule, asset or experiment.
    pub kind: String,
    /// Original portable identity.
    pub portable_id: String,
    /// Exact original revision string.
    pub revision: String,
    /// New, identical-source-definition, or conflicting.
    pub state: String,
    /// Fresh local identity/reference, never an overwrite.
    pub local_id: String,
}
/// A mutation-free review of a frozen selected dependency closure.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleImportReview {
    /// Required current workspace generation at commit.
    pub generation: u64,
    /// Human title from the source, not authority.
    pub title: String,
    /// Exact source manifest identity and intent.
    pub source: Collection,
    /// Conflicts/remaps to inspect before committing.
    pub items: Vec<RuleImportItem>,
    /// Imported live rules are disabled and experiments have no active revision.
    pub activation: &'static str,
}
/// Owned input snapshot and private reviewed mapping; cannot be forged through JSON.
#[derive(Debug)]
pub struct PreparedRuleImport {
    pub(crate) prepared: PreparedCollection,
    pub(crate) review: RuleImportReview,
    pub(crate) mapping: ImportedCollection,
    pub(crate) owner: std::sync::Weak<std::sync::Mutex<()>>,
}
impl PreparedRuleImport {
    /// Inspects intended enabled/paused state, exact revisions and conflicts.
    pub fn review(&self) -> &RuleImportReview {
        &self.review
    }
}
/// Explicit selected saved content to export; drafts and active registrations are excluded.
#[derive(Clone, Debug)]
pub struct RuleExportRequest {
    /// New portable collection identity.
    pub id: String,
    /// Canonical positive revision string.
    pub revision: String,
    /// Human collection name.
    pub title: String,
    /// Ordinary saved rule identities; export retains production precedence order.
    pub rule_ids: Vec<String>,
    /// Exact named saved revisions, independent of their live selection.
    pub selections: Vec<ExperimentSelection>,
}
pub(crate) fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
#[allow(clippy::needless_pass_by_value)] // Standard map_err adapter consumes the source error.
pub(crate) fn portable_error(error: transmog_portable::Error) -> AppError {
    AppError::new(
        match error {
            transmog_portable::Error::PasswordRequired => ErrorCategory::PasswordRequired,
            transmog_portable::Error::InvalidPassword => ErrorCategory::InvalidPassword,
            transmog_portable::Error::Io(_) => ErrorCategory::Unavailable,
            transmog_portable::Error::Canceled => ErrorCategory::Canceled,
            transmog_portable::Error::Unsupported(_) => ErrorCategory::Unsupported,
            transmog_portable::Error::Invalid(_) => ErrorCategory::InvalidInput,
        },
        error.to_string(),
        false,
    )
}
pub(crate) fn imported_assets(document: &crate::AutomationRuleSet) -> Vec<ResponseAsset> {
    document
        .portable_imports
        .iter()
        .flat_map(|c| c.assets.clone())
        .collect()
}
pub(crate) fn validate_imported(document: &crate::AutomationRuleSet) -> Result<(), AppError> {
    if document.portable_imports.len() > 64 {
        return Err(invalid("Too many imported rule collections"));
    }
    for imported in &document.portable_imports {
        imported.source.encode().map_err(portable_error)?;
        if imported.assets.len() != imported.source.assets.len()
            || imported.asset_refs.len() != imported.source.assets.len()
        {
            return Err(invalid("Incomplete imported asset catalog"));
        }
        for source in &imported.source.assets {
            let reference = imported
                .asset_refs
                .get(&format!("{}@{}", source.id, source.revision))
                .ok_or_else(|| invalid("Missing imported asset mapping"))?;
            let (id, revision) = reference
                .rsplit_once('@')
                .ok_or_else(|| invalid("Invalid imported asset mapping"))?;
            if !transmog_portable::valid_id(id) || revision != source.revision {
                return Err(invalid("Imported asset revision changed"));
            }
            let expected =
                crate::response_assets::ResponseAssetStore::portable_metadata(source, id.into())?;
            if !imported.assets.contains(&expected) {
                return Err(invalid("Imported asset metadata does not match its source"));
            }
        }
        let rules =
            imported
                .source
                .rules
                .iter()
                .chain(imported.source.experiments.iter().flat_map(|e| {
                    e.variants
                        .iter()
                        .flat_map(|v| v.revisions.iter().flat_map(|r| &r.rules))
                }));
        validate_mapping(rules.map(|r| r.id.clone()), &imported.rule_ids)?;
        validate_mapping(
            imported.source.experiments.iter().map(|e| e.id.clone()),
            &imported.experiment_ids,
        )?;
    }
    Ok(())
}
fn validate_mapping(
    keys: impl Iterator<Item = String>,
    mapping: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    use std::collections::BTreeSet;
    if keys.collect::<BTreeSet<_>>() != mapping.keys().cloned().collect() {
        return Err(invalid("Incomplete imported identity mapping"));
    }
    for id in mapping.values() {
        if !transmog_portable::valid_id(id) {
            return Err(invalid("Invalid imported local identity"));
        }
    }
    Ok(())
}
pub(crate) fn asset_index(
    document: &crate::AutomationRuleSet,
) -> std::sync::Arc<BTreeMap<String, ResponseAsset>> {
    std::sync::Arc::new(
        imported_assets(document)
            .into_iter()
            .map(|a| (a.asset_ref(), a))
            .collect(),
    )
}
pub(crate) fn fresh_id(kind: &str) -> Result<String, AppError> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        AppError::new(
            ErrorCategory::Unavailable,
            "Import identity allocation failed",
            false,
        )
    })?;
    Ok(format!("import-{kind}-{:032x}", u128::from_le_bytes(bytes)))
}
impl Application {
    /// Snapshots and reviews a strict rule collection without mutating the workspace.
    ///
    /// `open` is an explicit local object provider. No dependency path, URL or
    /// source-machine file is opened from manifest data.
    /// # Errors
    /// Rejects incomplete/unsupported content, bad hashes and cancellation.
    pub fn prepare_rule_import(
        &self,
        manifest: &[u8],
        staging_parent: &Path,
        open: impl FnMut(&str) -> Result<Box<dyn Read + Send>, transmog_portable::Error>,
        canceled: &AtomicBool,
    ) -> Result<PreparedRuleImport, AppError> {
        let prepared = PreparedCollection::prepare(manifest, staging_parent, open, canceled)
            .map_err(portable_error)?;
        self.automation.review_portable(prepared)
    }
    /// Publishes exactly the reviewed collection as disabled authoring content.
    /// # Errors
    /// Stale reviews, persistence failures and cancellation preserve existing state.
    pub fn import_rule_collection(
        &self,
        prepared: PreparedRuleImport,
        canceled: &AtomicBool,
    ) -> Result<AutomationStatus, AppError> {
        self.automation
            .import_portable(prepared, canceled)
            .map_err(|error| {
                if canceled.load(std::sync::atomic::Ordering::Acquire) {
                    AppError::new(ErrorCategory::Canceled, "Rule import canceled", false)
                } else {
                    error
                }
            })?;
        Ok(self.automation_status())
    }
    /// Exports selected immutable rules/assets with bounded coherent file snapshots.
    /// # Errors
    /// Rejects unavailable selections/assets, unsupported actions or cancellation.
    pub fn export_rule_collection(
        &self,
        request: &RuleExportRequest,
        staging_parent: &Path,
        canceled: &AtomicBool,
    ) -> Result<PreparedCollection, AppError> {
        self.automation
            .export_portable(request, staging_parent, canceled)
            .map_err(|error| {
                if canceled.load(std::sync::atomic::Ordering::Acquire) {
                    AppError::new(ErrorCategory::Canceled, "Rule export canceled", false)
                } else {
                    error
                }
            })
    }
}
