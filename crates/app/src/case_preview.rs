//! Explicit package previews never publish package rules into live automation.
use super::{AppError, Application, CaseEvidenceLink, check_cancel, invalid};
use crate::{CapturedPage, CapturedPageOptions, CapturedPageScope};
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, atomic::AtomicBool},
};
use transmog_portable as wire;

/// Explicit response and resource selection within an opened portable case.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CasePreviewOptions {
    /// Exact frozen variant, independently of workbench activation.
    pub selection: Option<wire::FrozenExperiment>,
    /// Explicitly participate all frozen ordinary auto-responses, including disabled definitions.
    #[serde(default)]
    pub ordinary: bool,
    /// Original source by default; all opened case evidence requires an explicit choice.
    #[serde(default)]
    pub scope: CapturedPageScope,
}

impl Application {
    /// Prepares a case-only scene and an optional frozen variant or saved auto-responses.
    /// # Errors
    /// Rejects missing evidence/assets, unsupported actions, changed identities or cancellation.
    pub async fn prepare_case_preview(
        &self,
        package: Arc<wire::Package>,
        links: Vec<CaseEvidenceLink>,
        id: String,
        options: CasePreviewOptions,
        canceled: Arc<AtomicBool>,
    ) -> Result<CapturedPage, AppError> {
        let app = self.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(prepare(&app, &package, &links, &id, &options, &canceled))
        })
        .await
        .map_err(|_| invalid("Case preview preparation failed"))?
    }
}

async fn prepare(
    app: &Application,
    package: &wire::Package,
    links: &[CaseEvidenceLink],
    id: &str,
    options: &CasePreviewOptions,
    canceled: &AtomicBool,
) -> Result<CapturedPage, AppError> {
    let selection = options.selection.as_ref();
    let ordinary = options.ordinary;
    check_cancel(canceled)?;
    let wire::PackageManifest::Case(manifest) = package.manifest() else {
        return Err(invalid("Open an investigation case first"));
    };
    if ordinary && selection.is_some() {
        return Err(invalid("Choose frozen saved rules or one variant"));
    }
    let sources = manifest
        .sources
        .iter()
        .map(|s| (&s.artifact_sha256, &s.source_key))
        .collect::<HashSet<_>>();
    let allowed = links
        .iter()
        .filter(|l| sources.contains(&(&l.reference.artifact_sha256, &l.reference.source_key)))
        .map(|l| l.exchange_id.clone())
        .collect::<HashSet<_>>();
    if !allowed.contains(id) {
        return Err(invalid(
            "Select an HTML request from this case’s opened evidence",
        ));
    }
    let options = CapturedPageOptions {
        scope: options.scope,
        ..Default::default()
    };
    let mut page = crate::captured_page::build(app, id, &options, canceled, Some(&allowed)).await?;
    let mut report = page.diagnostics.snapshot();
    report.source = format!(
        "Case {} · revision {} · {}",
        manifest.title,
        manifest.revision,
        if options.scope == CapturedPageScope::OriginalTrace {
            format!("Original source: {}", report.source)
        } else {
            "All opened case evidence".into()
        }
    );
    report.case_origin = Some(format!(
        "{}@{} · {}",
        manifest.id,
        manifest.revision,
        selection.map_or_else(
            || if ordinary {
                "frozen saved auto-responses".into()
            } else {
                "captured baseline".into()
            },
            |s| format!("{}/{}/{}", s.experiment_id, s.variant_id, s.revision)
        )
    ));
    page.diagnostics = crate::CapturedPageDiagnostics::new(report);
    if !ordinary && selection.is_none() {
        return Ok(page);
    }
    let collection = selected_collection(manifest, selection, ordinary)?;
    attach_package(package, page, &collection, ordinary, canceled).await
}
async fn attach_package(
    package: &wire::Package,
    page: CapturedPage,
    collection: &wire::Collection,
    ordinary: bool,
    canceled: &AtomicBool,
) -> Result<CapturedPage, AppError> {
    let selected = if ordinary {
        collection
            .rules
            .iter()
            .filter(|r| r.kind == "auto-response")
            .collect::<Vec<_>>()
    } else {
        collection
            .experiments
            .iter()
            .flat_map(|e| &e.variants)
            .flat_map(|v| &v.revisions)
            .flat_map(|r| &r.rules)
            .filter(|r| r.enabled_intent)
            .collect()
    };
    if selected.len() > 64 {
        return Err(invalid(
            "Case preview supports up to 64 participating auto-responses",
        ));
    }
    let mut rules = vec![];
    for rule in selected {
        if rule.kind != "auto-response" {
            return Err(invalid(
                "This variant includes live response transformations. Case previews support frozen auto-responses only; no rules were activated.",
            ));
        }
        rules.push(
            wire::to_native(
                rule,
                i32::try_from(rules.len()).map_err(|_| invalid("Too many case preview rules"))?,
            )
            .map_err(crate::rule_interchange::portable_error)?,
        );
    }
    let root = tempfile::tempdir().map_err(|_| invalid("Case preview storage unavailable"))?;
    let store = crate::response_assets::ResponseAssetStore::load(Some(root.path().join("assets")))?;
    let references = rules
        .iter()
        .filter_map(|r| r.request.response_asset.as_ref())
        .collect::<HashSet<_>>();
    let mut assets = vec![];
    let mut total = 0_u64;
    for asset in &collection.assets {
        if !references.contains(&format!("{}@{}", asset.id, asset.revision)) {
            continue;
        }
        check_cancel(canceled)?;
        let metadata =
            crate::response_assets::ResponseAssetStore::portable_metadata(asset, asset.id.clone())?;
        total = total.saturating_add(metadata.body_bytes);
        if metadata.body_bytes > 32 * 1024 * 1024 || total > 256 * 1024 * 1024 {
            return Err(invalid(
                "Case preview asset limit: 32 MiB per response / 256 MiB total",
            ));
        }
        let object = collection
            .objects
            .iter()
            .find(|o| o.sha256 == metadata.sha256)
            .ok_or_else(|| invalid("Frozen response object unavailable"))?;
        let mut input = package
            .open_object(&object.sha256)
            .map_err(crate::rule_interchange::portable_error)?;
        let mut file =
            tempfile::tempfile().map_err(|_| invalid("Case preview storage unavailable"))?;
        let mut buffer = vec![0; 128 * 1024];
        loop {
            check_cancel(canceled)?;
            let count = input
                .read(&mut buffer)
                .map_err(|_| invalid("Frozen response could not be read"))?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])
                .map_err(|_| invalid("Frozen response could not be written"))?;
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| invalid("Frozen response could not be read"))?;
        store.stage_portable_body(file, object, canceled)?;
        assets.push(metadata);
    }
    // The overlay takes its own immutable decoded copies before this temporary store closes.
    crate::captured_page_overlay::attach_selected(
        &store.with_imports(&assets),
        page,
        rules,
        None,
        canceled,
    )
    .await
}
fn selected_collection(
    manifest: &wire::CaseManifest,
    selection: Option<&wire::FrozenExperiment>,
    ordinary: bool,
) -> Result<wire::Collection, AppError> {
    let Some(selection) = selection else {
        return manifest
            .rules_for_import(&[], ordinary)
            .map_err(crate::rule_interchange::portable_error);
    };
    let value = manifest
        .experiments
        .iter()
        .find(|e| e["id"].as_str() == Some(&selection.experiment_id))
        .ok_or_else(|| invalid("The frozen response set is unavailable"))?;
    let mut experiment: wire::Experiment = serde_json::from_value(value.clone())
        .map_err(|_| invalid("Unsupported frozen response set semantics"))?;
    let variant = experiment
        .variants
        .iter_mut()
        .find(|v| v.id == selection.variant_id)
        .ok_or_else(|| invalid("The frozen variant is unavailable"))?;
    variant
        .revisions
        .retain(|r| r.revision == selection.revision);
    if variant.revisions.len() != 1 {
        return Err(invalid("The frozen response set revision is unavailable"));
    }
    experiment
        .variants
        .retain(|v| v.id == selection.variant_id || v.id == "baseline");
    experiment.active_intent = None;
    let mut narrowed = manifest.clone();
    narrowed.experiments = vec![
        serde_json::to_value(experiment).map_err(|_| invalid("Frozen response set is invalid"))?,
    ];
    narrowed
        .rules_for_import(std::slice::from_ref(&selection.experiment_id), false)
        .map_err(crate::rule_interchange::portable_error)
}
