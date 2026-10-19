use crate::{
    Error, MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES, Manifest, invalid, sha256,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, atomic::AtomicBool},
};

/// Bounded indexed tmext package, retaining its exact reader rather than reopening a path.
pub struct Package<R> {
    reader: R,
    manifest: Manifest,
    members: BTreeMap<String, transmog_package_zip::Member>,
}
impl<R: Read + Seek> Package<R> {
    /// Reads only structural metadata and the manifest; payloads remain lazy.
    /// # Errors
    /// Rejects unsafe archives, missing inventory, unsupported versions and oversized packages.
    pub fn open(mut reader: R) -> Result<Self, Error> {
        if reader.seek(SeekFrom::End(0))? > MAX_PACKAGE_BYTES + 4 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let members = transmog_package_zip::read(&mut reader, &AtomicBool::new(false))?;
        if members.len() > MAX_PACKAGE_FILES + 1 {
            return Err(Error::Limit);
        }
        let metadata = members
            .get("manifest.json")
            .ok_or_else(|| invalid("The extension package has no manifest"))?;
        if metadata.length > MAX_MANIFEST_BYTES as u64
            || members.values().any(|m| m.offset < metadata.offset)
        {
            return Err(invalid("The bounded extension manifest must be first"));
        }
        let bytes = read_member(&mut reader, metadata)?;
        let manifest = Manifest::parse(&bytes, false)?;
        if members.len() != manifest.files.len() + 1 {
            return Err(invalid("The extension payload inventory is incomplete"));
        }
        for file in &manifest.files {
            let member = members
                .get(&file.path)
                .ok_or_else(|| invalid("A declared extension payload is missing"))?;
            if member.length != file.bytes {
                return Err(invalid(
                    "Extension payload length disagrees with its manifest",
                ));
            }
        }
        Ok(Self {
            reader,
            manifest,
            members,
        })
    }

    /// Validated inert metadata for discovery and Extensions settings.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Reads and verifies exactly one declared payload, without extracting files.
    /// # Errors
    /// Denies undeclared paths and rejects changed bytes, CRCs and digests.
    pub fn payload(&mut self, path: &str) -> Result<Vec<u8>, Error> {
        let file = self
            .manifest
            .files
            .iter()
            .find(|f| f.path == path)
            .ok_or(Error::AccessDenied)?;
        let member = self.members.get(path).ok_or(Error::AccessDenied)?;
        let bytes = read_member(&mut self.reader, member)?;
        if Some(sha256(&bytes)).as_ref() != file.sha256.as_ref() {
            return Err(invalid(
                "Extension payload hash does not match its manifest",
            ));
        }
        Ok(bytes)
    }

    /// Materializes one verified immutable execution revision, only after activation.
    /// # Errors
    /// Rejects any payload integrity failure before publishing a snapshot.
    pub fn snapshot(&mut self) -> Result<Snapshot, Error> {
        let mut files = BTreeMap::new();
        for path in self
            .manifest
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect::<Vec<_>>()
        {
            files.insert(path.clone(), Arc::<[u8]>::from(self.payload(&path)?));
        }
        Snapshot::new(self.manifest.clone(), files)
    }
}

fn read_member(
    reader: &mut (impl Read + Seek),
    member: &transmog_package_zip::Member,
) -> Result<Vec<u8>, Error> {
    if member.length > 16 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    reader.seek(SeekFrom::Start(member.offset))?;
    let mut bytes = vec![0; usize::try_from(member.length).map_err(|_| Error::Limit)?];
    reader.read_exact(&mut bytes)?;
    if crc32fast::hash(&bytes) != member.crc {
        return Err(invalid("Extension member CRC does not match"));
    }
    Ok(bytes)
}

/// Exact verified code/data revision; contains no traffic and grants no filesystem access.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Normalized manifest with exact payload digests, including development snapshots.
    pub manifest: Manifest,
    /// Exact immutable payload bytes supplied over worker IPC.
    pub files: BTreeMap<String, Arc<[u8]>>,
    /// Content identity used to bind catalog, worker and hot-reload generation.
    pub digest: String,
}
impl Snapshot {
    /// Validates already-collected immutable sources and computes missing development digests.
    /// Directory registration must securely collect sources before calling this method.
    /// # Errors
    /// Rejects inventory drift, size/hash mismatches and malformed metadata.
    pub fn new(mut manifest: Manifest, files: BTreeMap<String, Arc<[u8]>>) -> Result<Self, Error> {
        manifest.validate(true)?;
        if files.len() != manifest.files.len() {
            return Err(invalid("Snapshot has undeclared or missing files"));
        }
        for payload in &mut manifest.files {
            let bytes = files.get(&payload.path).ok_or(Error::AccessDenied)?;
            let digest = sha256(bytes);
            if bytes.len() as u64 != payload.bytes
                || payload.sha256.as_ref().is_some_and(|old| old != &digest)
            {
                return Err(invalid("Snapshot bytes differ from the declared revision"));
            }
            payload.sha256 = Some(digest);
        }
        manifest.validate(false)?;
        let digest = sha256(&serde_json::to_vec(&manifest)?);
        Ok(Self {
            manifest,
            files,
            digest,
        })
    }
}

/// Writes the ordinary immutable execution snapshot as a Stored ZIP64 `.tmext`.
/// # Errors
/// Rejects snapshot drift, output failures or cancellation.
pub fn write_package(
    output: impl Write,
    snapshot: &Snapshot,
    canceled: &AtomicBool,
) -> Result<(), Error> {
    // Public value objects cannot bypass validation by mutating fields after construction.
    let checked = Snapshot::new(snapshot.manifest.clone(), snapshot.files.clone())?;
    if checked.digest != snapshot.digest {
        return Err(invalid("Snapshot identity changed"));
    }
    let metadata = serde_json::to_vec(&checked.manifest)?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(Error::Limit);
    }
    let mut writer = transmog_package_zip::Writer::new(output);
    writer.member(
        "manifest.json",
        &mut metadata.as_slice(),
        metadata.len() as u64,
        canceled,
    )?;
    for file in &checked.manifest.files {
        let bytes = &checked.files[&file.path];
        writer.member(&file.path, &mut bytes.as_ref(), file.bytes, canceled)?;
    }
    writer.finish()?;
    Ok(())
}

/// Persisted metadata-only catalog record. No worker/runtime state is serialized here.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    /// Validated declaration summary.
    pub manifest: Manifest,
    /// Revision identity supplied by installation or app bundling.
    pub digest: String,
    /// Whether available for explicit selection, never automatic execution.
    pub enabled: bool,
    /// Package provenance used by management UI.
    pub origin: Origin,
}

/// Ownership of extension code files, distinct from its read-only input permissions.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// App-owned package; disable rather than remove its installed app files.
    Bundled,
    /// Host-managed installed user package.
    Installed,
    /// Explicitly registered source folder; uninstall never deletes source.
    Development,
}
