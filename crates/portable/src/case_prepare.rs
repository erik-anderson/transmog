use crate::{
    CaseInventory, CaseManifest, Collection, Error, Experiment, ExperimentAvailability, invalid,
};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

impl CaseManifest {
    /// Parses a bounded revision-0 case without resolving paths or executing content.
    /// # Errors
    /// Rejects malformed structure, references and unsupported mandatory requirements.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let value = crate::strict_json::read(bytes)?;
        if value.get("format").and_then(serde_json::Value::as_str) != Some("transmog-investigation")
            || value
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(0)
            || value.get("kind").and_then(serde_json::Value::as_str) != Some("case")
        {
            return Err(Error::Unsupported(
                "Expected revision-0 case manifest".into(),
            ));
        }
        let result: Self = serde_json::from_value(value)?;
        crate::case_validate::check(&result)?;
        Ok(result)
    }
    /// Serializes validated case metadata within the eight-MiB/depth-32 bounds.
    /// # Errors
    /// Returns structural, semantic, capability or metadata-limit failures.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        crate::case_validate::check(self)?;
        crate::prepare::encode_json(self)
    }
    /// Inventories availability without claiming browser execution or complete evidence.
    /// # Errors
    /// Rejects invalid metadata before reporting inventory.
    pub fn inventory(&self) -> Result<CaseInventory, Error> {
        crate::case_validate::check(self)?;
        let mut experiments = Vec::new();
        for value in &self.experiments {
            let id = value["id"].as_str().unwrap_or("").to_owned();
            let result = self.select_rules(std::slice::from_ref(&id), false);
            experiments.push(ExperimentAvailability {
                id,
                state: if result.is_ok() {
                    "ready-for-explicit-import"
                } else {
                    "unavailable"
                }
                .into(),
                reason: result.err().map(|e| e.to_string()),
            });
        }
        let object_bytes = self.objects.iter().try_fold(0_u64, |sum, o| {
            sum.checked_add(crate::decimal(&o.bytes, true)?)
                .ok_or_else(|| invalid("Case length overflow"))
        })?;
        Ok(CaseInventory {
            sources: self.sources.len(),
            notes: self.notes.len(),
            contexts: self.contexts.len(),
            runs: self.runs.len(),
            object_bytes: object_bytes.to_string(),
            experiments,
            missing_dependencies: self
                .assets
                .iter()
                .filter(|a| a.body.state != "present")
                .count()
                + self
                    .sources
                    .iter()
                    .filter(|s| s.dependency.state != "present")
                    .count()
                + self
                    .contexts
                    .iter()
                    .filter(|c| c.content.state != "present")
                    .count(),
            incomplete_sources: self
                .sources
                .iter()
                .filter(|s| s.evidence_completeness != "complete")
                .count(),
        })
    }
    /// Selects complete supported rule content for a separate reviewed disabled import.
    /// # Errors
    /// Rejects unknown selections, opaque semantics or unavailable asset dependencies.
    pub fn rules_for_import(&self, ids: &[String], ordinary: bool) -> Result<Collection, Error> {
        crate::case_validate::check(self)?;
        self.select_rules(ids, ordinary)
    }
    fn select_rules(&self, ids: &[String], ordinary: bool) -> Result<Collection, Error> {
        let mut result = crate::case_validate::validation_collection(self)?;
        result.rules = if ordinary { self.rules.clone() } else { vec![] };
        let mut selected = BTreeSet::new();
        for id in ids {
            if !selected.insert(id) {
                return Err(invalid("Duplicate selected response set"));
            }
            let value = self
                .experiments
                .iter()
                .find(|e| e["id"].as_str() == Some(id))
                .ok_or_else(|| invalid("Selected response set is not included"))?;
            if !crate::case_validate::supported(value) {
                return Err(Error::Unsupported("Opaque response set semantics".into()));
            }
            result
                .experiments
                .push(serde_json::from_value::<Experiment>(value.clone())?);
        }
        let references = result
            .rules
            .iter()
            .chain(result.experiments.iter().flat_map(|e| {
                e.variants
                    .iter()
                    .flat_map(|v| v.revisions.iter().flat_map(|r| &r.rules))
            }))
            .filter(|r| r.kind == "auto-response")
            .map(|r| {
                format!(
                    "{}@{}",
                    r.action["assetId"].as_str().unwrap_or(""),
                    r.action["revision"].as_str().unwrap_or("")
                )
            })
            .collect::<BTreeSet<_>>();
        result.assets = self
            .assets
            .iter()
            .filter(|a| references.contains(&format!("{}@{}", a.id, a.revision)))
            .cloned()
            .collect();
        if result.assets.iter().any(|a| a.body.state != "present") {
            return Err(invalid("Selected response set has an unavailable asset"));
        }
        let digests = result
            .assets
            .iter()
            .filter_map(|a| a.body.sha256.as_ref())
            .collect::<BTreeSet<_>>();
        result.objects = self
            .objects
            .iter()
            .filter(|o| digests.contains(&o.sha256))
            .cloned()
            .collect();
        result.encode()?;
        Ok(result)
    }
}

/// Owns verified object snapshots independently of the original directory or archive.
#[derive(Debug)]
pub struct PreparedCase {
    manifest: CaseManifest,
    staging: tempfile::TempDir,
}
impl PreparedCase {
    /// Freezes all declared objects with bounded reads and exact integrity checks.
    /// # Errors
    /// Invalid metadata, missing/corrupt objects, cancellation or I/O remove owned staging.
    pub fn prepare(
        bytes: &[u8],
        parent: &Path,
        mut open: impl FnMut(&str) -> Result<Box<dyn Read + Send>, Error>,
        canceled: &AtomicBool,
    ) -> Result<Self, Error> {
        check_cancel(canceled)?;
        let manifest = CaseManifest::parse(bytes)?;
        let staging = tempfile::Builder::new()
            .prefix(".case-objects-")
            .tempdir_in(parent)?;
        for object in &manifest.objects {
            check_cancel(canceled)?;
            let mut output = File::create(staging.path().join(&object.sha256))?;
            crate::prepare::copy_object(open(&object.sha256)?, &mut output, object, canceled)?;
            output.sync_all()?;
            if object.role == "context" {
                let content = std::fs::read(staging.path().join(&object.sha256))?;
                if object.media_type == "application/json" {
                    crate::strict_json::read(&content)?;
                } else if object.media_type.starts_with("text/") {
                    std::str::from_utf8(&content)
                        .map_err(|_| invalid("Context text is not UTF-8"))?;
                }
            }
        }
        check_cancel(canceled)?;
        Ok(Self { manifest, staging })
    }
    /// Validated source definitions and authored intent; inspection never activates them.
    pub fn manifest(&self) -> &CaseManifest {
        &self.manifest
    }
    /// Opens only a declared owned object, without interpreting a foreign path.
    /// # Errors
    /// Rejects undeclared objects or unavailable owned storage.
    pub fn open_object(&self, digest: &str) -> Result<File, Error> {
        if !self.manifest.objects.iter().any(|o| o.sha256 == digest) {
            return Err(invalid("Undeclared case object"));
        }
        Ok(File::open(self.staging.path().join(digest))?)
    }
}
pub(crate) fn check_cancel(canceled: &AtomicBool) -> Result<(), Error> {
    if canceled.load(Ordering::Acquire) {
        Err(Error::Canceled)
    } else {
        Ok(())
    }
}
