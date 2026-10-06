#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::{
    fs::{self, File},
    io::{self, Read as _},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
};

use transmog_core::ClientIdentity;

const MAX_TCP_TABLE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROCESSES: usize = 32_768;
const MAX_FDS_PER_PROCESS: usize = 16_384;

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
        for (word_index, chunk) in address.as_bytes().chunks_exact(8).enumerate() {
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
            if fs::read_link(descriptor.path())
                .ok()
                .is_some_and(|target| target == Path::new(&needle))
            {
                return Some(pid);
            }
        }
    }
    None
}

fn process_name(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            fs::read_link(format!("/proc/{pid}/exe"))
                .ok()?
                .file_name()?
                .to_str()
                .map(ToOwned::to_owned)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::net::{TcpListener, TcpStream};

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
}
