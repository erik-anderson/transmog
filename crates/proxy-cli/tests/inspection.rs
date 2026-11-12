//! Exercise inspection option validation and certificate-free recording at the command boundary.
#![cfg(any(windows, target_os = "linux"))]

use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use transmog_capture::{CaptureLimits, CaptureRecordKind};

fn invoke(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .args(arguments)
        .current_dir(directory)
        .env("LOCALAPPDATA", directory.join("state"))
        .env("XDG_STATE_HOME", directory.join("state"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
#[allow(clippy::too_many_lines)] // Keep the command rejection matrix with its no-write assertion.
fn invalid_inspection_options_leave_output_and_preferences_untouched() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--password",
            "--no-decrypt-https",
            "--no-install-root",
        ],
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--process",
            "browser",
            "--exclude-process",
            "worker",
        ],
        vec!["record", "--output", "capture.tmcap", "--process", "   "],
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--process",
            "browser",
            "--process",
        ],
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--no-decrypt-https",
            "--install-root",
        ],
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--no-decrypt-https",
            "--persistent-root",
        ],
        vec![
            "record",
            "--output",
            "capture.tmcap",
            "--process",
            "browser",
            "--output",
            "other.tmcap",
        ],
        vec![
            "serve",
            "--capture",
            "capture.tmcap",
            "--no-decrypt-https",
            "--ca-key",
            "missing.key",
        ],
        vec![
            "serve",
            "--capture",
            "capture.tmcap",
            "--no-decrypt-https",
            "--ca-cert",
            "missing.pem",
        ],
        vec![
            "serve",
            "--capture",
            "capture.tmcap",
            "--no-decrypt-https",
            "--process",
            "browser",
            "--exclude-process",
            "worker",
        ],
        vec![
            "serve",
            "--capture",
            "capture.tmcap",
            "--no-decrypt-https",
            "--exclude-process",
            "../worker",
        ],
        vec![
            "serve",
            "--capture",
            "capture.tmcap",
            "--no-decrypt-https",
            "--listen",
            "127.0.0.1:0",
            "--listen",
            "127.0.0.1:1",
        ],
        vec!["serve", "--capture", "capture.tmcap"],
    ] {
        let output = invoke(directory.path(), &arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("transmog-cli:"));
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            0,
            "{arguments:?}"
        );
    }
}

#[test]
fn encrypted_certificate_free_recording_accepts_option_like_password_file_content() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("password.txt"), "--install-root").unwrap();
    let output = invoke(
        directory.path(),
        &[
            "record",
            "--output",
            "encrypted.tmcap",
            "--duration",
            "1",
            "--no-system-proxy",
            "--no-decrypt-https",
            "--encrypt",
            "--password-file",
            "password.txt",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let password = transmog_capture::CapturePassword::new("--install-root".into());
    let capture = transmog_capture::recover_with_password(
        fs::File::open(directory.path().join("encrypted.tmcap")).unwrap(),
        CaptureLimits::default(),
        Some(&password),
    )
    .unwrap();
    assert!(capture.sealed);
    let ledger = directory.path().join("state/Transmog-cli");
    assert!(fs::read_dir(ledger).unwrap().all(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        name == "root-workflow.lock" || name.starts_with("privacy-")
    }));
    assert!(capture.records.iter().any(|record| matches!(&record.kind,
        CaptureRecordKind::Unknown { kind, payload } if kind == "trace-metadata"
            && payload["httpsDecryption"] == false && payload["certificateContext"].is_null()
    )));
}

#[test]
fn certificate_free_recording_seals_without_touching_existing_roots() {
    let directory = tempfile::tempdir().unwrap();
    let ledger = directory.path().join("state/Transmog-cli");
    fs::create_dir_all(&ledger).unwrap();
    // Even malformed previous ownership and private material must remain untouched:
    // this mode never attempts to load, repair, retire or clean up a CA.
    for name in ["root-existing.json", "existing.pem", "existing.key"] {
        fs::write(ledger.join(name), b"unreadable previous certificate state").unwrap();
    }
    let output = invoke(
        directory.path(),
        &[
            "record",
            "--output",
            "opaque.tmcap",
            "--duration",
            "1",
            "--no-system-proxy",
            "--no-decrypt-https",
            "--process",
            "browser",
            "--process",
            "worker",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Process inspection: Only"), "{stdout}");
    assert!(
        stdout.contains("HTTPS mode: Opaque CONNECT tunnels"),
        "{stdout}"
    );
    assert!(stdout.contains("Trace saved:"), "{stdout}");
    assert!(
        !stdout.contains("Root SHA-256")
            && !stdout.contains("Removing this capture's trusted root")
    );
    for name in ["root-existing.json", "existing.pem", "existing.key"] {
        assert_eq!(
            fs::read(ledger.join(name)).unwrap(),
            b"unreadable previous certificate state"
        );
    }
    let mut names = fs::read_dir(&ledger)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert!(
        names.iter().all(|name| matches!(
            name.as_str(),
            "root-existing.json" | "existing.pem" | "existing.key" | "root-workflow.lock"
        ) || name.starts_with("privacy-")),
        "{names:?}"
    );
    let capture = transmog_capture::recover(
        fs::File::open(directory.path().join("opaque.tmcap")).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    assert!(capture.sealed);
    let metadata = capture
        .records
        .iter()
        .find_map(|record| match &record.kind {
            CaptureRecordKind::Unknown { kind, payload } if kind == "trace-metadata" => {
                Some(payload)
            }
            _ => None,
        })
        .unwrap();
    assert!(metadata["certificateContext"].is_null());
    assert_eq!(metadata["httpsDecryption"], false);
    assert_eq!(metadata["processInspection"]["mode"], "only");
    assert_eq!(
        metadata["processInspection"]["include"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
