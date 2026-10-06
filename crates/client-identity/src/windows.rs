use std::{
    ffi::OsString,
    mem::size_of,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    os::windows::ffi::OsStringExt as _,
    path::PathBuf,
    ptr,
};

use transmog_core::ClientIdentity;
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_CONNECTIONS,
    },
    Networking::WinSock::{AF_INET, AF_INET6},
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
};

const MAX_TABLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_PATH_UNITS: usize = 32_768;

pub(super) fn resolve(client_addr: SocketAddr, proxy_addr: SocketAddr) -> Option<ClientIdentity> {
    let pid = match (client_addr, proxy_addr) {
        (SocketAddr::V4(client), SocketAddr::V4(proxy)) => {
            query_rows::<MIB_TCPROW_OWNER_PID>(u32::from(AF_INET))?
                .into_iter()
                .find(|row| ipv4_row_matches(row, client, proxy))
                .map(|row| row.dwOwningPid)
        }
        (SocketAddr::V6(client), SocketAddr::V6(proxy)) => {
            query_rows::<MIB_TCP6ROW_OWNER_PID>(u32::from(AF_INET6))?
                .into_iter()
                .find(|row| ipv6_row_matches(row, &client, &proxy))
                .map(|row| row.dwOwningPid)
        }
        _ => None,
    }?;

    Some(ClientIdentity::LocalProcess {
        pid,
        name: process_name(pid),
    })
}

fn query_rows<Row: Copy>(family: u32) -> Option<Vec<Row>> {
    let mut byte_count = 0_u32;
    // SAFETY: a null table pointer is the documented size-query form. The
    // writable size pointer is valid for the duration of the call.
    let first = unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &raw mut byte_count,
            0,
            family,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
            0,
        )
    };
    if first != ERROR_INSUFFICIENT_BUFFER && first != NO_ERROR {
        return None;
    }

    for _ in 0..3 {
        let requested = usize::try_from(byte_count).ok()?;
        if requested < size_of::<u32>() || requested > MAX_TABLE_BYTES {
            return None;
        }
        // A u64 backing store provides stronger alignment than every table row
        // and is rounded up to cover the exact byte count Windows requested.
        let word_count = requested.checked_add(size_of::<u64>() - 1)? / size_of::<u64>();
        let mut storage = vec![0_u64; word_count];
        let mut actual = u32::try_from(storage.len().checked_mul(size_of::<u64>())?).ok()?;
        // SAFETY: storage is writable for `actual` bytes and its pointer stays
        // stable during the call. The family determines Row at each call site.
        let result = unsafe {
            GetExtendedTcpTable(
                storage.as_mut_ptr().cast(),
                &raw mut actual,
                0,
                family,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            byte_count = actual;
            continue;
        }
        if result != NO_ERROR {
            return None;
        }

        let actual = usize::try_from(actual).ok()?;
        if actual < size_of::<u32>() || actual > storage.len() * size_of::<u64>() {
            return None;
        }
        let bytes = storage.as_ptr().cast::<u8>();
        // SAFETY: the validated table buffer begins with the documented u32
        // entry count. read_unaligned avoids relying on the allocation layout.
        let count = usize::try_from(unsafe { ptr::read_unaligned(bytes.cast::<u32>()) }).ok()?;
        let row_bytes = count.checked_mul(size_of::<Row>())?;
        let required = size_of::<u32>().checked_add(row_bytes)?;
        if required > actual {
            return None;
        }
        let mut rows = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: `required <= actual` proves each complete row is inside
            // storage. Windows places the first row directly after dwNumEntries.
            let row = unsafe {
                ptr::read_unaligned(
                    bytes
                        .add(size_of::<u32>() + index * size_of::<Row>())
                        .cast::<Row>(),
                )
            };
            rows.push(row);
        }
        return Some(rows);
    }
    None
}

fn ipv4_row_matches(
    row: &MIB_TCPROW_OWNER_PID,
    client: std::net::SocketAddrV4,
    proxy: std::net::SocketAddrV4,
) -> bool {
    Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()) == *client.ip()
        && decode_port(row.dwLocalPort) == client.port()
        && Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes()) == *proxy.ip()
        && decode_port(row.dwRemotePort) == proxy.port()
}

fn ipv6_row_matches(
    row: &MIB_TCP6ROW_OWNER_PID,
    client: &std::net::SocketAddrV6,
    proxy: &std::net::SocketAddrV6,
) -> bool {
    Ipv6Addr::from(row.ucLocalAddr) == *client.ip()
        && row.dwLocalScopeId == client.scope_id()
        && decode_port(row.dwLocalPort) == client.port()
        && Ipv6Addr::from(row.ucRemoteAddr) == *proxy.ip()
        && row.dwRemoteScopeId == proxy.scope_id()
        && decode_port(row.dwRemotePort) == proxy.port()
}

fn decode_port(value: u32) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn process_name(pid: u32) -> Option<String> {
    // SAFETY: OpenProcess receives a PID reported by the TCP table and requests
    // query-only access. A null result is handled without dereferencing it.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let mut path = vec![0_u16; MAX_IMAGE_PATH_UNITS];
    let mut length = u32::try_from(path.len()).ok()?;
    // SAFETY: `path` is a writable UTF-16 buffer of `length` units and the
    // process handle remains open until after the call.
    let queried =
        unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &raw mut length) };
    // SAFETY: `process` is a non-null handle returned by OpenProcess and is
    // closed exactly once here, after its final use.
    unsafe { CloseHandle(process) };
    if queried == 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    if length > path.len() {
        return None;
    }
    let full_path = PathBuf::from(OsString::from_wide(&path[..length]));
    full_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn network_order_ports_are_decoded() {
        assert_eq!(decode_port(u32::from(8080_u16.to_be())), 8080);
    }

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

    #[test]
    fn current_process_name_is_a_file_name() {
        let name = process_name(std::process::id()).expect("current process is queryable");
        assert!(!name.is_empty());
        assert!(!name.contains(['\\', '/']));
    }
}
