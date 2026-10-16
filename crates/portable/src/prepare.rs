use crate::{Collection, Error, invalid};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

impl Collection {
    /// Parses strict bounded JSON and validates all declared rule/asset semantics.
    /// # Errors
    /// Rejects duplicate/unknown keys, unsupported versions and invalid native rules.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let value = crate::strict_json::read(bytes)?;
        if value.get("format").and_then(serde_json::Value::as_str) != Some("transmog-investigation")
            || value
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(0)
            || value.get("kind").and_then(serde_json::Value::as_str) != Some("rules")
        {
            return Err(Error::Unsupported(
                "Expected revision-0 rule collection".into(),
            ));
        }
        let result: Self = serde_json::from_value(value)?;
        crate::validate::collection(&result)?;
        Ok(result)
    }
    /// Serializes the independent public DTO after the same schema/semantic checks.
    /// # Errors
    /// Returns validation or manifest-size failures without writing a destination.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        crate::validate::collection(self)?;
        let bytes = encode_json(self)?;
        Self::parse(&bytes)?;
        Ok(bytes)
    }
}

pub(crate) fn encode_json(value: &impl serde::Serialize) -> Result<Vec<u8>, Error> {
    let mut output = BoundedManifest(Vec::new());
    serde_json::to_writer_pretty(&mut output, value)?;
    crate::strict_json::read(&output.0)?;
    Ok(output.0)
}

struct BoundedManifest(Vec<u8>);
impl Write for BoundedManifest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > (8 * 1024 * 1024) - self.0.len() {
            return Err(std::io::Error::other("Manifest exceeds eight MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An owned verified closure; input readers/paths cannot change it after review.
#[derive(Debug)]
pub struct PreparedCollection {
    manifest: Collection,
    staging: tempfile::TempDir,
}
impl PreparedCollection {
    /// Snapshots only declared objects using bounded streaming and exact hashes.
    ///
    /// `open` supplies local inert bytes for a digest, never a manifest path. The
    /// caller owns source access policy; this layer never resolves URLs or paths.
    /// # Errors
    /// Removes only owned staging on cancellation, corruption, limits or I/O failure.
    pub fn prepare(
        manifest: &[u8],
        staging_parent: &Path,
        mut open: impl FnMut(&str) -> Result<Box<dyn Read + Send>, Error>,
        canceled: &AtomicBool,
    ) -> Result<Self, Error> {
        if canceled.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        let manifest = Collection::parse(manifest)?;
        let staging = tempfile::Builder::new()
            .prefix(".rules-import-")
            .tempdir_in(staging_parent)?;
        for object in &manifest.objects {
            if canceled.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let input = open(&object.sha256)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.path().join(&object.sha256))?;
            copy_object(input, &mut output, object, canceled)?;
            output.sync_all()?;
        }
        if canceled.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        Ok(Self { manifest, staging })
    }
    /// The validated immutable manifest used for review and publication.
    pub fn manifest(&self) -> &Collection {
        &self.manifest
    }
    /// Opens one pinned, verified response object; no foreign path is interpreted.
    /// # Errors
    /// Rejects undeclared digests or unavailable owned staging.
    pub fn open_object(&self, digest: &str) -> Result<File, Error> {
        if !self.manifest.objects.iter().any(|o| o.sha256 == digest) {
            return Err(invalid("Undeclared response object"));
        }
        Ok(File::open(self.staging.path().join(digest))?)
    }
}
pub(crate) fn copy_object(
    mut input: Box<dyn Read + Send>,
    output: &mut impl Write,
    object: &crate::Object,
    canceled: &AtomicBool,
) -> Result<(), Error> {
    let expected = crate::decimal(&object.bytes, true)?;
    let mut total = 0_u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 128 * 1024];
    loop {
        if canceled.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| invalid("Object length overflow"))?;
        if total > expected {
            return Err(invalid(
                "Response object grew or exceeds its declared length",
            ));
        }
        hash.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    if total != expected || format!("{:x}", hash.finalize()) != object.sha256 {
        return Err(invalid("Response object length or SHA-256 mismatch"));
    }
    Ok(())
}
