use crate::{Collection, Error, Rule, decimal, invalid, valid_digest, valid_id};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::AtomicBool},
};
use transmog_automation::{
    AutomationLimits, AutomationResponse, ResponseAssetResolver, compile_with_assets,
};

fn identity(id: &str, revision: &str) -> Result<(), Error> {
    if !valid_id(id) {
        return Err(invalid("Invalid portable identity"));
    }
    decimal(revision, false)?;
    Ok(())
}
fn features(values: &[String]) -> Result<(), Error> {
    for value in values {
        if !matches!(
            value.as_str(),
            "response-assets/1" | "header-edits/1" | "literal-response/1"
        ) {
            return Err(Error::Unsupported(value.clone()));
        }
    }
    Ok(())
}
pub(crate) fn collection(value: &Collection) -> Result<(), Error> {
    if value.format != "transmog-investigation"
        || value.schema_version != 0
        || value.kind != "rules"
    {
        return Err(Error::Unsupported(
            "Expected current revision-0 rule collection".into(),
        ));
    }
    identity(&value.id, &value.revision)?;
    time::OffsetDateTime::parse(
        &value.created_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| invalid("Invalid collection creation time"))?;
    features(&value.required_features)?;
    if value
        .extensions
        .keys()
        .any(|key| key.len() > 128 || !key.contains('.') || !valid_id(key))
    {
        return Err(invalid("Extension keys require bounded ASCII namespaces"));
    }
    if value.title.trim().is_empty()
        || value.title.len() > 512
        || value.assets.len() > 16384
        || value.objects.len() > 16384
        || value.experiments.len() > 32
    {
        return Err(invalid("Collection metadata limits exceeded"));
    }
    groups(value, &assets(value)?)
}
fn assets(value: &Collection) -> Result<Arc<DeclaredAssets>, Error> {
    let mut objects = BTreeMap::new();
    for object in &value.objects {
        if !valid_digest(&object.sha256)
            || object.role != "response-body"
            || decimal(&object.bytes, true)? > 1024 * 1024 * 1024
            || objects.insert(&object.sha256, &object.bytes).is_some()
        {
            return Err(invalid("Invalid or duplicate response object"));
        }
    }
    let mut references = BTreeSet::new();
    let mut used = BTreeSet::new();
    for asset in &value.assets {
        identity(&asset.id, &asset.revision)?;
        if !references.insert(format!("{}@{}", asset.id, asset.revision))
            || !(200..=599).contains(&asset.status)
        {
            return Err(invalid("Invalid or duplicate asset revision"));
        }
        crate::convert::headers(&asset.headers)?;
        if asset.body.state != "present" {
            return Err(invalid("A rule import requires every selected asset body"));
        }
        let sha = asset
            .body
            .sha256
            .as_ref()
            .ok_or_else(|| invalid("Missing body digest"))?;
        if !objects.contains_key(sha)
            || asset.body.bytes.is_none()
            || asset.body.representation != "encoded-entity"
            || asset.body.bytes.as_ref() != objects.get(sha).copied()
            || asset.body.reason.is_some()
            || asset.body.expected_sha256.is_some()
        {
            return Err(invalid(
                "Asset body does not reference its exact encoded object",
            ));
        }
        used.insert(sha);
        provenance(&asset.provenance)?;
    }
    if used.len() != objects.len() {
        return Err(invalid("Unreferenced response object"));
    }
    Ok(Arc::new(DeclaredAssets(references)))
}
fn groups(value: &Collection, resolver: &Arc<DeclaredAssets>) -> Result<(), Error> {
    let mut definitions = BTreeMap::new();
    rules(&value.rules, resolver, &mut definitions)?;
    let mut owners = BTreeMap::new();
    let mut experiments = BTreeSet::new();
    for experiment in &value.experiments {
        identity(&experiment.id, "1")?;
        name(&experiment.label)?;
        features(&experiment.required_features)?;
        if experiment.kind != "response-variants" {
            return Err(Error::Unsupported(experiment.kind.clone()));
        }
        if !experiments.insert(&experiment.id)
            || experiment.variants.len() > 16
            || experiment.variants.is_empty()
        {
            return Err(invalid("Duplicate response set or variant bound exceeded"));
        }
        let mut variants = BTreeSet::new();
        for variant in &experiment.variants {
            identity(&variant.id, "1")?;
            name(&variant.label)?;
            if !variants.insert(&variant.id)
                || variant.revisions.is_empty()
                || variant.revisions.len() > 32
            {
                return Err(invalid("Invalid variant revisions"));
            }
            let mut previous = 0;
            for revision in &variant.revisions {
                let number = decimal(&revision.revision, false)?;
                if number <= previous {
                    return Err(invalid("Variant revision order is invalid"));
                }
                if revision.rules.len() > 1_000 {
                    return Err(invalid(
                        "A saved response revision can contain at most 1,000 responses",
                    ));
                }
                previous = number;
                if variant.id == "baseline"
                    && (variant.revisions.len() != 1 || !revision.rules.is_empty())
                {
                    return Err(invalid("Baseline must remain empty"));
                }
                for rule in &revision.rules {
                    if rule.kind != "auto-response"
                        || owners
                            .insert(&rule.id, &experiment.id)
                            .is_some_and(|previous| previous != &experiment.id)
                        || value.rules.iter().any(|r| r.id == rule.id)
                    {
                        return Err(invalid("Rule ownership conflicts"));
                    }
                }
                rules(&revision.rules, resolver, &mut definitions)?;
            }
        }
        if !variants.contains(&"baseline".to_owned()) {
            return Err(invalid("Missing empty Baseline"));
        }
        if let Some(active) = &experiment.active_intent
            && !experiment.variants.iter().any(|v| {
                v.id == active.variant_id
                    && v.revisions.iter().any(|r| r.revision == active.revision)
            })
        {
            return Err(invalid("Active intent references an unselected revision"));
        }
    }
    Ok(())
}
fn provenance(value: &crate::Provenance) -> Result<(), Error> {
    if value.kind == "export-snapshot" {
        let at = value
            .acquired_at
            .as_deref()
            .ok_or_else(|| invalid("Export snapshot requires acquisition time"))?;
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| invalid("Invalid snapshot acquisition time"))?;
    }
    if !matches!(
        value.kind.as_str(),
        "authored" | "captured-response" | "export-snapshot" | "imported"
    ) {
        return Err(Error::Unsupported("Asset provenance".into()));
    }
    if let Some(source) = &value.captured_source {
        if !valid_digest(&source.artifact_sha256)
            || source.source_key.is_empty()
            || source.source_key.len() > 512
            || source.exchange_id.as_ref().is_some_and(|id| id.len() > 512)
            || source.boundary != "client-response"
        {
            return Err(invalid("Invalid portable evidence reference"));
        }
        decimal(&source.occurrence, true)?;
    }
    Ok(())
}
fn name(value: &str) -> Result<(), Error> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(invalid("Response set/variant name exceeds native limits"));
    }
    Ok(())
}
fn rules<'a>(
    values: &'a [Rule],
    assets: &Arc<DeclaredAssets>,
    definitions: &mut BTreeMap<(&'a str, &'a str), &'a Rule>,
) -> Result<(), Error> {
    let mut native = Vec::new();
    let mut previous = None;
    for (position, value) in values.iter().enumerate() {
        if definitions
            .insert((&value.id, &value.revision), value)
            .is_some_and(|before| before != value)
        {
            return Err(invalid("A rule revision has conflicting definitions"));
        }
        let rule = crate::to_native(
            value,
            i32::try_from(position).map_err(|_| invalid("Too many rules"))?,
        )?;
        if value.matcher.examples.len() > 64 {
            return Err(invalid("Too many matcher examples"));
        }
        for example in &rule.matcher.examples {
            let headers = example
                .headers
                .iter()
                .map(|h| (h.name.clone(), h.value.clone()))
                .collect::<Vec<_>>();
            transmog_automation::request_for_test(&example.method, &example.url, &headers)
                .map_err(invalid)?;
        }
        if let Some(reference) = &rule.request.response_asset {
            assets.validate(reference).map_err(invalid)?;
            if previous.is_some_and(|priority| priority >= rule.priority) {
                return Err(invalid(
                    "Auto-response precedence disagrees with array order",
                ));
            }
            previous = Some(rule.priority);
        }
        native.push(rule);
    }
    compile_with_assets(native, AutomationLimits::default(), Some(assets.clone()))
        .map_err(|error| invalid(error.to_string()))?;
    Ok(())
}
struct DeclaredAssets(BTreeSet<String>);
impl ResponseAssetResolver for DeclaredAssets {
    fn validate(&self, reference: &str) -> Result<(), String> {
        if self.0.contains(reference) {
            Ok(())
        } else {
            Err("Missing declared response asset".into())
        }
    }
    fn resolve(&self, _: &str, _: &AtomicBool) -> Result<AutomationResponse, String> {
        Err("Validation never serves responses".into())
    }
}
