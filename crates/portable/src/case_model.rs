//! Case wire fields, independent of application persistence and executable state.
#![allow(missing_docs)] // Field contract is documented in portable-investigations.md.
use crate::{Asset, EvidenceReference, Object, Producer, Rule};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! object {
    ($name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
    };
}
object!(Availability {
    state: String,
    #[serde(default, skip_serializing_if="Option::is_none")] reason: Option<String>,
    #[serde(default, skip_serializing_if="Option::is_none")] sha256: Option<String>,
    #[serde(default, skip_serializing_if="Option::is_none")] bytes: Option<String>
});
object!(SourceAssociation {
    artifact_sha256: String,
    source_key: String
});
object!(ParentArtifact { artifact_sha256: String, #[serde(default, skip_serializing_if="Option::is_none")] source_key: Option<String>, state: String, #[serde(default)] reason: Option<String> });
object!(CaseEntryLineage { exchange_id: Option<String>, occurrence: String, original: EvidenceReference });
object!(Lineage {
    kind: String,
    #[serde(default, skip_serializing_if="Vec::is_empty")] operations: Vec<String>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] parents: Vec<ParentArtifact>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] entry_mappings: Vec<CaseEntryLineage>,
    #[serde(default, skip_serializing_if="Option::is_none")] unavailable_parent_reason: Option<String>
});
object!(CaseSource {
    id: String,
    artifact_sha256: String,
    source_key: String,
    label: String,
    #[serde(default, skip_serializing_if="Option::is_none")] request_count: Option<String>,
    dependency: Availability,
    lineage: Lineage,
    evidence_completeness: String
});
object!(CaseNote {
    id: String, text: String,
    #[serde(default, skip_serializing_if="Option::is_none")] source: Option<EvidenceReference>
});
object!(CaseMarker {
    id: String,
    label: String,
    source: SourceAssociation,
    original_id: String,
    elapsed_millis: Option<String>
});
object!(ContextOrigin {
    kind: String, collected_at: Option<String>, collector: Producer, platform: String, purpose: String,
    #[serde(default, skip_serializing_if="Option::is_none")] machine_label: Option<String>,
    #[serde(default, skip_serializing_if="Option::is_none")] source: Option<SourceAssociation>
});
object!(CaseContext {
    id: String, kind: String, origin: ContextOrigin, content: Availability,
    completeness: String, issues: Vec<String>
});
object!(FrozenExperiment {
    experiment_id: String,
    variant_id: String,
    revision: String
});
object!(AuthoredOutcome {
    origin: String,
    text: String
});
object!(RunResponse {
    source: EvidenceReference,
    body: crate::Dependency,
    provenance: String
});
object!(CaseRun {
    id: String, experiment_id: String, variant_id: String, revision: String,
    responses: Vec<RunResponse>, outcome: AuthoredOutcome
});
object!(ComparisonSettings {
    excluded_headers: Vec<String>, normalize_newlines: bool,
    excluded_query_names: Vec<String>, request_headers: Vec<String>, request_body_digest: bool
});
object!(ComparisonPair { left: Option<EvidenceReference>, right: Option<EvidenceReference> });
object!(CaseComparison {
    id: String, label: String, kind: String, engine: String,
    left: Vec<EvidenceReference>, right: Vec<EvidenceReference>,
    request_boundary: String, settings: ComparisonSettings,
    corrections: Vec<ComparisonPair>, recorded_summary: String
});
object!(CaseManifest {
    format: String, schema_version: u32, kind: String, id: String, revision: String,
    title: String, created_at: String, producer: Producer, required_features: Vec<String>,
    objects: Vec<Object>, sources: Vec<CaseSource>, assets: Vec<Asset>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] rules: Vec<Rule>,
    experiments: Vec<Value>, notes: Vec<CaseNote>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] markers: Vec<CaseMarker>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] contexts: Vec<CaseContext>,
    runs: Vec<CaseRun>,
    #[serde(default, skip_serializing_if="Vec::is_empty")] comparisons: Vec<CaseComparison>,
    extensions: Map<String, Value>
});

/// Inventory distinguishes inspectable evidence from available rule dependencies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseInventory {
    pub sources: usize,
    pub notes: usize,
    pub contexts: usize,
    pub runs: usize,
    pub object_bytes: String,
    pub experiments: Vec<ExperimentAvailability>,
    pub missing_dependencies: usize,
    pub incomplete_sources: usize,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentAvailability {
    pub id: String,
    pub state: String,
    pub reason: Option<String>,
}
