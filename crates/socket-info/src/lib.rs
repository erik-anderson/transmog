#![deny(unsafe_op_in_unsafe_fn)]
//! Read-only TCP statistics. Unsupported socket/platform queries return none;
//! these functions never enable counters, elevate, or change socket options.
use transmog_core::performance::TcpObservation;

/// Samples a borrowed Windows TCP socket synchronously, without retaining its handle.
#[cfg(windows)]
pub fn sample(socket: std::os::windows::io::BorrowedSocket<'_>) -> Option<TcpObservation> {
    use std::{mem::size_of, os::windows::io::AsRawSocket, ptr};
    use windows_sys::Win32::Networking::WinSock::{SIO_TCP_INFO, TCP_INFO_v0, WSAIoctl};
    let version = 0_u32;
    let mut info = TCP_INFO_v0::default();
    let mut returned = 0_u32;
    let handle = usize::try_from(socket.as_raw_socket()).ok()?;
    // SAFETY: The borrowed socket remains open for this synchronous call. Input
    // and initialized output are correctly sized, aligned, live stack objects;
    // null OVERLAPPED/completion pointers make the operation synchronous.
    let result = unsafe {
        WSAIoctl(
            handle,
            SIO_TCP_INFO,
            ptr::from_ref(&version).cast(),
            u32::try_from(size_of::<u32>()).ok()?,
            ptr::from_mut(&mut info).cast(),
            u32::try_from(size_of::<TCP_INFO_v0>()).ok()?,
            ptr::from_mut(&mut returned),
            ptr::null_mut(),
            None,
        )
    };
    if result != 0 || returned as usize != size_of::<TCP_INFO_v0>() {
        return None;
    }
    Some(TcpObservation {
        rtt_micros: Some(info.RttUs.into()),
        min_rtt_micros: Some(info.MinRttUs.into()),
        congestion_window: Some(info.Cwnd.into()),
        send_window: Some(info.SndWnd.into()),
        receive_window: Some(info.RcvWnd.into()),
        unacknowledged_bytes: Some(info.BytesInFlight.into()),
        retransmitted_bytes: Some(info.BytesRetrans.into()),
        fast_retransmissions: Some(info.FastRetrans.into()),
        duplicate_acks: Some(info.DupAcksIn.into()),
        timeout_episodes: Some(info.TimeoutEpisodes.into()),
        mss: Some(info.Mss.into()),
        connection_age_millis: Some(info.ConnectionTimeMs),
        ..TcpObservation::default()
    })
}

/// Samples Linux TCP_INFO on a borrowed descriptor; missing fields remain unavailable.
#[cfg(target_os = "linux")]
pub fn sample(socket: std::os::fd::BorrowedFd<'_>) -> Option<TcpObservation> {
    use std::{mem::size_of, os::fd::AsRawFd, ptr};
    // SAFETY: tcp_info is a C integer-only record, valid when zero initialized.
    let mut info: libc::tcp_info = unsafe { std::mem::zeroed() };
    let mut length = libc::socklen_t::try_from(size_of::<libc::tcp_info>()).ok()?;
    // SAFETY: The borrowed descriptor stays live; the initialized record is
    // writable for length bytes. getsockopt synchronously writes at most length.
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            ptr::from_mut(&mut info).cast(),
            ptr::from_mut(&mut length),
        )
    };
    if result != 0
        || (length as usize)
            < std::mem::offset_of!(libc::tcp_info, tcpi_total_retrans) + size_of::<u32>()
    {
        return None;
    }
    Some(TcpObservation {
        rtt_micros: Some(info.tcpi_rtt.into()),
        congestion_window: Some(u64::from(info.tcpi_snd_cwnd) * u64::from(info.tcpi_snd_mss)),
        retransmitted_segments: Some(info.tcpi_total_retrans.into()),
        mss: Some(info.tcpi_snd_mss.into()),
        ..TcpObservation::default()
    })
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    #[test]
    fn queries_live_loopback_socket_without_changing_it() {
        use std::{
            io::{Read, Write},
            net::{TcpListener, TcpStream},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"probe").unwrap();
        let mut bytes = [0; 5];
        server.read_exact(&mut bytes).unwrap();
        #[cfg(windows)]
        let sample = {
            use std::os::windows::io::AsSocket;
            sample(client.as_socket())
        };
        #[cfg(target_os = "linux")]
        let sample = {
            use std::os::fd::AsFd;
            sample(client.as_fd())
        };
        let info = sample.expect("TCP statistics supported on the qualification OS");
        assert!(info.mss.is_some_and(|mss| mss > 0));
        assert!(info.rtt_micros.is_some());
        server.write_all(b"reply").unwrap();
        client.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"reply");
    }
}
