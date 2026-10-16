use crate::{
    CaseManifest, Collection, Error, Experiment, Object, decimal, invalid, valid_digest, valid_id,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn check(value: &CaseManifest) -> Result<(), Error> {
    if value.format != "transmog-investigation" || value.schema_version != 0 || value.kind != "case"
    {
        return Err(Error::Unsupported("Expected revision-0 case".into()));
    }
    for feature in &value.required_features {
        if !matches!(
            feature.as_str(),
            "native-evidence/1"
                | "response-assets/1"
                | "header-edits/1"
                | "literal-response/1"
                | "context-objects/1"
                | "comparisons/1"
        ) {
            return Err(Error::Unsupported(feature.clone()));
        }
    }
    if value.objects.len() > 16384
        || value.sources.len() > 10000
        || value.notes.len() > 1024
        || value.contexts.len() > 256
        || value.runs.len() > 1024
        || value.markers.len() > 10000
        || value.experiments.len() > 32
        || value.comparisons.len() > 16
    {
        return Err(invalid("Case inventory limits exceeded"));
    }
    unique(value.sources.iter().map(|s| s.id.as_str()))?;
    unique(value.notes.iter().map(|n| n.id.as_str()))?;
    unique(value.markers.iter().map(|n| n.id.as_str()))?;
    unique(value.contexts.iter().map(|n| n.id.as_str()))?;
    unique(value.runs.iter().map(|n| n.id.as_str()))?;
    let mut objects = BTreeMap::new();
    let mut total = 0_u64;
    for object in &value.objects {
        let bytes = decimal(&object.bytes, true)?;
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("Case object lengths overflow"))?;
        let bound = match object.role.as_str() {
            "capture" => u64::MAX,
            "response-body" => 1024 * 1024 * 1024,
            "context" => 8 * 1024 * 1024,
            "script-source" => 1024 * 1024,
            _ => return Err(Error::Unsupported("Unknown object role".into())),
        };
        if !valid_digest(&object.sha256)
            || bytes > bound
            || object.media_type.len() > 256
            || objects.insert(&object.sha256, object).is_some()
        {
            return Err(invalid("Invalid/duplicate case object"));
        }
    }
    source_inventory(value, &objects)?;
    note_inventory(value)?;
    context_inventory(value, &objects)?;
    // Validate native rules against declared assets even when the evidence is missing.
    let base = validation_collection(value)?;
    base.encode()?;
    experiments(value, &base)?;
    run_inventory(value, &objects)?;
    comparison_inventory(value)?;
    Ok(())
}

fn comparison_inventory(value: &CaseManifest) -> Result<(), Error> {
    unique(value.comparisons.iter().map(|c| c.id.as_str()))?;
    for c in &value.comparisons {
        if !matches!(c.kind.as_str(), "exchange" | "captures")
            || c.engine.is_empty()
            || c.engine.len() > 128
            || c.label.len() > 512
            || c.recorded_summary.len() > 8192
            || c.left.len() > 10000
            || c.right.len() > 10000
            || c.corrections.len() > 20000
            || (c.kind == "exchange"
                && (c.left.len() != 1 || c.right.len() != 1 || !c.corrections.is_empty()))
            || !matches!(
                c.request_boundary.as_str(),
                "client-request" | "upstream-request"
            )
        {
            return Err(invalid("Invalid saved comparison"));
        }
        let settings = &c.settings;
        if (c.kind == "captures"
            && (c.request_boundary != "client-request"
                || !settings.excluded_headers.is_empty()
                || settings.normalize_newlines
                || c.left
                    .iter()
                    .chain(&c.right)
                    .any(|r| r.boundary != "client-response")))
            || (c.kind == "exchange"
                && (!settings.excluded_query_names.is_empty()
                    || !settings.request_headers.is_empty()
                    || settings.request_body_digest))
            || settings
                .excluded_headers
                .iter()
                .chain(&settings.request_headers)
                .any(|name| !name.is_ascii())
        {
            return Err(invalid("Unsupported comparison representation/settings"));
        }
        for (names, limit) in [
            (&settings.excluded_headers, 64),
            (&settings.excluded_query_names, 64),
            (&settings.request_headers, 32),
        ] {
            if names.len() > limit
                || names
                    .iter()
                    .any(|s| s.is_empty() || s.len() > 256 || s.contains(['\r', '\n', '\0']))
            {
                return Err(invalid("Invalid saved comparison settings"));
            }
        }
        let mut sides = [BTreeSet::new(), BTreeSet::new()];
        for (refs, seen) in [&c.left, &c.right].into_iter().zip(&mut sides) {
            for r in refs {
                evidence(value, r)?;
                if !matches!(r.boundary.as_str(), "client-response" | "upstream-response")
                    || !seen.insert(serde_json::to_string(r)?)
                {
                    return Err(invalid("Duplicate or invalid comparison subject"));
                }
            }
        }
        let mut used_left = BTreeSet::new();
        let mut used_right = BTreeSet::new();
        for pair in &c.corrections {
            if pair.left.is_none() && pair.right.is_none() {
                return Err(invalid("Empty pairing correction"));
            }
            if let Some(r) = &pair.left
                && (!sides[0].contains(&serde_json::to_string(r)?)
                    || !used_left.insert(serde_json::to_string(r)?))
            {
                return Err(invalid("Invalid left pairing correction"));
            }
            if let Some(r) = &pair.right
                && (!sides[1].contains(&serde_json::to_string(r)?)
                    || !used_right.insert(serde_json::to_string(r)?))
            {
                return Err(invalid("Invalid right pairing correction"));
            }
        }
    }
    Ok(())
}

fn experiments(value: &CaseManifest, base: &Collection) -> Result<(), Error> {
    unique(value.experiments.iter().map(|e| {
        e.get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    }))?;
    let mut jointly_validated = base.clone();
    for experiment in &value.experiments {
        let kind = experiment
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("Experiment kind is required"))?;
        if kind.is_empty()
            || kind.len() > 128
            || !experiment
                .get("requiredFeatures")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|features| {
                    features.len() <= 64
                        && features
                            .iter()
                            .all(|f| f.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128))
                })
        {
            return Err(invalid("Invalid experiment capability declaration"));
        }
        if !supported(experiment) {
            continue;
        }
        let experiment: Experiment = serde_json::from_value(experiment.clone())?;
        let mut candidate = base.clone();
        candidate.experiments = vec![experiment.clone()];
        match candidate.encode() {
            Ok(_) => jointly_validated.experiments.push(experiment),
            Err(Error::Unsupported(_)) => {}
            Err(error) => return Err(error),
        }
    }
    jointly_validated.encode()?;
    Ok(())
}
pub(crate) fn supported(value: &serde_json::Value) -> bool {
    value.get("kind").and_then(serde_json::Value::as_str) == Some("response-variants")
        && value
            .get("requiredFeatures")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|features| {
                features.iter().all(|f| {
                    matches!(
                        f.as_str(),
                        Some("response-assets/1" | "header-edits/1" | "literal-response/1")
                    )
                })
            })
}
pub(crate) fn validation_collection(value: &CaseManifest) -> Result<Collection, Error> {
    let mut assets = value.assets.clone();
    let mut objects = BTreeMap::new();
    for asset in &mut assets {
        availability(&asset.body.state, asset.body.reason.as_deref())?;
        if asset.body.state == "present" {
            let sha = asset.body.sha256.as_deref().unwrap_or("");
            let actual = value
                .objects
                .iter()
                .find(|o| o.sha256 == sha && o.role == "response-body")
                .ok_or_else(|| invalid("Asset object is not declared"))?;
            objects.insert(actual.sha256.clone(), actual.clone());
        } else {
            if asset.body.sha256.is_some()
                || asset.body.bytes.is_some()
                || asset.body.representation != "encoded-entity"
            {
                return Err(invalid(
                    "Unavailable assets require expected digests, not present object references",
                ));
            }
            if asset
                .body
                .expected_sha256
                .as_ref()
                .is_some_and(|s| !valid_digest(s))
            {
                return Err(invalid("Invalid expected asset digest"));
            }
            let sha = "0".repeat(64);
            asset.body = crate::Dependency {
                state: "present".into(),
                sha256: Some(sha.clone()),
                bytes: Some("0".into()),
                reason: None,
                expected_sha256: None,
                representation: "encoded-entity".into(),
            };
            objects.insert(
                sha.clone(),
                Object {
                    sha256: sha,
                    bytes: "0".into(),
                    role: "response-body".into(),
                    media_type: "application/octet-stream".into(),
                },
            );
        }
        if let Some(reference) = &asset.provenance.captured_source {
            evidence(value, reference)?;
        }
    }
    Ok(Collection {
        format: value.format.clone(),
        schema_version: value.schema_version,
        kind: "rules".into(),
        id: value.id.clone(),
        revision: value.revision.clone(),
        title: value.title.clone(),
        created_at: value.created_at.clone(),
        producer: value.producer.clone(),
        required_features: vec![],
        objects: objects.into_values().collect(),
        assets,
        rules: value.rules.clone(),
        experiments: vec![],
        extensions: value.extensions.clone(),
    })
}
pub(crate) fn evidence(
    value: &CaseManifest,
    reference: &crate::EvidenceReference,
) -> Result<(), Error> {
    source(
        value,
        &crate::SourceAssociation {
            artifact_sha256: reference.artifact_sha256.clone(),
            source_key: reference.source_key.clone(),
        },
    )?;
    decimal(&reference.occurrence, true)?;
    if reference
        .exchange_id
        .as_ref()
        .is_some_and(|id| id.len() > 512)
        || !matches!(
            reference.boundary.as_str(),
            "client-request" | "upstream-request" | "upstream-response" | "client-response"
        )
    {
        return Err(invalid("Invalid evidence boundary/original identity"));
    }
    Ok(())
}
fn source(value: &CaseManifest, reference: &crate::SourceAssociation) -> Result<(), Error> {
    association(&reference.artifact_sha256, &reference.source_key)?;
    if !value.sources.iter().any(|s| {
        s.artifact_sha256 == reference.artifact_sha256 && s.source_key == reference.source_key
    }) {
        return Err(invalid("Unknown evidence source association"));
    }
    Ok(())
}
fn association(digest: &str, key: &str) -> Result<(), Error> {
    if !valid_digest(digest) || key.is_empty() || key.len() > 512 {
        return Err(invalid("Invalid artifact/source identity"));
    }
    Ok(())
}
pub(crate) fn availability(state: &str, reason: Option<&str>) -> Result<(), Error> {
    match state {
        "present" if reason.is_none() => Ok(()),
        "missing" | "not-exported" | "unsupported" | "locked"
            if reason.is_some_and(|r| !r.is_empty() && r.len() <= 8192) =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "Unavailable content requires an explicit state/reason",
        )),
    }
}
fn object(
    objects: &BTreeMap<&String, &Object>,
    digest: &str,
    role: &str,
    bytes: Option<&str>,
) -> Result<(), Error> {
    if !objects
        .get(&digest.to_owned())
        .is_some_and(|o| o.role == role && bytes.is_none_or(|b| b == o.bytes))
    {
        return Err(invalid("Missing/mismatched object dependency"));
    }
    Ok(())
}
fn text(value: &str, limit: usize) -> Result<(), Error> {
    if value.len() > limit {
        return Err(invalid("Case text exceeds its limit"));
    }
    Ok(())
}
fn unique<'a>(values: impl Iterator<Item = &'a str>) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !valid_id(value) || !seen.insert(value) {
            return Err(invalid("Invalid/duplicate case identity"));
        }
    }
    Ok(())
}

fn source_inventory(
    value: &CaseManifest,
    objects: &BTreeMap<&String, &Object>,
) -> Result<(), Error> {
    let mut sources = BTreeSet::new();
    for source in &value.sources {
        association(&source.artifact_sha256, &source.source_key)?;
        if let Some(count) = &source.request_count {
            decimal(count, true)?;
        }
        if !sources.insert((&source.artifact_sha256, &source.source_key))
            || source.label.len() > 1024
        {
            return Err(invalid("Duplicate source association or excessive label"));
        }
        availability(
            &source.dependency.state,
            source.dependency.reason.as_deref(),
        )?;
        if source
            .dependency
            .sha256
            .as_ref()
            .is_some_and(|sha| sha != &source.artifact_sha256)
        {
            return Err(invalid(
                "Source dependency digest disagrees with its artifact",
            ));
        }
        if let Some(bytes) = &source.dependency.bytes {
            decimal(bytes, true)?;
        }
        if matches!(source.dependency.state.as_str(), "present" | "locked") {
            object(objects, &source.artifact_sha256, "capture", None)?;
        }
        if !matches!(
            source.evidence_completeness.as_str(),
            "complete"
                | "incomplete"
                | "truncated"
                | "redacted"
                | "evicted"
                | "not-retained"
                | "unknown"
        ) || !matches!(source.lineage.kind.as_str(), "original" | "derived")
        {
            return Err(invalid("Invalid source completeness/lineage"));
        }
        if source.lineage.kind == "derived" && source.lineage.operations.is_empty() {
            return Err(invalid(
                "Derived evidence requires its derivation operation",
            ));
        }
        for parent in &source.lineage.parents {
            if !valid_digest(&parent.artifact_sha256) {
                return Err(invalid("Invalid parent digest"));
            }
            availability(&parent.state, parent.reason.as_deref())?;
            if parent
                .source_key
                .as_ref()
                .is_some_and(|key| key.is_empty() || key.len() > 512)
            {
                return Err(invalid("Invalid parent source key"));
            }
        }
        lineage(&source.lineage)?;
    }
    Ok(())
}

fn lineage(value: &crate::Lineage) -> Result<(), Error> {
    if value.parents.len() > 32
        || value.operations.len() > 64
        || value.entry_mappings.len() > 100_000
    {
        return Err(invalid("Source lineage exceeds its bounds"));
    }
    for operation in &value.operations {
        text(operation, 128)?;
    }
    if let Some(reason) = &value.unavailable_parent_reason {
        text(reason, 8192)?;
    }
    let mut identities = BTreeSet::new();
    for entry in &value.entry_mappings {
        decimal(&entry.occurrence, true)?;
        decimal(&entry.original.occurrence, true)?;
        if entry.exchange_id.as_ref().is_some_and(|id| id.len() > 512)
            || entry
                .original
                .exchange_id
                .as_ref()
                .is_some_and(|id| id.len() > 512)
            || !identities.insert((&entry.exchange_id, &entry.occurrence))
            || !matches!(
                entry.original.boundary.as_str(),
                "client-request" | "upstream-request" | "upstream-response" | "client-response"
            )
            || !value.parents.iter().any(|p| {
                p.artifact_sha256 == entry.original.artifact_sha256
                    && p.source_key.as_ref() == Some(&entry.original.source_key)
            })
        {
            return Err(invalid("Invalid original-entry lineage mapping"));
        }
    }
    Ok(())
}
fn note_inventory(value: &CaseManifest) -> Result<(), Error> {
    for note in &value.notes {
        text(&note.text, 64 * 1024)?;
        if let Some(reference) = &note.source {
            evidence(value, reference)?;
        }
    }
    for marker in &value.markers {
        text(&marker.label, 1024)?;
        source(value, &marker.source)?;
        text(&marker.original_id, 512)?;
        if let Some(elapsed) = &marker.elapsed_millis {
            decimal(elapsed, true)?;
        }
    }
    Ok(())
}

fn context_inventory(
    value: &CaseManifest,
    objects: &BTreeMap<&String, &Object>,
) -> Result<(), Error> {
    for context in &value.contexts {
        text(&context.kind, 128)?;
        availability(&context.content.state, context.content.reason.as_deref())?;
        if context
            .content
            .sha256
            .as_ref()
            .is_some_and(|sha| !valid_digest(sha))
        {
            return Err(invalid("Invalid context object digest"));
        }
        if let Some(bytes) = &context.content.bytes {
            decimal(bytes, true)?;
        }
        if context.content.state == "present" {
            object(
                objects,
                context.content.sha256.as_deref().unwrap_or(""),
                "context",
                context.content.bytes.as_deref(),
            )?;
            if context.content.bytes.is_none() {
                return Err(invalid("Context object length is required"));
            }
        }
        if !matches!(
            context.completeness.as_str(),
            "complete" | "partial" | "unknown"
        ) || context.issues.len() > 128
        {
            return Err(invalid("Invalid context completeness/issues"));
        }
        for issue in &context.issues {
            text(issue, 8192)?;
        }
        if let Some(at) = &context.origin.collected_at {
            time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
                .map_err(|_| invalid("Invalid context collection time"))?;
        } else if context.origin.kind == "collected-for-case" {
            return Err(invalid(
                "New context collection requires its collection time",
            ));
        }
        match context.origin.kind.as_str() {
            "captured-source" => source(
                value,
                context
                    .origin
                    .source
                    .as_ref()
                    .ok_or_else(|| invalid("Captured context requires a source"))?,
            )?,
            "collected-for-case" if context.origin.source.is_none() => {}
            _ => return Err(invalid("Invalid context collection origin")),
        }
        text(&context.origin.platform, 256)?;
        text(&context.origin.purpose, 256)?;
        if let Some(label) = &context.origin.machine_label {
            text(label, 512)?;
        }
    }
    Ok(())
}

fn run_inventory(value: &CaseManifest, objects: &BTreeMap<&String, &Object>) -> Result<(), Error> {
    for run in &value.runs {
        decimal(&run.revision, false)?;
        if !value
            .experiments
            .iter()
            .filter_map(|v| serde_json::from_value::<Experiment>(v.clone()).ok())
            .any(|e| {
                e.id == run.experiment_id
                    && e.variants.iter().any(|v| {
                        v.id == run.variant_id
                            && v.revisions.iter().any(|r| r.revision == run.revision)
                    })
            })
        {
            return Err(invalid(
                "Run references an unselected frozen experiment revision",
            ));
        }
        if run.outcome.origin != "authored" {
            return Err(invalid("Run outcomes are authored observations"));
        }
        text(&run.outcome.text, 64 * 1024)?;
        for response in &run.responses {
            evidence(value, &response.source)?;
            if response.source.boundary != "client-response"
                || response.provenance != "retained-client-response"
            {
                return Err(invalid(
                    "Run responses require retained client-response evidence",
                ));
            }
            availability(&response.body.state, response.body.reason.as_deref())?;
            if response.body.state == "present" {
                object(
                    objects,
                    response.body.sha256.as_deref().unwrap_or(""),
                    "response-body",
                    response.body.bytes.as_deref(),
                )?;
                if response.body.bytes.is_none() || response.body.representation != "encoded-entity"
                {
                    return Err(invalid("Run body representation/length is invalid"));
                }
            }
        }
    }
    Ok(())
}
