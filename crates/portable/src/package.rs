//! Pinned read-only packages and transactional native/bare ZIP publication.
use crate::{
    CaseManifest, Collection, Error, Object, PreparedCase, PreparedCollection, WorkingCase,
    invalid, package_zip as zip,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_capture::{
    CaptureEncoding, CapturePassword,
    artifact::{ArtifactReader, ArtifactWriter},
};

/// Inert seekable local bytes; no dependency path or network resolution.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}
/// One validated current portable manifest.
#[derive(Clone, Debug)]
pub enum PackageManifest {
    /// Investigation evidence, notes, context and selected experiments.
    Case(CaseManifest),
    /// Independent rule/asset collection.
    Rules(Collection),
}
impl PackageManifest {
    /// Exact referenced immutable closure.
    pub fn objects(&self) -> &[Object] {
        match self {
            Self::Case(m) => &m.objects,
            Self::Rules(m) => &m.objects,
        }
    }
    /// Encodes validated metadata within existing strict bounds.
    /// # Errors
    /// Rejects unsupported or invalid semantics.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::Case(m) => m.encode(),
            Self::Rules(m) => m.encode(),
        }
    }
    /// Descriptive title, never a filesystem locator or trust decision.
    pub fn title(&self) -> &str {
        match self {
            Self::Case(m) => &m.title,
            Self::Rules(m) => &m.title,
        }
    }
    fn media(&self) -> &'static str {
        match self {
            Self::Case(_) => "application/vnd.transmog.case+zip",
            Self::Rules(_) => "application/vnd.transmog.rules+zip",
        }
    }
}
/// Explicit native compressed transfer or uncompressed bare ZIP interchange.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageFormat {
    /// `TMCap` compression and optional whole-case encryption.
    Native,
    /// Stored ZIP64, without encryption.
    BareZip,
}

/// Read-only validated package. Cloned member cursors keep the source pinned.
#[derive(Clone)]
pub struct Package {
    input: Arc<Mutex<Box<dyn ReadSeek>>>,
    members: BTreeMap<String, zip::Member>,
    manifest: PackageManifest,
    encrypted: bool,
}
impl std::fmt::Debug for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}
impl Package {
    /// Opens/fully verifies all declared objects without persisting decrypted bytes.
    /// # Errors
    /// Rejects unsafe ZIPs, incomplete outer/inner transfers, wrong passwords and cancellation.
    pub fn open(
        path: &Path,
        password: Option<&CapturePassword>,
        canceled: &AtomicBool,
    ) -> Result<Self, Error> {
        check(canceled)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(5);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("Package must be a regular file"));
        }
        let mut input = CheckedFile {
            file,
            length: metadata.len(),
            modified: metadata.modified()?,
        };
        let mut magic = [0; 9];
        input.read_exact(&mut magic)?;
        input.rewind()?;
        let mut protected_media = None;
        let mut encrypted = false;
        let mut input: Box<dyn ReadSeek> = if &magic == b"TMCAP001\0" {
            let mut wrapped = ArtifactReader::new(input, password).map_err(capture_error)?;
            wrapped
                .verify(|| canceled.load(Ordering::Acquire))
                .map_err(|e| {
                    if canceled.load(Ordering::Acquire) {
                        Error::Canceled
                    } else {
                        capture_error(e)
                    }
                })?;
            protected_media = Some(wrapped.descriptor().media_type.clone());
            encrypted = wrapped.encrypted();
            Box::new(wrapped)
        } else {
            Box::new(input)
        };
        let members = zip::read(&mut *input, canceled)?;
        let member = members
            .get("manifest.json")
            .ok_or_else(|| invalid("Missing package manifest"))?;
        if member.length > 8 * 1024 * 1024 {
            return Err(invalid("Manifest exceeds eight MiB"));
        }
        input.seek(SeekFrom::Start(member.offset))?;
        let mut bytes = vec![
            0;
            usize::try_from(member.length)
                .map_err(|_| invalid("Manifest length overflow"))?
        ];
        input.read_exact(&mut bytes)?;
        if crc32fast::hash(&bytes) != member.crc {
            return Err(invalid("Manifest CRC mismatch"));
        }
        let value = crate::strict_json::read(&bytes)?;
        let manifest = match value["kind"].as_str() {
            Some("case") => PackageManifest::Case(CaseManifest::parse(&bytes)?),
            Some("rules") => PackageManifest::Rules(Collection::parse(&bytes)?),
            _ => return Err(Error::Unsupported("Unknown package manifest kind".into())),
        };
        if protected_media
            .as_deref()
            .is_some_and(|m| m != manifest.media())
        {
            return Err(invalid("Outer artifact type disagrees with manifest"));
        }
        if members.len() != manifest.objects().len() + 1 {
            return Err(invalid("Unlisted or missing package objects"));
        }
        let result = Self {
            input: Arc::new(Mutex::new(input)),
            members,
            manifest,
            encrypted,
        };
        result.verify_objects(canceled)?;
        Ok(result)
    }
    fn verify_objects(&self, canceled: &AtomicBool) -> Result<(), Error> {
        let mut buffer = vec![0; 128 * 1024];
        for object in self.manifest.objects() {
            check(canceled)?;
            let mut reader = self.open_object(&object.sha256)?;
            let expected = crate::decimal(&object.bytes, true)?;
            let member = &self.members[&format!("objects/sha256/{}", object.sha256)];
            if member.length != expected {
                return Err(invalid("Package object size mismatch"));
            }
            let mut hash = Sha256::new();
            let mut crc = crc32fast::Hasher::new();
            let mut context = Vec::new();
            loop {
                check(canceled)?;
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
                crc.update(&buffer[..count]);
                if object.role == "context" {
                    if context.len() + count > 8 * 1024 * 1024 {
                        return Err(invalid("Context exceeds eight MiB"));
                    }
                    context.extend_from_slice(&buffer[..count]);
                }
            }
            if format!("{:x}", hash.finalize()) != object.sha256 || crc.finalize() != member.crc {
                return Err(invalid("Package object SHA-256/CRC mismatch"));
            }
            if object.role == "context" {
                if object.media_type == "application/json" {
                    crate::strict_json::read(&context)?;
                } else if object.media_type.starts_with("text/") {
                    std::str::from_utf8(&context)
                        .map_err(|_| invalid("Context text is not UTF-8"))?;
                }
            }
        }
        Ok(())
    }
    /// Strict validated inventory metadata. Content remains inert.
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }
    /// Whether the whole transfer is protected, separately from locked embedded sources.
    pub fn encrypted(&self) -> bool {
        self.encrypted
    }
    /// Copies the exact inner ZIP, with optional new outer compression/encryption.
    /// No decrypted archive is staged unless bare/readable output is explicitly selected.
    /// # Errors
    /// Verifies before atomic publication; preserves source and existing destinations on failure.
    pub fn write_transfer(
        &self,
        path: &Path,
        format: PackageFormat,
        password: Option<&CapturePassword>,
        replace: bool,
        canceled: &AtomicBool,
    ) -> Result<(), Error> {
        check(canceled)?;
        if matches!(format, PackageFormat::BareZip) && password.is_some() {
            return Err(invalid(
                "Bare ZIP cannot be encrypted; choose native transfer",
            ));
        }
        let length = self
            .input
            .lock()
            .map_err(|_| invalid("Package reader failed"))?
            .seek(SeekFrom::End(0))?;
        let mut input = PackageMember {
            input: self.input.clone(),
            member: zip::Member {
                offset: 0,
                length,
                crc: 0,
            },
            position: 0,
        };
        let absolute = std::path::absolute(path)?;
        let parent = absolute
            .parent()
            .ok_or_else(|| invalid("Package destination has no parent"))?;
        let mut staged = tempfile::Builder::new()
            .prefix(".case-transfer-")
            .tempfile_in(parent)?;
        let mut boundaries = self
            .members
            .values()
            .map(|m| m.offset + m.length)
            .collect::<Vec<_>>();
        boundaries.sort_unstable();
        let mut buffer = vec![0; 128 * 1024];
        let mut copy = |output: &mut dyn io::Write,
                        position: &mut u64,
                        next: u64,
                        input: &mut PackageMember|
         -> Result<(), Error> {
            while *position < next {
                check(canceled)?;
                let count = usize::try_from((next - *position).min(buffer.len() as u64))
                    .map_err(|_| invalid("Package length overflow"))?;
                input.read_exact(&mut buffer[..count])?;
                output.write_all(&buffer[..count])?;
                *position += count as u64;
            }
            Ok(())
        };
        match format {
            PackageFormat::BareZip => {
                copy(staged.as_file_mut(), &mut 0, length, &mut input)?;
            }
            PackageFormat::Native => {
                let mut writer = ArtifactWriter::new(
                    staged.as_file_mut(),
                    self.manifest.media(),
                    &CaptureEncoding {
                        password: password.cloned(),
                    },
                    true,
                )
                .map_err(capture_error)?;
                let mut position = 0;
                for end in boundaries {
                    copy(&mut writer, &mut position, end, &mut input)?;
                    writer.member_boundary().map_err(capture_error)?;
                }
                copy(&mut writer, &mut position, length, &mut input)?;
                writer.finish().map_err(capture_error)?;
            }
        }
        staged.as_file().sync_all()?;
        let staged = staged.into_temp_path();
        drop(Self::open(&staged, password, canceled)?);
        check(canceled)?;
        publish(staged, &absolute, replace)?;
        Ok(())
    }
    /// Independent bounded cursor over one declared object, retaining the source handle.
    /// # Errors
    /// Refuses undeclared identities.
    pub fn open_object(&self, digest: &str) -> Result<PackageMember, Error> {
        if !self.manifest.objects().iter().any(|o| o.sha256 == digest) {
            return Err(invalid("Object is not declared"));
        }
        let member = self
            .members
            .get(&format!("objects/sha256/{digest}"))
            .ok_or_else(|| invalid("Missing package object"))?;
        Ok(PackageMember {
            input: self.input.clone(),
            member: member.clone(),
            position: 0,
        })
    }
    /// Explicit plaintext working copy; existing destinations are preserved.
    /// # Errors
    /// Verifies owned snapshots before publishing a new directory.
    pub fn save_working_copy(
        &self,
        destination: &Path,
        staging: &Path,
        canceled: &AtomicBool,
    ) -> Result<WorkingCase, Error> {
        let PackageManifest::Case(manifest) = &self.manifest else {
            return Err(invalid("A rule collection has no working-case directory"));
        };
        let prepared = PreparedCase::prepare(
            &manifest.encode()?,
            staging,
            |sha| Ok(Box::new(self.open_object(sha)?)),
            canceled,
        )?;
        WorkingCase::create(prepared, destination, canceled)
    }
}
/// Cloneable object cursor; no plaintext files are created by reading it.
#[derive(Clone)]
pub struct PackageMember {
    input: Arc<Mutex<Box<dyn ReadSeek>>>,
    member: zip::Member,
    position: u64,
}
impl std::fmt::Debug for PackageMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageMember")
            .field("length", &self.member.length)
            .finish_non_exhaustive()
    }
}
impl Read for PackageMember {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut input = self
            .input
            .lock()
            .map_err(|_| io::Error::other("Package reader failed"))?;
        let count = buffer.len().min(
            usize::try_from(self.member.length.saturating_sub(self.position)).unwrap_or(usize::MAX),
        );
        if count == 0 {
            return Ok(0);
        }
        input.seek(SeekFrom::Start(self.member.offset + self.position))?;
        let count = input.read(&mut buffer[..count])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Package object became unavailable",
            ));
        }
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for PackageMember {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let at = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.member.length) + i128::from(n),
        };
        self.position = u64::try_from(at)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid package seek"))?;
        Ok(self.position)
    }
}
struct CheckedFile {
    file: File,
    length: u64,
    modified: std::time::SystemTime,
}
impl Read for CheckedFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let m = self.file.metadata()?;
        if m.len() != self.length || m.modified()? != self.modified {
            return Err(io::Error::other("Package changed since opening"));
        }
        self.file.read(buffer)
    }
}
impl Seek for CheckedFile {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.file.seek(from)
    }
}
fn check(canceled: &AtomicBool) -> Result<(), Error> {
    if canceled.load(Ordering::Acquire) {
        Err(Error::Canceled)
    } else {
        Ok(())
    }
}
fn capture_error(error: transmog_capture::CaptureError) -> Error {
    match error {
        transmog_capture::CaptureError::PasswordRequired => Error::PasswordRequired,
        transmog_capture::CaptureError::InvalidPassword => Error::InvalidPassword,
        transmog_capture::CaptureError::UnsupportedEncoding => {
            Error::Unsupported("Unsupported TMCap document/encoding".into())
        }
        error => Error::Invalid(error.to_string()),
    }
}

/// Writes one selected case revision; history/recovery state is excluded.
/// # Errors
/// Stages, flushes and verifies the entire transfer before create-new/confirmed replacement.
pub fn write_case_package(
    case: &PreparedCase,
    path: &Path,
    format: PackageFormat,
    password: Option<CapturePassword>,
    replace: bool,
    canceled: &AtomicBool,
) -> Result<(), Error> {
    write_package(
        &PackageManifest::Case(case.manifest().clone()),
        |sha| case.open_object(sha),
        path,
        format,
        password,
        replace,
        canceled,
    )
}
/// Writes a frozen rule collection with the same protected archive contract.
/// # Errors
/// Does not publish incomplete, canceled, corrupt or plaintext fallback output.
pub fn write_rules_package(
    rules: &PreparedCollection,
    path: &Path,
    format: PackageFormat,
    password: Option<CapturePassword>,
    replace: bool,
    canceled: &AtomicBool,
) -> Result<(), Error> {
    write_package(
        &PackageManifest::Rules(rules.manifest().clone()),
        |sha| rules.open_object(sha),
        path,
        format,
        password,
        replace,
        canceled,
    )
}
fn write_zip<W: io::Write>(
    output: W,
    manifest: &PackageManifest,
    mut open: impl FnMut(&str) -> Result<File, Error>,
    canceled: &AtomicBool,
    mut boundary: impl FnMut(&mut W) -> Result<(), Error>,
) -> Result<W, Error> {
    let bytes = manifest.encode()?;
    let mut writer = zip::Writer::new(output);
    writer.member(
        "manifest.json",
        &mut bytes.as_slice(),
        bytes.len() as u64,
        canceled,
    )?;
    writer.boundary(&mut boundary)?;
    for object in manifest.objects() {
        check(canceled)?;
        writer.member(
            &format!("objects/sha256/{}", object.sha256),
            &mut open(&object.sha256)?,
            crate::decimal(&object.bytes, true)?,
            canceled,
        )?;
        writer.boundary(&mut boundary)?;
    }
    writer.finish()
}
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)] // Own the transient password through verification/publication.
fn write_package(
    manifest: &PackageManifest,
    open: impl FnMut(&str) -> Result<File, Error>,
    path: &Path,
    format: PackageFormat,
    password: Option<CapturePassword>,
    replace: bool,
    canceled: &AtomicBool,
) -> Result<(), Error> {
    check(canceled)?;
    if matches!(format, PackageFormat::BareZip) && password.is_some() {
        return Err(invalid(
            "Bare ZIP cannot be encrypted; choose native transfer",
        ));
    }
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| invalid("Package destination has no parent"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".case-transfer-")
        .tempfile_in(parent)?;
    match format {
        PackageFormat::BareZip => {
            write_zip(staged.as_file_mut(), manifest, open, canceled, |_| Ok(()))?;
        }
        PackageFormat::Native => {
            let writer = ArtifactWriter::new(
                staged.as_file_mut(),
                manifest.media(),
                &CaptureEncoding {
                    password: password.clone(),
                },
                true,
            )
            .map_err(capture_error)?;
            let writer = write_zip(writer, manifest, open, canceled, |w| {
                w.member_boundary().map_err(capture_error)
            })?;
            writer.finish().map_err(capture_error)?;
        }
    }
    staged.as_file().sync_all()?;
    let staged = staged.into_temp_path();
    drop(Package::open(&staged, password.as_ref(), canceled)?);
    check(canceled)?;
    publish(staged, &absolute, replace)?;
    Ok(())
}

fn publish(staged: tempfile::TempPath, destination: &Path, replace: bool) -> Result<(), Error> {
    if replace {
        // keep() clears Windows' temporary attribute. Immediately restore owned
        // cleanup, then use std's FileRenameInfoEx fallback: pinned share-delete
        // readers retain the original snapshot while new opens see the replacement.
        let mut staged =
            tempfile::TempPath::try_from_path(staged.keep().map_err(|e| Error::Io(e.error))?)?;
        std::fs::rename(&staged, destination)?;
        staged.disable_cleanup(true);
    } else {
        staged
            .persist_noclobber(destination)
            .map_err(|e| Error::Io(e.error))?;
    }
    Ok(())
}
