//! Pinned non-reparse local directory traversal for explicitly registered developer sources.
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path},
    sync::Arc,
};
use transmog_extensions::{Error, MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, Manifest, Snapshot};

#[cfg(windows)]
use std::{fs::OpenOptions, path::PathBuf};

pub(super) struct Directory {
    #[cfg(windows)]
    root: PathBuf,
    #[cfg(windows)]
    _ancestors: Vec<File>,
    #[cfg(unix)]
    root: File,
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, Error> {
        let path = std::path::absolute(path)?;
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(invalid(
                "A developer directory must be an absolute local path",
            ));
        }
        #[cfg(windows)]
        {
            use std::path::Prefix;
            if !matches!(path.components().next(),Some(Component::Prefix(p))if matches!(p.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_)))
            {
                return Err(invalid("Developer extensions require a local directory"));
            }
            // Holding directory handles without FILE_SHARE_DELETE prevents ancestor
            // replacement while descendants are checked and opened by path.
            let mut ancestors = Vec::new();
            for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
                ancestors.push(open_windows(ancestor, true)?);
            }
            Ok(Self {
                root: path,
                _ancestors: ancestors,
            })
        }
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags, openat};
            let mut directory = File::open("/")?;
            for component in path.components() {
                if let Component::Normal(name) = component {
                    let fd = openat(
                        &directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?;
                    directory = File::from(fd);
                }
            }
            Ok(Self { root: directory })
        }
    }
    fn file(&self, name: &str) -> Result<File, Error> {
        transmog_package_name(name)?;
        #[cfg(windows)]
        {
            let target = self.root.join(name);
            let mut held = Vec::new();
            let mut parent = self.root.clone();
            let parts: Vec<_> = Path::new(name).components().collect();
            for part in &parts[..parts.len() - 1] {
                parent.push(part.as_os_str());
                held.push(open_windows(&parent, true)?);
            }
            // Keep each parent pinned until after the final non-reparse file opens.
            let file = open_windows(&target, false)?;
            drop(held);
            Ok(file)
        }
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags, openat};
            let mut directory = self.root.try_clone()?;
            let parts: Vec<_> = Path::new(name).components().collect();
            for (index, part) in parts.iter().enumerate() {
                let flags = OFlags::RDONLY
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK
                    | if index + 1 == parts.len() {
                        OFlags::empty()
                    } else {
                        OFlags::DIRECTORY
                    };
                directory = File::from(
                    openat(&directory, part.as_os_str(), flags, Mode::empty())
                        .map_err(std::io::Error::from)?,
                );
            }
            if !directory.metadata()?.is_file() {
                return Err(invalid("A source must be a regular file"));
            }
            Ok(directory)
        }
    }
    pub(super) fn bytes(&self, name: &str, max: usize) -> Result<Vec<u8>, Error> {
        let file = self.file(name)?;
        if file.metadata()?.len() > max as u64 {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        file.take(max as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
    pub(super) fn manifest(&self) -> Result<Manifest, Error> {
        Manifest::parse(&self.bytes("manifest.json", MAX_MANIFEST_BYTES)?, true)
    }
    pub(super) fn snapshot(&self) -> Result<Snapshot, Error> {
        let mut manifest = self.manifest()?;
        let mut files = BTreeMap::new();
        let mut total = 0_u64;
        for file in &mut manifest.files {
            let bytes = self.bytes(&file.path, 16 * 1024 * 1024)?;
            total = total.checked_add(bytes.len() as u64).ok_or(Error::Limit)?;
            if total > MAX_PACKAGE_BYTES {
                return Err(Error::Limit);
            }
            // Registered developer files are mutable. Snapshot exact bytes, lengths
            // and hashes together rather than require hash edits after each save.
            file.bytes = bytes.len() as u64;
            file.sha256 = None;
            files.insert(file.path.clone(), Arc::<[u8]>::from(bytes));
        }
        Snapshot::new(manifest, files)
    }
}
fn transmog_package_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 240
        || !name.is_ascii()
        || name.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(invalid("Invalid developer source path"));
    }
    Ok(())
}
#[cfg(windows)]
fn open_windows(path: &Path, directory: bool) -> Result<File, Error> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(if directory { 3 } else { 1 })
        .custom_flags(0x0020_0000 | if directory { 0x0200_0000 } else { 0 });
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & 0x400 != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(invalid(
            "Developer sources cannot contain links or reparse points",
        ));
    }
    Ok(file)
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
