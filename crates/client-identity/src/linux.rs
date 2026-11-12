#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read as _},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
};

use transmog_core::ClientIdentity;

const MAX_TCP_TABLE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROCESSES: usize = 32_768;
const MAX_FDS_PER_PROCESS: usize = 16_384;

pub(super) fn running_process_names() -> io::Result<Vec<String>> {
    let names = fs::read_dir("/proc")?
        .take(MAX_PROCESSES)
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(process_name)
        .collect::<BTreeSet<_>>();
    Ok(names.into_iter().collect())
}

pub(super) fn resolve(client_addr: SocketAddr, proxy_addr: SocketAddr) -> Option<ClientIdentity> {
    let table = match (client_addr, proxy_addr) {
        (SocketAddr::V4(_), SocketAddr::V4(_)) => "/proc/net/tcp",
        (SocketAddr::V6(_), SocketAddr::V6(_)) => "/proc/net/tcp6",
        _ => return None,
    };
    let text = read_bounded(Path::new(table), MAX_TCP_TABLE_BYTES).ok()?;
    let inode = find_socket_inode(&text, client_addr, proxy_addr)?;
    let pid = find_process_with_socket(inode)?;
    Some(ClientIdentity::LocalProcess {
        pid,
        name: process_name(pid),
    })
}

fn read_bounded(path: &Path, limit: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "kernel TCP table exceeded attribution limit",
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn find_socket_inode(table: &str, client_addr: SocketAddr, proxy_addr: SocketAddr) -> Option<u64> {
    table.lines().skip(1).find_map(|line| {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() < 10 {
            return None;
        }
        let local = parse_endpoint(fields[1], client_addr.is_ipv6())?;
        let remote = parse_endpoint(fields[2], client_addr.is_ipv6())?;
        (local == client_addr && remote == proxy_addr)
            .then(|| fields[9].parse::<u64>().ok())
            .flatten()
    })
}

fn parse_endpoint(value: &str, ipv6: bool) -> Option<SocketAddr> {
    let (address, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    if ipv6 {
        if address.len() != 32 {
            return None;
        }
        let mut octets = [0_u8; 16];
        for (word_index, chunk) in address.as_bytes().as_chunks::<8>().0.iter().enumerate() {
            let chunk = std::str::from_utf8(chunk).ok()?;
            let word = u32::from_str_radix(chunk, 16).ok()?;
            octets[word_index * 4..word_index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
    } else {
        let raw = u32::from_str_radix(address, 16).ok()?;
        Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::from(raw.to_le_bytes())),
            port,
        ))
    }
}

fn find_process_with_socket(inode: u64) -> Option<u32> {
    let needle = format!("socket:[{inode}]");
    for process in fs::read_dir("/proc").ok()?.take(MAX_PROCESSES).flatten() {
        let file_name = process.file_name();
        let Some(pid) = file_name
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(descriptors) = fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.take(MAX_FDS_PER_PROCESS).flatten() {
            if fs::read_link(descriptor.path()).is_ok_and(|target| target == Path::new(&needle)) {
                return Some(pid);
            }
        }
    }
    None
}

pub(super) fn process_name(pid: u32) -> Option<String> {
    // comm is mutable and normally truncated to 15 bytes, so it cannot safely
    // identify a caller for executable-name exclusions. Missing image access
    // remains nameless, which bypasses inspection when a filter is active.
    let image = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
    let path = fs::read_link(&image).ok()?;
    let name = path.file_name()?.to_str()?;
    let name = if let Some(original) = name.strip_suffix(" (deleted)") {
        if has_deleted_marker(&image, &path)? {
            original
        } else {
            name
        }
    } else {
        name
    };
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(target_os = "linux")]
fn has_deleted_marker(image: &Path, _path: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let image = fs::metadata(image).ok()?;
    if image.nlink() == 0 {
        // The inode has no links, so procfs appended exactly one marker. A
        // literal suffix on the original executable remains in its basename.
        return Some(true);
    }
    // A still-linked executable may literally end in " (deleted)", or its
    // original dentry may have been unlinked while another hard link remains.
    // Even an exact inode match at the reported path cannot distinguish those
    // cases: the surviving hard link may have that same suffix. Stay nameless.
    None
}

#[cfg(not(target_os = "linux"))]
fn has_deleted_marker(_image: &Path, _path: &Path) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::{
        net::{TcpListener, TcpStream},
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[cfg(target_os = "linux")]
    struct ExecutableFixture {
        directory: PathBuf,
        executable: PathBuf,
        child: Child,
    }

    #[cfg(target_os = "linux")]
    impl ExecutableFixture {
        fn spawn(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "transmog-image-name-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&directory).unwrap();
            let directory = fs::canonicalize(directory).unwrap();
            let executable = directory.join(name);
            fs::copy("/bin/sleep", &executable).unwrap();
            let child = Command::new(&executable)
                .arg("60")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self {
                directory,
                executable,
                child,
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for ExecutableFixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_file(&self.executable);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_executable_names_are_exact_or_nameless_before_and_after_unlink() {
        for name in ["full-executable-name", "literal (deleted)"] {
            let fixture = ExecutableFixture::spawn(name);
            let policy = transmog_core::process_inspection::ProcessInspectionPolicy {
                mode: transmog_core::process_inspection::ProcessInspectionMode::Except,
                include: vec![],
                exclude: vec![name.into()],
            }
            .validated()
            .unwrap();
            let linked_name = (!name.ends_with(" (deleted)")).then_some(name);
            assert_eq!(process_name(fixture.child.id()).as_deref(), linked_name);
            assert!(!policy.should_inspect(&ClientIdentity::LocalProcess {
                pid: fixture.child.id(),
                name: process_name(fixture.child.id()),
            }));
            if let Some(name) = linked_name {
                assert!(
                    running_process_names()
                        .unwrap()
                        .iter()
                        .any(|item| item == name)
                );
            }
            fs::remove_file(&fixture.executable).unwrap();
            let image = fs::read_link(format!("/proc/{}/exe", fixture.child.id())).unwrap();
            assert_eq!(
                image.file_name().unwrap(),
                format!("{name} (deleted)").as_str()
            );
            assert_eq!(process_name(fixture.child.id()).as_deref(), Some(name));
            assert!(!policy.should_inspect(&ClientIdentity::LocalProcess {
                pid: fixture.child.id(),
                name: process_name(fixture.child.id()),
            }));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unlinked_image_with_a_remaining_hard_link_is_nameless() {
        let fixture = ExecutableFixture::spawn("client");
        let retained = fixture.directory.join("another-link");
        fs::hard_link(&fixture.executable, &retained).unwrap();
        fs::remove_file(&fixture.executable).unwrap();
        assert_eq!(process_name(fixture.child.id()), None);
        fs::remove_file(retained).unwrap();
        assert_eq!(process_name(fixture.child.id()).as_deref(), Some("client"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unlinked_image_with_a_suffix_named_hard_link_is_nameless() {
        let fixture = ExecutableFixture::spawn("client");
        let retained = fixture.directory.join("client (deleted)");
        fs::hard_link(&fixture.executable, &retained).unwrap();
        fs::remove_file(&fixture.executable).unwrap();
        let image = fs::read_link(format!("/proc/{}/exe", fixture.child.id())).unwrap();
        assert_eq!(image, retained);
        assert_eq!(process_name(fixture.child.id()), None);
        let policy = transmog_core::process_inspection::ProcessInspectionPolicy {
            mode: transmog_core::process_inspection::ProcessInspectionMode::Except,
            include: vec![],
            exclude: vec!["client".into()],
        }
        .validated()
        .unwrap();
        assert!(!policy.should_inspect(&ClientIdentity::LocalProcess {
            pid: fixture.child.id(),
            name: process_name(fixture.child.id()),
        }));
        fs::remove_file(retained).unwrap();
        assert_eq!(process_name(fixture.child.id()).as_deref(), Some("client"));
    }

    #[test]
    fn parses_ipv4_and_ipv6_proc_endpoints() {
        assert_eq!(
            parse_endpoint("0100007F:1F90", false),
            Some("127.0.0.1:8080".parse().unwrap())
        );
        assert_eq!(
            parse_endpoint("00000000000000000000000001000000:1F90", true),
            Some("[::1]:8080".parse().unwrap())
        );
    }

    #[test]
    fn finds_an_exact_connection_inode() {
        let table = "  sl  local_address rem_address st tx_queue tr tm->when retrnsmt uid timeout inode\n  1: 0100007F:C350 0100007F:1F90 01 00000000:00000000 00:00000000 00000000 1000 0 424242 1\n";
        assert_eq!(
            find_socket_inode(
                table,
                "127.0.0.1:50000".parse().unwrap(),
                "127.0.0.1:8080".parse().unwrap(),
            ),
            Some(424_242)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_loopback_socket_resolves_to_the_current_process() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(proxy_addr).unwrap();
        let client_addr = client.local_addr().unwrap();
        let (_accepted, accepted_peer) = listener.accept().unwrap();
        assert_eq!(client_addr, accepted_peer);

        let identity = resolve(client_addr, proxy_addr).expect("client process should resolve");
        assert!(matches!(
            identity,
            ClientIdentity::LocalProcess { pid, .. } if pid == std::process::id()
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inventory_and_attribution_use_the_complete_executable_name() {
        let executable = std::env::current_exe().unwrap();
        let expected = executable.file_name().unwrap().to_str().unwrap();
        assert_eq!(process_name(std::process::id()).as_deref(), Some(expected));
        let names = running_process_names().unwrap();
        assert!(names.iter().any(|name| name == expected));
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
