//! Owned non-exportable Microsoft software-key fixture; never changes root trust.
use std::{os::windows::process::CommandExt as _, process::Command};
pub struct WindowsFixture {
    thumbprint: String,
    issuer_thumbprint: String,
    pub id: String,
}
impl WindowsFixture {
    pub fn new(algorithm: &str) -> Self {
        let output = fixture_command("Create", None, Some(algorithm));
        // This helper emits only safe public fields; no JSON dependency is
        // needed just to read its fixture response in the TLS crate.
        let field = |name: &str| {
            let prefix = format!("\"{name}\":\"");
            output
                .split(&prefix)
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_owned()
        };
        Self {
            thumbprint: field("thumbprint"),
            issuer_thumbprint: field("issuerThumbprint"),
            id: field("id"),
        }
    }
    pub fn change(&self, action: &str) {
        fixture_command(action, Some(&self.thumbprint), None);
    }
}
impl Drop for WindowsFixture {
    fn drop(&mut self) {
        // Do not panic during test unwinding, but report any cleanup failure.
        let result = command("Cleanup", Some(&self.thumbprint), None)
            .args(["-IssuerThumbprint", &self.issuer_thumbprint])
            .output();
        assert_cleanup(result);
    }
}
fn assert_cleanup(result: std::io::Result<std::process::Output>) {
    match result {
        Ok(output) if output.status.success() => (),
        other => {
            eprintln!("Windows client certificate fixture cleanup failed: {other:?}");
            assert!(std::thread::panicking(), "fixture cleanup failed");
        }
    }
}
fn command(action: &str, thumbprint: Option<&str>, algorithm: Option<&str>) -> Command {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/client-certificate-test-fixture.ps1");
    let mut command = Command::new("powershell.exe");
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the fixture helper.
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(path)
        .args(["-Action", action]);
    if let Some(value) = thumbprint {
        command.args(["-Thumbprint", value]);
    }
    if let Some(value) = algorithm {
        command.args(["-Algorithm", value]);
    }
    command
}
fn fixture_command(action: &str, thumbprint: Option<&str>, algorithm: Option<&str>) -> String {
    let output = command(action, thumbprint, algorithm).output().unwrap();
    assert!(
        output.status.success(),
        "fixture {action} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
