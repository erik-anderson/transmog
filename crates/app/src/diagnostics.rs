use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write as _,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{AppError, ErrorCategory, ProductState};

const MAX_EVENTS: usize = 256;
const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Severity for one structured operational event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticLevel {
    /// Informational lifecycle event.
    Info,
    /// Recoverable condition requiring attention.
    Warning,
    /// Operation failure.
    Error,
}

/// One bounded and redacted structured operational event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Event severity.
    pub level: DiagnosticLevel,
    /// Stable bounded component name.
    pub component: String,
    /// Stable bounded event code.
    pub code: String,
    /// Redacted operator-safe detail.
    pub message: String,
}

/// Platform facts supplied by the native host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    /// Operating-system description.
    pub operating_system: String,
    /// Process architecture.
    pub architecture: String,
    /// Native `WebView` runtime version, when available.
    pub webview_version: Option<String>,
}

/// Bounded diagnostics suitable for display and copying.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    /// Product version.
    pub application_version: String,
    /// Rust package versions important to support.
    pub dependencies: Vec<String>,
    /// Native runtime facts.
    pub runtime: RuntimeDiagnostics,
    /// Persisted state schema.
    pub product_state_schema: u32,
    /// Redacted newest-last events.
    pub events: Vec<DiagnosticEvent>,
    /// Explicit statement of omitted sensitive evidence.
    pub privacy_notice: String,
}

/// Create-new support-bundle request.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleRequest {
    /// Destination ZIP path.
    pub destination: PathBuf,
    /// Platform facts collected by the native host.
    pub runtime: RuntimeDiagnostics,
    /// Explicit opt-in to include recent artifact paths, subject to persisted privacy policy.
    #[serde(default)]
    pub include_recent_paths: bool,
}

/// Completed support-bundle summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportBundleResult {
    /// Created archive path.
    pub destination: PathBuf,
    /// Final archive bytes.
    pub bytes: u64,
    /// Whether opted-in recent paths were included.
    pub included_recent_paths: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DiagnosticLog {
    events: Arc<Mutex<VecDeque<DiagnosticEvent>>>,
    path: Option<PathBuf>,
}

impl DiagnosticLog {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            events: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_EVENTS))),
            path,
        }
    }

    pub(crate) fn record(
        &self,
        level: DiagnosticLevel,
        component: &str,
        code: &str,
        message: &str,
    ) {
        let event = DiagnosticEvent {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| {
                    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
                }),
            level,
            component: bounded(component, 48),
            code: bounded(code, 64),
            message: redact(message, false),
        };
        let mut events = self.events.lock().unwrap();
        if events.len() == MAX_EVENTS {
            events.pop_front();
        }
        events.push_back(event.clone());
        drop(events);
        if let Some(path) = &self.path {
            let _ = append_bounded(path, &event);
        }
    }

    pub(crate) fn report(
        &self,
        mut runtime: RuntimeDiagnostics,
        state: &ProductState,
    ) -> DiagnosticsReport {
        runtime.operating_system = bounded(&runtime.operating_system, 128);
        runtime.architecture = bounded(&runtime.architecture, 32);
        runtime.webview_version = runtime.webview_version.map(|value| bounded(&value, 128));
        DiagnosticsReport {
            application_version: crate::application_version().to_owned(),
            dependencies: vec![
                format!("transmog-app {}", env!("CARGO_PKG_VERSION")),
                "Tauri 2.12.1".to_owned(),
                "Microsoft WebUI 0.0.30".to_owned(),
                "BoringSSL via boring 5.2.0".to_owned(),
            ],
            runtime,
            product_state_schema: state.schema_version,
            events: self.events.lock().unwrap().iter().cloned().collect(),
            privacy_notice: "Bodies, header values, credentials, private keys, and paths are excluded by default.".to_owned(),
        }
    }
}

pub(crate) async fn create_support_bundle(
    request: SupportBundleRequest,
    report: DiagnosticsReport,
    state: ProductState,
) -> Result<SupportBundleResult, AppError> {
    tokio::task::spawn_blocking(move || create_support_bundle_blocking(&request, &report, &state))
        .await
        .map_err(|_| AppError::new(ErrorCategory::Internal, "support bundle task failed", true))?
}

fn create_support_bundle_blocking(
    request: &SupportBundleRequest,
    report: &DiagnosticsReport,
    state: &ProductState,
) -> Result<SupportBundleResult, AppError> {
    let include_paths =
        request.include_recent_paths && state.privacy.include_paths_in_support_bundles;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&request.destination)
        .map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "support bundle destination must be new and writable",
                true,
            )
        })?;
    let result = (|| -> Result<u64, Box<dyn std::error::Error>> {
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        write_json(&mut zip, "diagnostics.json", report, options)?;
        let state_summary = serde_json::json!({
            "schemaVersion": state.schema_version,
            "preferences": state.preferences,
            "privacy": state.privacy,
            "recentArtifacts": if include_paths { serde_json::to_value(&state.recent_artifacts)? } else { serde_json::json!([]) },
            "exclusions": ["captured bodies", "HTTP header values", "credentials", "CA private keys"]
        });
        write_json(
            &mut zip,
            "product-state-summary.json",
            &state_summary,
            options,
        )?;
        let mut output = zip.finish()?;
        output.flush()?;
        output.sync_all()?;
        Ok(output.metadata()?.len())
    })();
    if let Ok(bytes) = result {
        Ok(SupportBundleResult {
            destination: request.destination.clone(),
            bytes,
            included_recent_paths: include_paths,
        })
    } else {
        let _ = fs::remove_file(&request.destination);
        Err(AppError::new(
            ErrorCategory::Unavailable,
            "support bundle generation failed",
            true,
        ))
    }
}

fn write_json<W: std::io::Write + std::io::Seek, T: Serialize>(
    zip: &mut ZipWriter<W>,
    name: &str,
    value: &T,
    options: SimpleFileOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    zip.start_file(name, options)?;
    zip.write_all(&serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn append_bounded(path: &PathBuf, event: &DiagnosticEvent) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_vec(event).map_err(std::io::Error::other)?;
    line.push(b'\n');
    if fs::metadata(path)
        .is_ok_and(|metadata| metadata.len().saturating_add(line.len() as u64) > MAX_LOG_BYTES)
    {
        let rotated = path.with_extension("jsonl.1");
        let _ = fs::remove_file(&rotated);
        fs::rename(path, rotated)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)?;
    file.flush()
}

fn bounded(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn redact(value: &str, include_paths: bool) -> String {
    let mut output = String::new();
    for line in value.lines().take(16) {
        if !output.is_empty() {
            output.push('\n');
        }
        let lower = line.to_ascii_lowercase();
        if [
            "authorization",
            "proxy-authorization",
            "cookie",
            "private key",
            "password",
            "bearer ",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            output.push_str("[REDACTED SENSITIVE LINE]");
        } else if !include_paths
            && (line.contains(":\\") || line.starts_with('/') || line.starts_with("\\\\"))
        {
            output.push_str("[REDACTED PATH]");
        } else {
            output.extend(line.chars().take(512));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn temp(label: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "transmog-{label}-{}.{}",
            std::process::id(),
            extension
        ))
    }

    #[test]
    fn log_is_bounded_and_redacts_credentials_keys_and_paths() {
        let log = DiagnosticLog::new(None);
        for index in 0..300 {
            log.record(
                DiagnosticLevel::Info,
                "test",
                "event",
                &format!("{index} Authorization: Bearer secret C:\\secret.key"),
            );
        }
        let report = log.report(
            RuntimeDiagnostics {
                operating_system: "Windows".into(),
                architecture: "x86_64".into(),
                webview_version: None,
            },
            &ProductState::default(),
        );
        assert_eq!(report.events.len(), MAX_EVENTS);
        let text = serde_json::to_string(&report).unwrap();
        assert!(!text.contains("secret"));
        assert!(!text.contains("Bearer"));
    }

    #[tokio::test]
    async fn support_bundle_is_create_new_and_private_by_default() {
        let path = temp("support", "zip");
        let _ = fs::remove_file(&path);
        let log = DiagnosticLog::new(None);
        log.record(DiagnosticLevel::Warning, "test", "safe", "safe message");
        let mut state = ProductState::default();
        state.privacy.remember_recent_artifacts = true;
        state.recent_artifacts.push(crate::RecentArtifact {
            path: PathBuf::from("C:\\private\\capture.tmcap"),
            kind: crate::ArtifactKind::NativeCapture,
        });
        let runtime = RuntimeDiagnostics {
            operating_system: "Windows".into(),
            architecture: "x86_64".into(),
            webview_version: Some("1.2.3".into()),
        };
        let result = create_support_bundle(
            SupportBundleRequest {
                destination: path.clone(),
                runtime: runtime.clone(),
                include_recent_paths: true,
            },
            log.report(runtime, &state),
            state,
        )
        .await
        .unwrap();
        assert!(!result.included_recent_paths);
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut text = String::new();
        archive
            .by_name("product-state-summary.json")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(!text.contains("C:\\private\\capture.tmcap"));
        assert!(
            create_support_bundle_blocking(
                &SupportBundleRequest {
                    destination: path.clone(),
                    runtime: RuntimeDiagnostics {
                        operating_system: String::new(),
                        architecture: String::new(),
                        webview_version: None
                    },
                    include_recent_paths: false
                },
                &log.report(
                    RuntimeDiagnostics {
                        operating_system: String::new(),
                        architecture: String::new(),
                        webview_version: None
                    },
                    &ProductState::default()
                ),
                &ProductState::default()
            )
            .is_err()
        );
        fs::remove_file(path).unwrap();
    }
}
