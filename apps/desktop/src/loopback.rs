//! Read-only `AppContainer` inventory and narrowly scoped, elevated exemption changes.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
    process::{Command, Stdio},
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppContainer {
    display_name: String,
    description: String,
    package: String,
    name: String,
    sid: String,
    users: Vec<String>,
    exempt: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExemptionChange {
    pub(crate) enabled: bool,
    pub(crate) sids: Vec<String>,
}
impl ExemptionChange {
    fn validate(&self) -> Result<(), String> {
        if self.sids.is_empty() || self.sids.len() > 4096 {
            return Err("Choose between 1 and 4096 AppContainers.".into());
        }
        let mut unique = std::collections::HashSet::new();
        for sid in &self.sids {
            let valid = sid.strip_prefix("S-1-15-2-").is_some_and(|tail| {
                let parts = tail.split('-').collect::<Vec<_>>();
                !parts.is_empty()
                    && parts.len() <= 12
                    && parts.iter().all(|part| {
                        !part.is_empty()
                            && part.bytes().all(|byte| byte.is_ascii_digit())
                            && part
                                .parse::<u32>()
                                .is_ok_and(|number| number.to_string() == *part)
                    })
            });
            if sid.len() > 184 || !valid || !unique.insert(sid) {
                return Err("The selection contains an invalid or repeated AppContainer SID. Refresh and try again.".into());
            }
        }
        Ok(())
    }
}

fn powershell() -> Result<Command, String> {
    use std::os::windows::process::CommandExt as _;
    // The elevated path must come from machine-owned state, never a caller's
    // SystemRoot/PATH environment pointing to a different executable.
    let system = windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|key| key.get_string("SystemRoot"))
        .map(PathBuf::from)
        .map_err(|_| "The Windows system directory could not be located.".to_owned())?;
    if !system.is_absolute() {
        return Err("The Windows system directory is invalid.".to_owned());
    }
    let mut command = Command::new(system.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    command.creation_flags(0x0800_0000);
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
    Ok(command)
}
fn script(action: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);\nAdd-Type -TypeDefinition @'\n{}\n'@\n{action}",
        include_str!("../windows/loopback.cs")
    )
}

const ELEVATION_SCRIPT: &str = r"
$ErrorActionPreference='Stop'
try {
  $process = Start-Process -FilePath $env:TRANSMOG_LOOPBACK_EXECUTABLE -ArgumentList @('--set-loopback-exemptions', $env:TRANSMOG_LOOPBACK_DELTA, $env:TRANSMOG_LOOPBACK_DIGEST) -Verb RunAs -WindowStyle Hidden -PassThru -Wait
  exit $process.ExitCode
} catch {
  $loopbackException = $_.Exception
  while ($null -ne $loopbackException) { if ($loopbackException -is [ComponentModel.Win32Exception] -and $loopbackException.NativeErrorCode -eq 1223) { exit 3 }; $loopbackException = $loopbackException.InnerException }
  exit 2
}";

pub(crate) fn enumerate() -> Result<Vec<AppContainer>, String> {
    let mut child = powershell()?
        .arg(script(
            "ConvertTo-Json -InputObject @([Transmog.Loopback]::Read()) -Depth 5 -Compress",
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "Windows AppContainer enumeration could not start.".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Windows inventory output is unavailable.")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Windows inventory diagnostics are unavailable.")?;
    let read = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65_537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(45) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = read.join().ok().and_then(Result::ok).unwrap_or_default();
    let _ = errors.join();
    if status.is_none_or(|status| !status.success()) || bytes.len() > 16 * 1024 * 1024 {
        return Err("Windows could not enumerate AppContainers and loopback exemptions. Check that the Windows Firewall service is running, then refresh.".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        "Windows returned an unreadable AppContainer inventory. Refresh and try again.".into()
    })
}

/// Launch the current executable with only a validated SID delta in a temporary
/// data file. The elevated helper only reads it; no temporary code or elevated
/// result-file writes are involved. A file avoids Windows' command-line limit
/// when the user selects hundreds of `AppContainers`.
pub(crate) fn request_change(change: &ExemptionChange) -> Result<Vec<AppContainer>, String> {
    change.validate()?;
    let executable = std::env::current_exe()
        .map_err(|_| "The Transmog executable could not be located.".to_owned())?;
    let mut input = tempfile::Builder::new()
        .prefix(".transmog-loopback-")
        .suffix(".json")
        .tempfile()
        .map_err(|_| "The exemption selection could not be prepared.".to_owned())?;
    let json = serde_json::to_vec(change)
        .map_err(|_| "The exemption selection could not be encoded.".to_owned())?;
    input
        .write_all(&json)
        .and_then(|()| input.as_file().sync_all())
        .map_err(|_| "The exemption selection could not be prepared.".to_owned())?;
    let encoded = STANDARD.encode(
        input
            .path()
            .to_str()
            .ok_or("The temporary selection path could not be encoded.")?,
    );
    let output = powershell()?
        .arg(ELEVATION_SCRIPT)
        .env("TRANSMOG_LOOPBACK_EXECUTABLE", executable)
        .env("TRANSMOG_LOOPBACK_DELTA", encoded)
        .env(
            "TRANSMOG_LOOPBACK_DIGEST",
            hex::encode(sha2::Sha256::digest(&json)),
        )
        .output()
        .map_err(|_| "Windows approval could not be requested. Try again.".to_owned())?;
    match output.status.code() {
        Some(0) => enumerate(),
        Some(3) => Err("Windows approval was canceled. No loopback exemptions were changed.".into()),
        _ => Err("Windows could not apply the loopback exemptions. Refresh to inspect the current state, then try again with administrator approval.".into()),
    }
}

pub(crate) fn maintenance_exit_code() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_none_or(|argument| argument != "--set-loopback-exemptions")
    {
        return None;
    }
    let result = (|| {
        if arguments.len() != 3 {
            return Err(());
        }
        let change = read_change(
            arguments[1].to_str().ok_or(())?,
            arguments[2].to_str().ok_or(())?,
        )?;
        let json = serde_json::to_string(&change).map_err(|_| ())?;
        let mut child = powershell().map_err(|_| ())?.arg(script("$delta=[Console]::In.ReadToEnd() | ConvertFrom-Json;[Transmog.Loopback]::Apply([string[]]@($delta.sids),[bool]$delta.enabled)"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| ())?;
        child
            .stdin
            .take()
            .ok_or(())?
            .write_all(json.as_bytes())
            .map_err(|_| ())?;
        let output = child.wait_with_output().map_err(|_| ())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(())
        }
    })();
    Some(if result.is_ok() { 0 } else { 2 })
}

fn read_change(encoded: &str, digest: &str) -> Result<ExemptionChange, ()> {
    if encoded.len() > 8192
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(());
    }
    let path = PathBuf::from(
        String::from_utf8(STANDARD.decode(encoded).map_err(|_| ())?).map_err(|_| ())?,
    );
    if !path.is_absolute()
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(".transmog-loopback-")
                    && std::path::Path::new(name)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            })
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| ())?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    // Bind the helper to the exact selection prepared before Windows approval,
    // even if another process edits or replaces the temporary data file.
    if bytes.len() > 1_048_576
        || !hex::encode(sha2::Sha256::digest(&bytes)).eq_ignore_ascii_case(digest)
    {
        return Err(());
    }
    let change: ExemptionChange = serde_json::from_slice(&bytes).map_err(|_| ())?;
    change.validate().map_err(|_| ())?;
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_selections_are_bound_to_the_prepared_payload_without_native_writes() {
        let mut input = tempfile::Builder::new()
            .prefix(".transmog-loopback-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        let change=ExemptionChange {enabled:true,sids:(1000..1512).map(|index|format!("S-1-15-2-{index}-123456789-234567890-345678901-456789012-567890123-678901234")).collect()};
        let bytes = serde_json::to_vec(&change).unwrap();
        assert!(bytes.len() > 32_767);
        input.write_all(&bytes).unwrap();
        input.flush().unwrap();
        let encoded = STANDARD.encode(input.path().to_str().unwrap());
        let digest = hex::encode(sha2::Sha256::digest(&bytes));
        assert_eq!(read_change(&encoded, &digest).unwrap().sids.len(), 512);
        let edited = serde_json::to_vec(&ExemptionChange {
            enabled: false,
            ..change
        })
        .unwrap();
        std::fs::write(input.path(), &edited).unwrap();
        assert!(read_change(&encoded, &digest).is_err());
        assert!(read_change(&encoded, "invalid").is_err());
        assert!(read_change("not a path", &digest).is_err());
        assert!(
            !read_change(&encoded, &hex::encode(sha2::Sha256::digest(&edited)))
                .unwrap()
                .enabled
        );
    }
    #[test]
    fn approval_reports_cancel_and_failures_without_starting_an_elevated_process() {
        for (stub, expected) in [
            (
                "function Start-Process { throw [ComponentModel.Win32Exception]::new(1223) }",
                3,
            ),
            (
                "function Start-Process { [pscustomobject]@{ExitCode=0} }",
                0,
            ),
            (
                "function Start-Process { [pscustomobject]@{ExitCode=2} }",
                2,
            ),
            (
                "function Start-Process { throw 'Could not start helper' }",
                2,
            ),
        ] {
            let output = powershell()
                .unwrap()
                .arg(format!("{stub}\n{ELEVATION_SCRIPT}"))
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    #[test]
    fn elevated_input_accepts_only_a_bounded_canonical_appcontainer_delta() {
        let valid = "S-1-15-2-123-456-0";
        assert!(
            ExemptionChange {
                enabled: true,
                sids: vec![valid.into()]
            }
            .validate()
            .is_ok()
        );
        for sid in [
            "S-1-5-18",
            "S-1-15-2-",
            "S-1-15-2-01",
            "S-1-15-2-4294967296",
            "S-1-15-2-1;whoami",
            "S-1-15-2-1\n",
        ] {
            assert!(
                ExemptionChange {
                    enabled: false,
                    sids: vec![sid.into()]
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ExemptionChange {
                enabled: true,
                sids: vec![valid.into(); 2]
            }
            .validate()
            .is_err()
        );
        assert!(
            ExemptionChange {
                enabled: true,
                sids: vec![]
            }
            .validate()
            .is_err()
        );
        assert!(
            ExemptionChange {
                enabled: true,
                sids: (0..4097).map(|index| format!("S-1-15-2-{index}")).collect()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<ExemptionChange>(
                r#"{"enabled":true,"sids":["S-1-15-2-1"],"script":"unexpected"}"#
            )
            .is_err()
        );
    }
}
