//! Windows integration tests for manual diagnostic capture.

#![cfg(windows)]

use std::{
    fs,
    net::SocketAddr,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use transmog_host_windows::{ProxySettingsBackend, SystemProxyBackend};

async fn record_with_failed_previous_root_scan(profile: &Path, conflict: bool) -> (String, String) {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

    let capture = profile.join("capture.tmcap");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_transmog-cli"));
    command
        .env("LOCALAPPDATA", profile)
        .args(["record", "--output"])
        .arg(&capture)
        .args(["--no-system-proxy", "--no-install-root", "--duration", "2"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if conflict {
        command.args(["--circular-buffer", "1MiB"]);
    }
    let mut child = command.spawn().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut stdout = String::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let start = stdout.len();
            assert!(output.read_line(&mut stdout).await.unwrap() > 0, "{stdout}");
            if stdout[start..].starts_with("LISTEN_ADDR=") {
                break;
            }
        }
    })
    .await
    .unwrap();

    // Introduce the bad entry after setup so only the post-run sweep sees it.
    let malformed = profile.join("Transmog-cli/root-unreadable.json");
    let invalid = vec![b'x'; 8193];
    fs::write(&malformed, &invalid).unwrap();
    if conflict {
        fs::write(&capture, b"competing destination").unwrap();
    }
    let (result, _) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::try_join!(child.wait_with_output(), output.read_to_string(&mut stdout))
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.status.code(), Some(2), "{stdout}");
    assert_eq!(fs::read(malformed).unwrap(), invalid);
    (stdout, String::from_utf8(result.stderr).unwrap())
}

#[tokio::test]
async fn previous_root_scan_error_keeps_successful_trace_visible() {
    let profile = tempfile::tempdir().unwrap();
    let (stdout, stderr) = record_with_failed_previous_root_scan(profile.path(), false).await;
    let capture = profile.path().join("capture.tmcap");
    assert!(
        stdout.contains(&format!("Trace saved: {}", capture.display())),
        "{stdout}"
    );
    assert!(
        stderr.contains("Could not check previous or rotated roots"),
        "{stderr}"
    );
    let recovered = transmog_capture::recover(
        fs::File::open(capture).unwrap(),
        transmog_capture::CaptureLimits::default(),
    )
    .unwrap();
    assert!(recovered.sealed);
}

#[tokio::test]
async fn previous_root_scan_error_keeps_the_circular_recovery_path_visible() {
    let profile = tempfile::tempdir().unwrap();
    let (stdout, stderr) = record_with_failed_previous_root_scan(profile.path(), true).await;
    assert!(!stdout.contains("Trace saved:"));
    let recovery = stderr
        .lines()
        .find_map(|line| {
            line.strip_prefix("transmog-cli: Cannot save to the requested destination:")?
                .split_once("preserved at ")?
                .1
                .split_once(". Use capture inspect")
                .map(|(path, _)| path)
        })
        .unwrap_or_else(|| panic!("Missing circular recovery path in {stderr}"));
    assert!(
        stderr.contains("Could not check previous or rotated roots"),
        "{stderr}"
    );
    let recovered = transmog_capture::recover(
        fs::File::open(recovery).unwrap(),
        transmog_capture::CaptureLimits::default(),
    )
    .unwrap();
    assert!(recovered.sealed);
    assert_eq!(
        fs::read(profile.path().join("capture.tmcap")).unwrap(),
        b"competing destination"
    );
}

#[test]
fn manual_record_binds_and_exits_with_an_abandoned_proxy_journal() {
    let profile = tempfile::tempdir().unwrap();
    let state = profile.path().join("Transmog-cli");
    fs::create_dir(&state).unwrap();
    let journal = state.join("proxy-recovery.json");
    let pending = b"an interrupted prior run with an unreadable journal";
    fs::write(&journal, pending).unwrap();
    let before = SystemProxyBackend.read().unwrap();
    let capture = profile.path().join("manual.tmcap");
    let result = Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .env("LOCALAPPDATA", profile.path())
        .args(["record", "--output"])
        .arg(&capture)
        .args([
            "--no-system-proxy",
            "--no-decrypt-https",
            "--listen",
            "127.0.0.1:0",
            "--duration",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    let endpoint: SocketAddr = output
        .lines()
        .find_map(|line| line.strip_prefix("LISTEN_ADDR="))
        .expect("actual address after bind")
        .parse()
        .unwrap();
    assert!(endpoint.ip().is_loopback());
    assert_ne!(endpoint.port(), 0);
    assert!(output.contains("Set your application's HTTP and HTTPS proxy"));
    assert!(output.contains("Finishing active requests and saving the recording."));
    assert!(!output.contains("restoring proxy settings"));
    assert!(!output.contains("Routing restoration:"));
    assert!(capture.is_file());
    assert_eq!(fs::read(&journal).unwrap(), pending);
    assert_eq!(SystemProxyBackend.read().unwrap(), before);
}

#[tokio::test]
async fn manual_record_does_not_claim_a_competing_destination_as_evidence() {
    use std::os::windows::io::AsHandle as _;
    use tokio::{io::AsyncWriteExt as _, net::windows::named_pipe::ServerOptions};

    let profile = tempfile::tempdir().unwrap();
    let capture = profile.path().join("competing.tmcap");
    let pipe = format!(
        r"\\.\pipe\transmog-record-preflight-{}-{}",
        std::process::id(),
        profile.path().file_name().unwrap().to_string_lossy()
    );
    let mut password = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe)
        .unwrap();
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .env("LOCALAPPDATA", profile.path())
        .args(["record", "--output"])
        .arg(&capture)
        .args([
            "--no-system-proxy",
            "--no-decrypt-https",
            "--duration",
            "1",
            "--encrypt",
            "--password-file",
            &pipe,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Password input opens after destination preflight and before the capture writer.
    tokio::time::timeout(Duration::from_secs(10), password.connect())
        .await
        .unwrap()
        .unwrap();
    fs::write(&capture, b"another program's file").unwrap();
    password
        .write_all(b"disposable test password\n")
        .await
        .unwrap();
    let drain = fs::File::from(password.as_handle().try_clone_to_owned().unwrap());
    tokio::task::spawn_blocking(move || drain.sync_all())
        .await
        .unwrap()
        .unwrap();
    drop(password);

    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stderr.contains("capture destination already exists"),
        "{stderr}"
    );
    assert!(!stderr.contains("native evidence remains"), "{stderr}");
    assert!(!stdout.contains("LISTEN_ADDR="), "{stdout}");
    assert_eq!(fs::read(&capture).unwrap(), b"another program's file");
}

#[test]
fn system_record_seals_setup_metadata_when_proxy_ownership_is_unavailable() {
    let profile = tempfile::tempdir().unwrap();
    let state = profile.path().join("Transmog-cli");
    fs::create_dir(&state).unwrap();
    let lease = fs::File::create(state.join("proxy-recovery.lock")).unwrap();
    lease.try_lock().unwrap();
    let before = SystemProxyBackend.read().unwrap();
    let capture = profile.path().join("setup.tmcap");
    let result = Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .env("LOCALAPPDATA", profile.path())
        .args(["record", "--output"])
        .arg(&capture)
        .args(["--no-decrypt-https", "--duration", "1"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("host integration apply failed"), "{stderr}");
    assert!(!stdout.contains("LISTEN_ADDR="), "{stdout}");
    assert!(!stdout.contains("Trace saved:"), "{stdout}");
    let setup = transmog_capture::recover(
        fs::File::open(&capture).unwrap(),
        transmog_capture::CaptureLimits::default(),
    )
    .unwrap();
    assert!(setup.sealed, "{stderr}");
    assert!(setup.records.iter().all(|record| record.exchange_id == 0));
    assert!(stderr.contains("setup metadata"), "{stderr}");
    assert_eq!(SystemProxyBackend.read().unwrap(), before);
}
