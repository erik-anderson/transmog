//! Static help is usable through the shipped CLI without granting traffic access.
use std::{
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
fn script_help_is_offline_and_validated_at_the_cli_boundary() {
    let root = tempfile::tempdir().unwrap();
    for (input, expected) in [
        (r#"{"topic":"scripts","section":"examples"}"#, true),
        (r#"{"topic":"scripts","section":"unknown"}"#, false),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
            .args(["mcp", "call", "transmog_help"])
            .env_remove("TRANSMOG_ACCESS_KEY")
            .env("LOCALAPPDATA", root.path())
            .env("XDG_STATE_HOME", root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if expected {
            let help: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(help["apiRevision"], 3);
            assert_eq!(help["sections"][0]["id"], "examples");
            assert!(help["mcpSupport"].as_str().unwrap().contains("cannot"));
        }
    }
    assert!(
        !root.path().join("Transmog").exists(),
        "Static help started desktop state"
    );
}
