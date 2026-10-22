//! Exercise the distributed command boundary, with synthetic local capture data.
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};
use transmog_capture::{
    CaptureEncoding, CaptureLimits, CapturePassword, CaptureRecord, CaptureRecordKind,
    CaptureWriter,
};

const PASSWORD: &str = "  synthetic Café password with \"quotes\", \\slashes\\ and 'apostrophes'  ";

fn invoke(input: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .arg("context")
        .arg("--input")
        .arg(input)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(output: Output) -> Value {
    let Output {
        status,
        stdout,
        stderr,
    } = output;
    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    serde_json::from_slice(&stdout).unwrap()
}
fn failure(output: Output) -> String {
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, [] as [u8; 0]);
    String::from_utf8(output.stderr).unwrap()
}
fn network() -> Value {
    json!({"collector":"original collector","purpose":"capture-start","computerName":"original machine","platform":"fixture","collectedAt":1_700_000_000_000_u64,"command":"fixture tool (inert)","output":"DNS Café\r\nsetting=value\r\n","notes":["second tool unavailable"]})
}
fn fixture(path: &Path, password: Option<&str>, sealed: bool, metadata: Value) {
    let mut writer = CaptureWriter::with_encoding(
        fs::File::create(path).unwrap(),
        CaptureLimits::default(),
        &CaptureEncoding {
            password: password.map(|v| CapturePassword::new(v.into())),
        },
    )
    .unwrap();
    for (kind, payload) in [
        ("trace-metadata", metadata),
        (
            "trace-source",
            json!({"id":"source-original","name":"Original source","importedAt":1_999_999_999_999_u64,"context":{"networkContext":network(),"extension/~name":{"value":"original"}},"notes":["Source note <script>inert</script>"]}),
        ),
    ] {
        writer
            .append(&CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: CaptureRecordKind::Unknown {
                    kind: kind.into(),
                    payload,
                },
            })
            .unwrap();
    }
    if sealed {
        writer.seal().unwrap();
    } else {
        writer.checkpoint().unwrap();
    }
}

#[test]
fn list_and_extract_keep_source_time_issues_unicode_and_crlf() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("support Café.tmcap");
    fixture(
        &input,
        None,
        true,
        json!({"networkContext":network(),"supportCapture":{"expected":"original user observation"}}),
    );
    let original = fs::read(&input).unwrap();
    let listing = success(invoke(&input, &["--list"]));
    assert_eq!(listing["sealed"], true);
    assert_eq!(listing["bodyIntegrityChecked"], false);
    let contexts = listing["contexts"].as_array().unwrap();
    let sources: Vec<_> = contexts
        .iter()
        .filter(|v| v["kind"] == "networkContext")
        .collect();
    assert_eq!(sources.len(), 2);
    assert_ne!(sources[0]["id"], sources[1]["id"]);
    assert_eq!(sources[1]["sourceKey"], "source-original");
    assert_eq!(sources[1]["collectedAt"], 1_700_000_000_000_u64);
    assert_eq!(sources[1]["completeness"], "partial");
    assert_eq!(sources[1]["issues"][0], "second tool unavailable");
    assert!(
        !String::from_utf8(serde_json::to_vec(&listing).unwrap())
            .unwrap()
            .contains("setting=value")
    );
    assert_eq!(
        success(invoke(
            &input,
            &["--id", sources[1]["id"].as_str().unwrap()]
        )),
        network()
    );
    assert_eq!(
        success(invoke(
            &input,
            &["--id", "record/1/context/extension~1~0name"]
        )),
        json!({"value":"original"})
    );
    assert_eq!(
        success(invoke(&input, &["--id", "record/1/notes"])),
        json!(["Source note <script>inert</script>"])
    );
    let output = root.path().join("selected Café.json");
    let result = invoke(
        &input,
        &[
            "--id",
            "record/0/networkContext",
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!(result.status.success());
    assert_eq!(result.stdout, [] as [u8; 0]);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&output).unwrap()).unwrap(),
        network()
    );
    assert_eq!(fs::read(&input).unwrap(), original);
}

#[test]
fn encrypted_input_supports_arguments_files_and_nonblocking_failures() {
    let bad_help = Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
        .args(["help", "context", "--password", PASSWORD])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!failure(bad_help).contains(PASSWORD));
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("encrypted.tmcap");
    fixture(
        &input,
        Some(PASSWORD),
        true,
        json!({"networkContext":network()}),
    );
    assert!(failure(invoke(&input, &["--no-prompt"])).contains("--no-prompt"));
    assert!(failure(invoke(&input, &[])).contains("interactive console"));
    let missing = root.path().join("must-not-exist.json");
    assert!(
        failure(invoke(
            &input,
            &[
                "--password",
                "incorrect-private-value",
                "--output",
                missing.to_str().unwrap()
            ]
        ))
        .contains("password")
    );
    assert!(!missing.exists());
    let output = invoke(&input, &["--password", PASSWORD, "--no-prompt"]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PASSWORD));
    assert_eq!(success(output)["sealed"], true);
    let file = root.path().join("protected password.txt");
    fs::write(&file, format!("{PASSWORD}\r\n")).unwrap();
    assert_eq!(
        success(invoke(
            &input,
            &["--password-file", file.to_str().unwrap(), "--no-prompt"]
        ))["sealed"],
        true
    );
    let special = root.path().join("special.tmcap");
    for password in ["--help", "--id"] {
        fixture(
            &special,
            Some(password),
            true,
            json!({"networkContext":network()}),
        );
        assert_eq!(
            success(invoke(
                &special,
                &[
                    "--password",
                    password,
                    "--no-prompt",
                    "--id",
                    "record/0/networkContext"
                ]
            )),
            network()
        );
    }
}

#[test]
fn missing_context_bad_options_and_destinations_leave_originals_intact() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("support.tmcap");
    fixture(&input, None, true, json!({"networkContext":null}));
    let output = root.path().join("context.json");
    assert!(
        failure(invoke(
            &input,
            &[
                "--id",
                "record/0/networkContext",
                "--output",
                output.to_str().unwrap()
            ]
        ))
        .contains("not collected")
    );
    assert!(!output.exists());
    assert!(
        failure(invoke(
            &input,
            &["--id", "missing", "--output", output.to_str().unwrap()]
        ))
        .contains("ID not found")
    );
    assert!(!output.exists());
    fs::write(&output, b"existing").unwrap();
    assert!(
        failure(invoke(&input, &["--output", output.to_str().unwrap()])).contains("already exists")
    );
    assert_eq!(fs::read(&output).unwrap(), b"existing");
    for args in [
        vec!["--password", "a", "--password-file", "missing"],
        vec!["--list", "--id", "missing"],
        vec!["--no-prompt", "--no-prompt"],
        vec!["--unrecognized"],
        vec!["--id"],
    ] {
        failure(invoke(&root.path().join("does-not-exist"), &args));
    }
    let before = fs::read(&input).unwrap();
    failure(invoke(&input, &["--output", input.to_str().unwrap()]));
    assert_eq!(fs::read(input).unwrap(), before);
}

#[test]
fn committed_prefixes_and_context_limits_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("partial.tmcap");
    fixture(&input, None, false, json!({"networkContext":network()}));
    let output = invoke(&input, &["--list"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("committed capture prefix"));
    assert_eq!(success(output)["sealed"], false);
    let large = root.path().join("many.tmcap");
    let metadata: serde_json::Map<String, Value> =
        (0..257).map(|n| (format!("field-{n}"), json!(n))).collect();
    fixture(&large, None, true, metadata.into());
    let destination = root.path().join("limited.json");
    assert!(
        failure(invoke(&large, &["--output", destination.to_str().unwrap()])).contains("256-item")
    );
    assert!(!destination.exists());
}

#[test]
fn empty_captures_and_original_incidents_are_readable_without_traffic() {
    use transmog_capture::support::{IncidentMarker, MarkerOrigin};
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("empty.tmcap");
    let mut writer =
        CaptureWriter::new(fs::File::create(&input).unwrap(), CaptureLimits::default()).unwrap();
    writer.seal().unwrap();
    drop(writer);
    assert_eq!(success(invoke(&input, &[]))["contexts"], json!([]));
    let input = root.path().join("incidents.tmcap");
    let mut writer =
        CaptureWriter::new(fs::File::create(&input).unwrap(), CaptureLimits::default()).unwrap();
    let marker = IncidentMarker::new(
        transmog_capture::support::new_id().unwrap(),
        Some(11),
        MarkerOrigin::Cli,
        "Original support note <script>inert</script>".into(),
    )
    .unwrap();
    writer.append(&marker.record().unwrap()).unwrap();
    let retention = json!({"evictedExchanges":3,"reason":"original circular retention"});
    writer
        .append(&CaptureRecord {
            sequence: 0,
            exchange_id: 0,
            kind: CaptureRecordKind::Unknown {
                kind: "circular-retention".into(),
                payload: retention.clone(),
            },
        })
        .unwrap();
    writer.seal().unwrap();
    drop(writer);
    assert_eq!(
        success(invoke(&input, &["--id", "record/0/incident-marker"])),
        serde_json::to_value(marker).unwrap()
    );
    assert_eq!(
        success(invoke(&input, &["--id", "record/1/circular-retention"])),
        retention
    );
}

#[test]
#[ignore = "requires TRANSMOG_CONTEXT_TEST_BINARY and TRANSMOG_CONTEXT_TEST_SHELL for standalone shell qualification"]
fn standalone_binary_and_shell_quoted_passwords() {
    let binary =
        std::env::var_os("TRANSMOG_CONTEXT_TEST_BINARY").expect("explicit standalone binary");
    let shell = std::env::var_os("TRANSMOG_CONTEXT_TEST_SHELL")
        .expect("explicit PowerShell 7 or POSIX shell");
    let root = tempfile::Builder::new()
        .prefix("context acceptance Café ")
        .tempdir()
        .unwrap();
    let name = if cfg!(windows) {
        "transmog-cli.exe"
    } else {
        "transmog-cli"
    };
    let copied = root.path().join(name);
    fs::copy(&binary, &copied).unwrap();
    let input = root.path().join("support trace.tmcap");
    fixture(
        &input,
        Some(PASSWORD),
        true,
        json!({"networkContext":network()}),
    );
    let profile = root.path().join("unused profile");
    let executable = |arguments: &[&str]| {
        let mut command = Command::new(&copied);
        command
            .current_dir(root.path())
            .args(arguments)
            .stdin(Stdio::null())
            .env("LOCALAPPDATA", &profile)
            .env("XDG_STATE_HOME", &profile);
        if cfg!(windows) {
            command.env(
                "PATH",
                Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32"),
            );
        } else {
            command.env("PATH", "/usr/bin:/bin");
        }
        command.output().unwrap()
    };
    let help = executable(&["context", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--no-prompt"));
    assert!(
        failure(executable(&[
            "context",
            "--input",
            "support trace.tmcap",
            "--no-prompt"
        ]))
        .contains("--no-prompt")
    );
    assert_eq!(
        success(executable(&[
            "context",
            "--input",
            "support trace.tmcap",
            "--password",
            PASSWORD,
            "--no-prompt",
            "--id",
            "record/0/networkContext"
        ])),
        network()
    );
    let (script_name, script, arguments) = quoted_script();
    fs::write(root.path().join(script_name), script).unwrap();
    let output = Command::new(shell)
        .args(arguments)
        .arg(script_name)
        .current_dir(root.path())
        .stdin(Stdio::null())
        .env("LOCALAPPDATA", &profile)
        .env("XDG_STATE_HOME", &profile)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PASSWORD));
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(root.path().join("quoted context.json")).unwrap()
        )
        .unwrap(),
        network()
    );
    assert!(!profile.exists());
    println!(
        "Standalone binary and native shell preserved password spaces, quotes, apostrophes, backslashes and Unicode; no desktop state or repository tooling used by the product."
    );
}

#[cfg(windows)]
fn quoted_script() -> (&'static str, String, Vec<&'static str>) {
    (
        "quoted.ps1",
        format!(
            "& './transmog-cli.exe' context --input 'support trace.tmcap' --password '{}' --no-prompt --id record/0/networkContext --output 'quoted context.json'\nexit $LASTEXITCODE\n",
            PASSWORD.replace('\'', "''")
        ),
        vec!["-NoLogo", "-NoProfile", "-NonInteractive", "-File"],
    )
}
#[cfg(not(windows))]
fn quoted_script() -> (&'static str, String, Vec<&'static str>) {
    (
        "quoted.sh",
        format!(
            "exec ./transmog-cli context --input 'support trace.tmcap' --password '{}' --no-prompt --id record/0/networkContext --output 'quoted context.json'\n",
            PASSWORD.replace('\'', "'\\''")
        ),
        Vec::<&str>::new(),
    )
}
