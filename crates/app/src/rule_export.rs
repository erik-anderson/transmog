use super::AutomationRegistry;
use crate::{
    AppError, ResponseAssetProvenance,
    rule_interchange::{RuleExportRequest, invalid, portable_error},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use transmog_portable as wire;

impl AutomationRegistry {
    #[allow(clippy::too_many_lines)] // One selected graph walk with owned snapshots and final verification.
    pub(crate) fn export_portable(
        &self,
        request: &RuleExportRequest,
        staging_parent: &Path,
        canceled: &AtomicBool,
    ) -> Result<wire::PreparedCollection, AppError> {
        let document = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for id in &request.rule_ids {
            if !seen.insert(id) {
                return Err(invalid("Duplicate selected rule"));
            }
            if document.experiments.iter().any(|e| e.owns(id)) {
                return Err(invalid(
                    "Select the exact named variant instead of its projected live rule",
                ));
            }
            selected.push(
                document
                    .rules
                    .iter()
                    .find(|r| &r.id == id)
                    .ok_or_else(|| invalid("Selected saved rule is unavailable"))?
                    .clone(),
            );
        }
        selected.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));
        let mut experiments = BTreeMap::<String, wire::Experiment>::new();
        let mut revisions = BTreeSet::new();
        for selection in &request.selections {
            if !revisions.insert((
                &selection.experiment_id,
                &selection.variant_id,
                selection.revision,
            )) {
                return Err(invalid("Duplicate selected revision"));
            }
            let experiment = document
                .experiments
                .iter()
                .find(|e| e.id == selection.experiment_id)
                .ok_or_else(|| invalid("Selected response set is unavailable"))?;
            let revision = experiment
                .revision(selection)
                .ok_or_else(|| invalid("Selected saved revision is unavailable"))?;
            let variant = experiment
                .variants
                .iter()
                .find(|v| v.id == selection.variant_id)
                .ok_or_else(|| invalid("Selected variant is unavailable"))?;
            let target =
                experiments
                    .entry(experiment.id.clone())
                    .or_insert_with(|| wire::Experiment {
                        id: portable_id(&experiment.id),
                        kind: "response-variants".into(),
                        required_features: vec!["response-assets/1".into()],
                        label: experiment.name.clone(),
                        paused_intent: experiment.paused,
                        archived_intent: experiment.archived,
                        active_intent: None,
                        variants: vec![wire::Variant {
                            id: "baseline".into(),
                            label: "Baseline".into(),
                            archived_intent: false,
                            revisions: vec![wire::Revision {
                                revision: "1".into(),
                                rules: vec![],
                            }],
                        }],
                    });
            if experiment.active.as_ref() == Some(selection) {
                target.active_intent = Some(wire::ActiveSelection {
                    variant_id: portable_id(&variant.id),
                    revision: selection.revision.to_string(),
                });
            }
            if variant.id != "baseline" {
                let existing = target
                    .variants
                    .iter_mut()
                    .find(|v| v.id == portable_id(&variant.id));
                let rules = revision
                    .rules
                    .iter()
                    .map(export_rule)
                    .collect::<Result<_, _>>()?;
                let revision = wire::Revision {
                    revision: revision.revision.to_string(),
                    rules,
                };
                if let Some(existing) = existing {
                    existing.revisions.push(revision);
                    existing
                        .revisions
                        .sort_by_key(|r| wire::decimal(&r.revision, false).unwrap_or(0));
                } else {
                    target.variants.push(wire::Variant {
                        id: portable_id(&variant.id),
                        label: variant.name.clone(),
                        archived_intent: variant.archived,
                        revisions: vec![revision],
                    });
                }
            }
        }
        let rules = selected
            .iter()
            .map(export_rule)
            .collect::<Result<Vec<_>, _>>()?;
        let mut references = BTreeSet::new();
        for rule in selected
            .iter()
            .chain(request.selections.iter().flat_map(|s| {
                document
                    .experiments
                    .iter()
                    .find_map(|e| e.revision(s))
                    .into_iter()
                    .flat_map(|r| &r.rules)
            }))
        {
            if let Some(reference) = &rule.request.response_asset {
                references.insert(reference.clone());
            }
        }
        let mut assets = Vec::new();
        let mut objects = BTreeMap::new();
        let mut snapshots = BTreeMap::new();
        for reference in references {
            if canceled.load(Ordering::Acquire) {
                return Err(invalid("Rule export canceled"));
            }
            let (asset, body) = self.assets.portable_snapshot(&reference, canceled)?;
            let provenance = match (&asset.provenance, &asset.linked_body) {
                (_, Some(_)) => "export-snapshot",
                (ResponseAssetProvenance::Session { .. }, _) => "captured-response",
                (ResponseAssetProvenance::Imported, _) => "imported",
                (ResponseAssetProvenance::Authored, _) => "authored",
            };
            let bytes = asset.body_bytes.to_string();
            objects.insert(
                asset.sha256.clone(),
                wire::Object {
                    sha256: asset.sha256.clone(),
                    bytes: bytes.clone(),
                    role: "response-body".into(),
                    media_type: asset
                        .media_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".into()),
                },
            );
            assets.push(wire::Asset { id: portable_id(&asset.id), revision: asset.revision.to_string(), status: asset.status, headers: wire::from_headers(&asset.headers), body: wire::Dependency { state: "present".into(), sha256: Some(asset.sha256.clone()), bytes: Some(bytes), reason: None, expected_sha256: None, representation: "encoded-entity".into() }, provenance: wire::Provenance { kind: provenance.into(), captured_source: None, acquired_at: if provenance == "export-snapshot" { Some(now()?) } else { None }, source_unavailable_reason: if provenance == "captured-response" { Some("Capture evidence is outside this rule collection; runtime exchange IDs are not portable source keys".into()) } else { None } } });
            snapshots.insert(asset.sha256, body);
        }
        let mut extensions = serde_json::Map::new();
        let selected_ids = rules
            .iter()
            .chain(experiments.values().flat_map(|e| {
                e.variants
                    .iter()
                    .flat_map(|v| v.revisions.iter().flat_map(|r| &r.rules))
            }))
            .map(|r| &r.id)
            .collect::<BTreeSet<_>>();
        let origins = document.portable_imports.iter().flat_map(|import| import.rule_ids.iter().filter(|(_, local)| selected_ids.contains(&portable_id(local))).map(move |(original, local)| (portable_id(local), serde_json::json!({"collectionId":import.source.id,"originalRuleId":original})))).collect::<serde_json::Map<_, _>>();
        if !origins.is_empty() {
            extensions.insert("transmog.rule-origins".into(), origins.into());
        }
        let collection = wire::Collection {
            format: "transmog-investigation".into(),
            schema_version: 0,
            kind: "rules".into(),
            id: request.id.clone(),
            revision: request.revision.clone(),
            title: request.title.clone(),
            created_at: now()?,
            producer: wire::Producer {
                name: "Transmog".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            required_features: vec![
                "response-assets/1".into(),
                "header-edits/1".into(),
                "literal-response/1".into(),
            ],
            objects: objects.into_values().collect(),
            assets,
            rules,
            experiments: experiments.into_values().collect(),
            extensions,
        };
        let manifest = collection.encode().map_err(portable_error)?;
        wire::PreparedCollection::prepare(
            &manifest,
            staging_parent,
            |sha| {
                let body = snapshots
                    .get(sha)
                    .ok_or_else(|| wire::Error::Invalid("Missing frozen export object".into()))?;
                Ok(Box::new(body.reopen()?))
            },
            canceled,
        )
        .map_err(portable_error)
    }
}
fn export_rule(value: &transmog_automation::Rule) -> Result<wire::Rule, AppError> {
    let mut rule = value.clone();
    rule.id = portable_id(&rule.id);
    if let Some(reference) = &rule.request.response_asset {
        let (id, revision) = reference
            .rsplit_once('@')
            .ok_or_else(|| invalid("Invalid saved response reference"))?;
        rule.request.response_asset = Some(format!("{}@{}", portable_id(id), revision));
    }
    wire::from_native(&rule).map_err(portable_error)
}
fn portable_id(id: &str) -> String {
    if wire::valid_id(id) {
        id.into()
    } else {
        format!("id-{}", wire::sha256(id.as_bytes()))
    }
}
fn now() -> Result<String, AppError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| invalid("Collection timestamp is unavailable"))
}
