//! Review/publish shares the registry's lock and one persisted document boundary.
use super::{ActiveAutomation, AutomationRegistry};
use crate::rule_interchange::{
    ImportedCollection, PreparedRuleImport, RuleImportItem, RuleImportReview, fresh_id,
    imported_assets, invalid, portable_error,
};
use crate::{
    AppError, AutomationStatus, Experiment, ExperimentRevision, ExperimentVariant,
    response_assets::ResponseAssetStore,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

impl AutomationRegistry {
    #[allow(clippy::too_many_lines)] // Keep the three reviewed identity domains in one nonmutating pass.
    pub(crate) fn review_portable(
        &self,
        prepared: transmog_portable::PreparedCollection,
    ) -> Result<PreparedRuleImport, AppError> {
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .document
            .clone();
        let source = prepared.manifest().clone();
        let mut mapping = ImportedCollection {
            source: source.clone(),
            assets: Vec::new(),
            asset_refs: BTreeMap::new(),
            rule_ids: BTreeMap::new(),
            experiment_ids: BTreeMap::new(),
            variant_ids: BTreeMap::new(),
        };
        let mut items = Vec::new();
        for asset in &source.assets {
            let reference = format!("{}@{}", asset.id, asset.revision);
            let local = format!("{}@{}", fresh_id("asset")?, asset.revision);
            let prior = current
                .portable_imports
                .iter()
                .flat_map(|c| &c.source.assets)
                .filter(|a| a.id == asset.id && a.revision == asset.revision)
                .collect::<Vec<_>>();
            let state = classify(
                prior.contains(&asset),
                !prior.is_empty()
                    || self
                        .assets
                        .list()
                        .iter()
                        .any(|a| a.asset_ref() == reference),
            );
            items.push(item("asset", &asset.id, &asset.revision, state, &local));
            mapping.asset_refs.insert(reference, local);
        }
        let mut rules = BTreeMap::new();
        for rule in source
            .rules
            .iter()
            .chain(source.experiments.iter().flat_map(|e| {
                e.variants
                    .iter()
                    .flat_map(|v| v.revisions.iter().flat_map(|r| &r.rules))
            }))
        {
            rules.insert((&rule.id, &rule.revision), rule);
        }
        for rule in rules.into_values() {
            let local = match mapping.rule_ids.get(&rule.id) {
                Some(id) => id.clone(),
                None => fresh_id("rule")?,
            };
            let prior = current
                .portable_imports
                .iter()
                .flat_map(|c| {
                    c.source
                        .rules
                        .iter()
                        .chain(c.source.experiments.iter().flat_map(|e| {
                            e.variants
                                .iter()
                                .flat_map(|v| v.revisions.iter().flat_map(|r| &r.rules))
                        }))
                })
                .filter(|r| r.id == rule.id && r.revision == rule.revision)
                .collect::<Vec<_>>();
            let state = classify(
                prior.contains(&rule),
                !prior.is_empty() || current.rules.iter().any(|r| r.id == rule.id),
            );
            items.push(item("rule", &rule.id, &rule.revision, state, &local));
            mapping.rule_ids.insert(rule.id.clone(), local);
        }
        for experiment in &source.experiments {
            for variant in &experiment.variants {
                mapping.variant_ids.insert(
                    format!("{}/{}", experiment.id, variant.id),
                    if variant.id == "baseline" {
                        "baseline".into()
                    } else {
                        fresh_id("variant")?
                    },
                );
            }
            let local = fresh_id("experiment")?;
            let prior = current
                .portable_imports
                .iter()
                .flat_map(|c| &c.source.experiments)
                .filter(|e| e.id == experiment.id)
                .collect::<Vec<_>>();
            items.push(item(
                "experiment",
                &experiment.id,
                "1",
                classify(
                    prior.contains(&experiment),
                    !prior.is_empty() || current.experiments.iter().any(|e| e.id == experiment.id),
                ),
                &local,
            ));
            mapping.experiment_ids.insert(experiment.id.clone(), local);
        }
        let review = RuleImportReview {
            generation: current.generation,
            title: source.title.clone(),
            source,
            items,
            activation: "disabled",
        };
        Ok(PreparedRuleImport {
            prepared,
            review,
            mapping,
            owner: Arc::downgrade(&self.mutation),
        })
    }
    #[allow(clippy::too_many_lines)] // Keep validation, persistence and one publication boundary together.
    pub(crate) fn import_portable(
        &self,
        mut prepared: PreparedRuleImport,
        canceled: &AtomicBool,
    ) -> Result<AutomationStatus, AppError> {
        if !std::sync::Weak::ptr_eq(&prepared.owner, &Arc::downgrade(&self.mutation)) {
            return Err(invalid(
                "This import review belongs to another workspace. Review it here before importing.",
            ));
        }
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _assets = self.assets.portable_lock();
        let current = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if current.document.generation != prepared.review.generation {
            return Err(AppError::new(
                crate::ErrorCategory::Conflict,
                "Automation changed after import review. Review again; existing work is preserved.",
                true,
            ));
        }
        if canceled.load(Ordering::Acquire) {
            return Err(invalid("Rule import canceled"));
        }
        let source = prepared.prepared.manifest();
        for object in &source.objects {
            let file = prepared
                .prepared
                .open_object(&object.sha256)
                .map_err(portable_error)?;
            self.assets.stage_portable_body(file, object, canceled)?;
        }
        for asset in &source.assets {
            let reference =
                &prepared.mapping.asset_refs[&format!("{}@{}", asset.id, asset.revision)];
            let id = reference
                .rsplit_once('@')
                .ok_or_else(|| invalid("Invalid mapped asset"))?
                .0;
            prepared
                .mapping
                .assets
                .push(ResponseAssetStore::portable_metadata(asset, id.into())?);
        }
        let mut document = current.document.clone();
        let mut ids = document
            .rules
            .iter()
            .map(|r| r.id.clone())
            .collect::<BTreeSet<_>>();
        for (position, rule) in source.rules.iter().enumerate() {
            let mut rule = mapped_rule(rule, position, &prepared.mapping)?;
            if !ids.insert(rule.id.clone()) {
                return Err(invalid("Import ID collision"));
            }
            rule.enabled = false;
            document.rules.push(rule);
        }
        for experiment in &source.experiments {
            document
                .experiments
                .push(import_experiment(experiment, &prepared.mapping)?);
        }
        document.portable_imports.push(prepared.mapping);
        document.generation = document
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("Automation generation exhausted"))?;
        super::validate_document(&document)?;
        let assets = imported_assets(&document);
        if !self.assets.import_catalog_valid(&assets) {
            return Err(invalid("Import asset identity collision"));
        }
        let resolver = Arc::new(self.assets.with_imports(&assets));
        super::compile_document(&document, self.limits, resolver.clone())?;
        for experiment in &document.experiments {
            for revision in experiment.variants.iter().flat_map(|v| &v.revisions) {
                transmog_automation::compile_with_assets(
                    revision.rules.clone(),
                    self.limits,
                    Some(resolver.clone()),
                )
                .map_err(super::compile_error)?;
            }
        }
        if canceled.load(Ordering::Acquire) {
            return Err(invalid("Rule import canceled"));
        }
        if let Some(path) = self.path.as_deref() {
            super::persist(path, &document)?;
        } else {
            super::persistence_bytes(&document)?;
        }
        self.assets.publish_imports(&assets);
        let imported_assets = crate::rule_interchange::asset_index(&document);
        // Existing registrations and admitted requests retain exactly their previous behavior.
        *self
            .active
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(ActiveAutomation {
            document,
            compiled: current.compiled.clone(),
            imported_assets,
        });
        Ok(self.status())
    }
}
fn classify(identical: bool, exists: bool) -> &'static str {
    if identical {
        "identical-source-definition"
    } else if exists {
        "conflicting"
    } else {
        "new"
    }
}
fn item(kind: &str, id: &str, revision: &str, state: &str, local: &str) -> RuleImportItem {
    RuleImportItem {
        kind: kind.into(),
        portable_id: id.into(),
        revision: revision.into(),
        state: state.into(),
        local_id: local.into(),
    }
}
fn mapped_rule(
    rule: &transmog_portable::Rule,
    position: usize,
    mapping: &ImportedCollection,
) -> Result<transmog_automation::Rule, AppError> {
    let mut result = transmog_portable::to_native(
        rule,
        i32::try_from(position).map_err(|_| invalid("Rule order overflow"))?,
    )
    .map_err(portable_error)?;
    result.id.clone_from(&mapping.rule_ids[&rule.id]);
    if let Some(reference) = &result.request.response_asset {
        result.request.response_asset = Some(
            mapping
                .asset_refs
                .get(reference)
                .ok_or_else(|| invalid("Unmapped response dependency"))?
                .clone(),
        );
    }
    Ok(result)
}
fn import_experiment(
    source: &transmog_portable::Experiment,
    mapping: &ImportedCollection,
) -> Result<Experiment, AppError> {
    let mut variants = Vec::new();
    for variant in &source.variants {
        let revisions = variant
            .revisions
            .iter()
            .enumerate()
            .map(|(index, revision)| {
                Ok(ExperimentRevision {
                    revision: u64::try_from(index).map_err(|_| invalid("Revision overflow"))? + 1,
                    rules: revision
                        .rules
                        .iter()
                        .enumerate()
                        .map(|(position, r)| mapped_rule(r, position, mapping))
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, AppError>>()?;
        variants.push(ExperimentVariant {
            id: mapping.variant_ids[&format!("{}/{}", source.id, variant.id)].clone(),
            name: variant.label.clone(),
            archived: false,
            revisions,
        });
    }
    Ok(Experiment::imported(
        mapping.experiment_ids[&source.id].clone(),
        source.label.clone(),
        variants,
    ))
}

#[path = "rule_export.rs"]
mod export;
#[cfg(test)]
#[path = "rule_interchange_tests.rs"]
mod rule_interchange_tests;
