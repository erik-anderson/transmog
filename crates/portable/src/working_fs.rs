use crate::{Error, invalid};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
};

pub(super) fn plain(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt as _;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if metadata.file_type().is_symlink()
        || reparse
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(invalid(
            "Case paths must be regular files/directories without links or reparse points",
        ));
    }
    Ok(())
}
pub(super) fn root(path: &Path) -> Result<std::path::PathBuf, Error> {
    let path = std::path::absolute(path)?;
    for ancestor in path.ancestors() {
        plain(ancestor, true)?;
    }
    Ok(path)
}
pub(super) fn open(path: &Path) -> Result<File, Error> {
    plain(path, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    nofollow(&mut options);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("Case member is not a regular file"));
    }
    Ok(file)
}
fn nofollow(options: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.custom_flags(0x0020_0000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(
            i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
                .expect("O_NOFOLLOW fits native flags"),
        );
    }
}
pub(super) fn bytes(path: &Path) -> Result<Vec<u8>, Error> {
    let file = open(path)?;
    if file.metadata()?.len() > 8 * 1024 * 1024 {
        return Err(invalid("Case manifest exceeds eight MiB"));
    }
    let mut result = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut result)?;
    if result.len() > 8 * 1024 * 1024 {
        return Err(invalid("Case manifest grew beyond its bound"));
    }
    Ok(result)
}
pub(super) fn lock(path: &Path) -> Result<File, Error> {
    let target = path.join(".case.lock");
    if target.exists() {
        plain(&target, false)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    nofollow(&mut options);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(0);
    }
    let file = options
        .open(target)
        .map_err(|_| invalid("Another writer holds this case, or its lock is unavailable"))?;
    #[cfg(unix)]
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(std::io::Error::from)?;
    Ok(file)
}
pub(super) fn layout(path: &Path) -> Result<usize, Error> {
    plain(&path.join("objects"), true)?;
    plain(&path.join("objects/sha256"), true)?;
    plain(&path.join("history"), true)?;
    let mut recovery = 0;
    for (count, entry) in std::fs::read_dir(path)?.enumerate() {
        if count >= 64 {
            return Err(invalid("Excessive working-case staging members"));
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| invalid("Invalid case member name"))?;
        if name.starts_with(".case-manifest-") {
            plain(&entry.path(), false)?;
            recovery += 1;
        } else if !matches!(name, "manifest.json" | "objects" | "history" | ".case.lock") {
            return Err(invalid("Unexpected working-case member"));
        } else if matches!(name, "manifest.json" | ".case.lock") {
            plain(&entry.path(), false)?;
        }
    }
    for entry in std::fs::read_dir(path.join("objects"))? {
        if entry?.file_name() != "sha256" {
            return Err(invalid("Unexpected object-store directory"));
        }
    }
    let mut count = 0;
    for entry in std::fs::read_dir(path.join("objects/sha256"))? {
        let entry = entry?;
        plain(&entry.path(), false)?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| invalid("Invalid object name"))?;
        if name.starts_with(".case-object-") {
            recovery += 1;
        } else if !crate::valid_digest(name) {
            return Err(invalid("Unexpected object-store member"));
        }
        count += 1;
        if count > 16384 * 32 {
            return Err(invalid("Working object-store count exceeded"));
        }
    }
    Ok(recovery)
}

pub(super) fn children(path: &Path) -> Result<Vec<std::fs::DirEntry>, Error> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        if entries.len() >= 64 {
            return Err(invalid("Excessive historical staging members"));
        }
        entries.push(entry?);
    }
    Ok(entries)
}
