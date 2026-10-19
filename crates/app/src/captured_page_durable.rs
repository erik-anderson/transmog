//! Path-free, immutable scene representation for durable demo storage.
use super::{
    CapturedPage, CapturedResource, MAX_RESOURCE_BYTES, MAX_RESOURCES, MAX_SCENE_BYTES,
    ResourceVariant, canonical_url, error,
};
use crate::{AppError, CapturedPageDiagnostics, CapturedPageReport};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DurableResource {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) status: u16,
    pub(crate) reason: String,
    pub(crate) headers: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableVariant {
    method: String,
    url: String,
    resource: DurableResource,
    entry_id: String,
    body_sha256: Option<String>,
    vary: Option<Vec<(String, Option<String>)>>,
    primary: bool,
    user_agent: Option<String>,
    same_process: bool,
    origin: Option<String>,
    referer: Option<String>,
    sec_headers: Vec<(String, String)>,
    time: u64,
    // JSON numbers cannot represent all capture IDs in every presentation layer.
    order: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DurableScene {
    url: String,
    user_agent: Option<String>,
    resources: usize,
    skipped: usize,
    navigation_time: u64,
    variants: Vec<DurableVariant>,
    overlay: Vec<crate::captured_page_overlay::DurableRule>,
    report: CapturedPageReport,
}

impl DurableScene {
    pub(crate) fn freeze(
        page: &CapturedPage,
        store: &mut impl FnMut(&CapturedResource) -> Result<DurableResource, AppError>,
    ) -> Result<Self, AppError> {
        let mut variants = Vec::new();
        let mut keys = page.files.iter().collect::<Vec<_>>();
        keys.sort_by_key(|(key, _)| *key);
        for ((method, url), choices) in keys {
            for variant in choices {
                variants.push(DurableVariant {
                    method: method.clone(),
                    url: url.clone(),
                    resource: store(&variant.resource)?,
                    entry_id: variant.entry_id.clone(),
                    body_sha256: variant.body_sha256.clone(),
                    vary: variant.vary.clone(),
                    primary: variant.primary,
                    user_agent: variant.user_agent.clone(),
                    same_process: variant.same_process,
                    origin: variant.origin.clone(),
                    referer: variant.referer.clone(),
                    sec_headers: variant.sec_headers.clone(),
                    time: variant.time,
                    order: format!("{:032x}", variant.order),
                });
            }
        }
        let mut report = page.diagnostics.snapshot();
        reset_report(&mut report);
        let scene = Self {
            url: page.url.clone(),
            user_agent: page.user_agent.clone(),
            resources: page.resources,
            skipped: page.skipped,
            navigation_time: page.navigation_time,
            variants,
            overlay: page.overlay.freeze(store)?,
            report,
        };
        scene.validate()?;
        Ok(scene)
    }

    pub(crate) fn objects(&self) -> BTreeMap<String, u64> {
        self.variants
            .iter()
            .map(|v| &v.resource)
            .chain(self.overlay.iter().map(|r| &r.resource))
            .map(|resource| (resource.sha256.clone(), resource.bytes))
            .collect()
    }

    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if canonical_url(&self.url).as_deref() != Some(&self.url)
            || self.variants.is_empty()
            || self.variants.len() > MAX_RESOURCES + 1
            || self.resources > MAX_RESOURCES
            || self.overlay.len() > crate::automation::MAX_RESPONSES
            || !self
                .variants
                .iter()
                .any(|v| v.primary && v.method == "GET" && v.url == self.url)
        {
            return Err(error("Saved demo contains an invalid captured page"));
        }
        let mut total = 0_u64;
        for resource in self
            .variants
            .iter()
            .map(|v| &v.resource)
            .chain(self.overlay.iter().map(|r| &r.resource))
        {
            if !valid_hash(&resource.sha256)
                || resource.bytes > MAX_RESOURCE_BYTES
                || !(200..=599).contains(&resource.status)
                || resource.headers.len() > 256 * 1024
                || resource.reason.len() > 1024
            {
                return Err(error("Saved demo contains an invalid frozen response"));
            }
            total = total.saturating_add(resource.bytes);
        }
        // A selected POST also has a GET navigation alias for the same bytes.
        let aliases = self
            .variants
            .iter()
            .filter(|v| {
                v.primary
                    && self.variants.iter().any(|other| {
                        !other.primary
                            && other.entry_id == v.entry_id
                            && other.resource.sha256 == v.resource.sha256
                    })
            })
            .map(|v| v.resource.bytes)
            .sum::<u64>();
        if total.saturating_sub(aliases) > MAX_SCENE_BYTES
            || self.report.bytes != total.saturating_sub(aliases)
        {
            return Err(error("Saved demo exceeds the 256 MiB scene limit"));
        }
        for variant in &self.variants {
            if variant.method.len() > 32
                || variant.url.len() > 16 * 1024
                || canonical_url(&variant.url).as_deref() != Some(&variant.url)
                || u128::from_str_radix(&variant.order, 16).is_err()
                || variant.vary.as_ref().is_some_and(|v| v.len() > 32)
                || variant.sec_headers.len() > 64
            {
                return Err(error(
                    "Saved demo contains invalid captured request preferences",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn hydrate(
        self,
        root: Arc<tempfile::TempDir>,
        load: &mut impl FnMut(&DurableResource) -> Result<CapturedResource, AppError>,
    ) -> Result<CapturedPage, AppError> {
        self.validate()?;
        let mut files: HashMap<(String, String), Vec<ResourceVariant>> = HashMap::new();
        for variant in self.variants {
            let resource = load(&variant.resource)?;
            files
                .entry((variant.method, variant.url))
                .or_default()
                .push(ResourceVariant {
                    resource,
                    entry_id: variant.entry_id,
                    body_sha256: variant.body_sha256,
                    vary: variant.vary,
                    primary: variant.primary,
                    user_agent: variant.user_agent,
                    same_process: variant.same_process,
                    origin: variant.origin,
                    referer: variant.referer,
                    sec_headers: variant.sec_headers,
                    time: variant.time,
                    order: u128::from_str_radix(&variant.order, 16)
                        .map_err(|_| error("Invalid saved demo capture order"))?,
                });
        }
        let overlay =
            crate::captured_page_overlay::Overlay::hydrate(self.overlay, root.clone(), load)?;
        let mut report = self.report;
        reset_report(&mut report);
        Ok(CapturedPage {
            url: self.url,
            user_agent: self.user_agent,
            resources: self.resources,
            skipped: self.skipped,
            files: Arc::new(files),
            overlay,
            diagnostics: CapturedPageDiagnostics::new(report),
            navigation_time: self.navigation_time,
            positions: Mutex::new(HashMap::new()),
            root,
        })
    }
}

pub(crate) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn reset_report(report: &mut CapturedPageReport) {
    report.hits = 0;
    report.misses = 0;
    report.failures = 0;
    report.requests.clear();
    report.scripts_enabled = None;
    report.runtime = "WebView2 (version unavailable)".into();
    report.bytes_available = true;
}
