//! Preview-only ordered rules and owned byte snapshots. No live activation or transport.
use crate::captured_page::{browser_headers, error};
use crate::{
    AppError, Application, CapturedPage, CapturedPageDiagnostics, CapturedPageOptions,
    CapturedResource, PreviewRuleSnapshot,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use transmog_automation::{CompiledRequestMatcher, Rule, request_for_test, same_matching_behavior};

#[derive(Default)]
pub(super) struct Overlay {
    rules: Vec<FrozenRule>,
    root: Option<tempfile::TempDir>,
}
struct FrozenRule {
    rule: Rule,
    matcher: CompiledRequestMatcher,
    resource: CapturedResource,
}
pub(super) enum Resolution {
    Served(CapturedResource),
    Fallback(String),
}
impl Overlay {
    pub(super) fn resolve(
        &self,
        method: &str,
        url: &str,
        headers: Option<&[(String, String)]>,
        document: bool,
        diagnostics: &CapturedPageDiagnostics,
    ) -> Resolution {
        if self.rules.is_empty() {
            return Resolution::Fallback(String::new());
        }
        let Ok(request) = request_for_test(method, url, headers.unwrap_or_default()) else {
            return Resolution::Fallback(
                "; rule matching unavailable: malformed or over-budget browser request".into(),
            );
        };
        let mut reasons = Vec::new();
        for frozen in &self.rules {
            if headers.is_none() && !frozen.rule.matcher.request_headers.is_empty() {
                reasons.push(format!("{}: request headers unavailable", frozen.rule.id));
                continue;
            }
            let result = frozen.matcher.evaluate(&request);
            if result.matched {
                let mut resource = frozen.resource.clone();
                if method.eq_ignore_ascii_case("HEAD") {
                    resource.body_path = self
                        .root
                        .as_ref()
                        .expect("overlay storage")
                        .path()
                        .join("empty");
                    resource.sha256 = empty_hash();
                }
                // WebView2 cannot show these as a top-level document. Preserve the status;
                // the native boundary reports a navigation failure rather than inventing HTML.
                let reason = if document && matches!(resource.status, 204 | 205 | 304) {
                    "Winning auto-response has no renderable document body"
                } else {
                    "First participating saved rule matched actual browser method, URL and available explicit headers"
                };
                if document && matches!(resource.status, 204 | 205 | 304) {
                    diagnostics.failed_response(method, url, frozen.rule.id.clone(), reason);
                } else {
                    diagnostics.supplied(
                        method,
                        url,
                        None,
                        Some(frozen.rule.id.clone()),
                        &resource,
                        reason,
                    );
                }
                return Resolution::Served(resource);
            }
            let failed = result
                .checks
                .iter()
                .filter(|check| !check.matched)
                .map(|check| check.label.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            // Condition names/results only. MatchTest detail/captures can contain secrets.
            if reasons.len() < 8 {
                reasons.push(format!("{}: {failed} did not match", frozen.rule.id));
            }
        }
        Resolution::Fallback(format!("; overlay: {}", reasons.join("; ")))
    }
}
fn empty_hash() -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest([]))
}

fn selected_rules(
    application: &Application,
    options: &CapturedPageOptions,
) -> Result<Vec<Rule>, AppError> {
    if let Some(selection) = &options.variant {
        if options.rule_ids.is_some() {
            return Err(error(
                "Choose either a variant or individual rules for preview",
            ));
        }
        let status = application.automation.status();
        return status
            .experiments
            .iter()
            .find(|e| e.id == selection.experiment_id)
            .and_then(|e| e.revision(selection))
            .map(|r| r.rules.iter().filter(|r| r.enabled).cloned().collect())
            .ok_or_else(|| error("Saved variant revision is unavailable; reopen preview choices"));
    }
    let Some(ids) = &options.rule_ids else {
        return Ok(Vec::new());
    };
    if ids.is_empty() || ids.len() > 64 {
        return Err(error("Select 1–64 saved auto-responses for this preview"));
    }
    let status = application.automation.status();
    if options
        .generation
        .is_some_and(|generation| generation != status.generation)
    {
        return Err(error(
            "Saved rules changed; reopen the preview choices and select rules again",
        ));
    }
    let mut selected = Vec::new();
    let mut unique = std::collections::HashSet::new();
    for id in ids {
        if !unique.insert(id) {
            return Err(error("A preview rule was selected more than once"));
        }
        selected.push(
            status
                .rules
                .iter()
                .find(|r| &r.id == id && r.request.response_asset.is_some())
                .cloned()
                .ok_or_else(|| {
                    error("A selected saved rule is unavailable; reopen preview choices")
                })?,
        );
    }
    // Same order as the live compiler/editor, including ties. The live gate and enabled
    // bits deliberately do not govern explicit preview participation.
    selected.sort_by(|a, b| (a.priority, &a.id).cmp(&(b.priority, &b.id)));
    Ok(selected)
}

pub(super) async fn attach(
    application: &Application,
    mut page: CapturedPage,
    options: &CapturedPageOptions,
    canceled: &AtomicBool,
) -> Result<CapturedPage, AppError> {
    let selected = selected_rules(application, options)?;
    let mut report = page.diagnostics.snapshot();
    report.variant = options.variant.clone();
    if selected.is_empty() {
        page.diagnostics = CapturedPageDiagnostics::new(report);
        return Ok(page);
    }
    let root = tempfile::Builder::new()
        .prefix("transmog-preview-rules-")
        .tempdir()
        .map_err(|_| error("Preview response storage is unavailable"))?;
    std::fs::write(root.path().join("empty"), [])
        .map_err(|_| error("Preview response storage is unavailable"))?;
    let mut rules: Vec<FrozenRule> = Vec::new();
    for rule in selected {
        if canceled.load(Ordering::Acquire) {
            return Err(error("Captured page preview canceled"));
        }
        let name = rule.display_name.clone().unwrap_or_else(|| rule.id.clone());
        let reference = rule
            .request
            .response_asset
            .as_deref()
            .expect("selected response");
        let matcher = CompiledRequestMatcher::new(&rule.matcher)
            .map_err(|why| error(&format!("{name}: {why}")))?;
        let path = root.path().join(format!("rule-{}", rules.len()));
        let (asset, mut bytes, mut sha256) = application
            .response_assets
            .freeze_preview(
                reference,
                &path,
                (32 * 1024 * 1024).min((256 * 1024 * 1024_u64).saturating_sub(report.bytes)),
                canceled,
            )
            .await
            .map_err(|why| error(&format!("{name}: {}", why.message)))?;
        if !(200..=599).contains(&asset.status) {
            return Err(error(&format!("{name}: HTTP status cannot be rendered")));
        }
        if matches!(asset.status, 204 | 205 | 304) {
            std::fs::write(&path, [])
                .map_err(|_| error("Frozen response could not be normalized"))?;
            bytes = 0;
            sha256 = empty_hash();
        }
        let shadowed_by = rules
            .iter()
            .find(|earlier| {
                same_matching_behavior(&earlier.rule.matcher, &rule.matcher).unwrap_or(false)
            })
            .map(|r| r.rule.id.clone());
        report.rules.push(PreviewRuleSnapshot {
            id: rule.id.clone(),
            name,
            revision: rule.revision,
            asset: reference.into(),
            sha256: sha256.clone(),
            bytes,
            shadowed_by,
        });
        report.bytes += bytes;
        let head = transmog_core::ResponseHead {
            status: asset.status,
            headers: asset.headers,
            source_version: transmog_core::HttpLegVersion::Http1,
        };
        let headers = browser_headers(&head, bytes);
        rules.push(FrozenRule {
            rule,
            matcher,
            resource: CapturedResource {
                status: asset.status,
                reason: http::StatusCode::from_u16(asset.status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or("")
                    .into(),
                headers,
                body_path: path,
                sha256,
            },
        });
    }
    page.overlay = Overlay {
        rules,
        root: Some(root),
    };
    page.diagnostics = CapturedPageDiagnostics::new(report);
    Ok(page)
}

impl Application {
    /// Prepares an independent run from retained capture bytes and current saved rules.
    ///
    /// # Errors
    /// Reports stale selections, unavailable assets, byte limits or cancellation.
    pub async fn prepare_related_captured_page(
        &self,
        page: Arc<CapturedPage>,
        options: CapturedPageOptions,
        canceled: Arc<AtomicBool>,
    ) -> Result<CapturedPage, AppError> {
        let application = self.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(attach(
                &application,
                crate::captured_page::fork(&page),
                &options,
                &canceled,
            ))
        })
        .await
        .map_err(|_| error("Preview preparation failed"))?
    }
}
