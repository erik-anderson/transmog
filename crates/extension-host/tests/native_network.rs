//! Native network probes observe owned peers because Windows may silently drop denied UDP sends.
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
#[allow(clippy::too_many_lines)] // One standalone test executable orchestrates its own isolated child and owned peers.
fn main() {
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
        os::windows::process::CommandExt as _,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--sandbox-bootstrap") {
        std::process::exit(
            transmog_extension_host::sandbox_bootstrap(64 * 1024 * 1024)
                .expect("probe sandbox bootstrap"),
        );
    }
    if args.first().is_some_and(|a| a == "--sandboxed") {
        transmog_process_sandbox::verify_current_process().expect("native probe must be contained");
        let mut targets = String::new();
        std::io::stdin()
            .take(1024)
            .read_to_string(&mut targets)
            .unwrap();
        let targets: Vec<SocketAddr> = targets
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        let result = TcpStream::connect_timeout(&targets[0], Duration::from_millis(500));
        // WFP may drop loopback attempts instead of returning WSAEACCES. The parent
        // also checks its listening peer to distinguish a real connection.
        assert!(result.is_err(), "loopback TCP connection succeeded");
        let external: SocketAddr = "192.0.2.1:443".parse().unwrap();
        assert!(
            TcpStream::connect_timeout(&external, Duration::from_millis(200))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(10013)),
            "external TCP was not denied by policy"
        );
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            // A successful UDP enqueue does not prove delivery; owned receivers
            // outside the sandbox verify that no packet crosses its boundary.
            let _ = socket.send_to(b"loopback-probe", targets[1]);
            let _ = socket.send_to(&[0_u8; 12], targets[2]);
            let _ = socket.send_to(b"lan-probe", targets[3]);
        }
        println!("probed");
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.set_nonblocking(true).unwrap();
    let dns = UdpSocket::bind("127.0.0.1:0").unwrap();
    dns.set_nonblocking(true).unwrap();
    let route = UdpSocket::bind("0.0.0.0:0").unwrap();
    route.connect("192.0.2.1:1").unwrap();
    let lan = UdpSocket::bind((route.local_addr().unwrap().ip(), 0)).unwrap();
    lan.set_nonblocking(true).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .creation_flags(0x0800_0000)
        .arg("--sandbox-bootstrap")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    write!(
        input,
        "{} {} {} {}",
        listener.local_addr().unwrap(),
        udp.local_addr().unwrap(),
        dns.local_addr().unwrap(),
        lan.local_addr().unwrap()
    )
    .unwrap();
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native network probe timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut result = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut result)
        .unwrap();
    let mut errors = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(
        status.success(),
        "native network probe failed: {result} {errors}"
    );
    assert_eq!(result.trim(), "probed");
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        listener
            .accept()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
    );
    for (label, socket) in [("loopback UDP", udp), ("DNS packet", dns), ("LAN UDP", lan)] {
        assert!(
            socket
                .recv_from(&mut [0_u8; 128])
                .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock),
            "{label} crossed the sandbox"
        );
    }
    println!("Native sandbox blocked loopback TCP/UDP, outbound TCP, DNS packets and LAN UDP");
}
