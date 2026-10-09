//! Opt-in machine network configuration; no host settings are changed.
use serde::{Deserialize, Serialize};
use std::{
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command};
const MAX_OUTPUT: u64 = 1024 * 1024;
/// Original support context, associated with the machine where it was collected.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkContext {
    /// Application and version that collected this context.
    #[serde(default)]
    pub collector: String,
    /// Capture start or trace-save collection purpose.
    #[serde(default)]
    pub purpose: String,
    /// Collector machine name when available; collected only after opt-in.
    #[serde(default)]
    pub computer_name: Option<String>,
    /// Operating system that produced the output.
    pub platform: String,
    /// UTC collection time in Unix milliseconds.
    pub collected_at: u64,
    /// Human-readable command names; never an executable input.
    pub command: String,
    /// Bounded UTF-8 text produced by the platform tools.
    pub output: String,
    /// Unavailable tools, nonzero exits, or bounded-read failures.
    pub notes: Vec<String>,
}
/// Collects configuration only when explicitly requested by the caller.
pub async fn collect() -> NetworkContext {
    collect_for("capture-start").await
}
/// Collects context with the caller's collection purpose.
pub async fn collect_for(purpose: &str) -> NetworkContext {
    let mut context = NetworkContext {
        collector: format!("Transmog {}", env!("CARGO_PKG_VERSION")),
        purpose: purpose.into(),
        computer_name: std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .map(|name| name.chars().take(255).collect()),
        platform: std::env::consts::OS.into(),
        collected_at: u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX),
        command: String::new(),
        output: String::new(),
        notes: vec![],
    };
    #[cfg(windows)]
    let tools: Vec<(&str, std::ffi::OsString, Vec<&str>)> = vec![{
        let system = std::env::var_os("SystemRoot").map_or_else(
            || std::path::PathBuf::from("C:\\Windows"),
            std::path::PathBuf::from,
        );
        // PowerShell decodes native console text using its normal OEM environment,
        // then the fixed wrapper writes Unicode output as UTF-8 to our pipe.
        let script = "$text = (& (Join-Path $env:SystemRoot 'System32\\ipconfig.exe') /all | Out-String -Width 4096); $code = $LASTEXITCODE; [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); [Console]::Write($text); exit $code";
        (
            "ipconfig /all",
            system
                .join("System32/WindowsPowerShell/v1.0/powershell.exe")
                .into_os_string(),
            vec![
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ],
        )
    }];
    #[cfg(target_os = "macos")]
    let tools: Vec<(&str, std::ffi::OsString, Vec<&str>)> = vec![
        ("ifconfig -a", "/sbin/ifconfig".into(), vec!["-a"]),
        ("scutil --dns", "/usr/sbin/scutil".into(), vec!["--dns"]),
        ("netstat -rn", "/usr/sbin/netstat".into(), vec!["-rn"]),
    ];
    #[cfg(not(any(windows, target_os = "macos")))]
    let tools: Vec<(&str, std::ffi::OsString, Vec<&str>)> = vec![
        ("ip address show", "ip".into(), vec!["address", "show"]),
        (
            "ip route show table all",
            "ip".into(),
            vec!["route", "show", "table", "all"],
        ),
        ("resolvectl status", "resolvectl".into(), vec!["status"]),
    ];
    for (label, program, args) in tools {
        if !context.command.is_empty() {
            context.command.push_str("; ");
        }
        context.command.push_str(label);
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        match bounded_output(command).await {
            Ok((stdout, stderr, success)) => {
                if !context.output.is_empty() {
                    context.output.push_str("\n\n");
                }
                context.output.push_str(label);
                context.output.push(char::from(10));
                context.output.push_str(&String::from_utf8_lossy(&stdout));
                if !success {
                    context.notes.push(format!(
                        "{label} exited unsuccessfully: {}",
                        String::from_utf8_lossy(&stderr)
                    ));
                }
            }
            Err(error) => context.notes.push(format!("{label}: {error}")),
        }
    }
    context
}
async fn bounded_output(mut command: Command) -> std::io::Result<(Vec<u8>, Vec<u8>, bool)> {
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("Output pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("Error pipe unavailable"))?;
    let operation = async {
        let (out, err, status) = tokio::try_join!(
            read_limited(stdout, MAX_OUTPUT),
            read_limited(stderr, 8192),
            child.wait()
        )?;
        Ok((out, err, status.success()))
    };
    let result = tokio::time::timeout(Duration::from_secs(10), operation).await;
    match result {
        Ok(Ok(result)) => Ok(result),
        failure => {
            let _ = child.kill().await;
            match failure {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Network configuration collection timed out",
                )),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}
async fn read_limited(
    mut source: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = vec![0u8; 8192];
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() as u64 + count as u64 > limit {
            return Err(std::io::Error::other(
                "Network configuration output exceeded its limit",
            ));
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn output_limits_do_not_hide_incomplete_context() {
        let mut exact = std::io::Cursor::new(b"abc".to_vec());
        assert_eq!(read_limited(&mut exact, 3).await.unwrap(), b"abc");
        let mut excessive = std::io::Cursor::new(vec![0u8; 8193]);
        assert!(
            read_limited(&mut excessive, 8192)
                .await
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_wrapper_preserves_unicode_without_collecting_real_machine_data() {
        let script = "$text = 'Unicode fixture: '+[char]0x00e9+[char]0x4e2d; [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false); [Console]::Write($text)";
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000)
            .kill_on_drop(true);
        let (out, err, success) = bounded_output(command).await.unwrap();
        assert!(success, "{}", String::from_utf8_lossy(&err));
        assert_eq!(String::from_utf8(out).unwrap(), "Unicode fixture: é中");
    }
}
