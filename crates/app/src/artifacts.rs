use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use transmog_capture::{
    CaptureExporter, CaptureLimits, CapturePolicy, CaptureRecordKind, CaptureWriter,
    JsonLinesExporter, recover,
};
use transmog_saz::{SazExporter, SazLimits, SazMode};
use transmog_session::{ApplicationSessionService, CaptureStart, CaptureStatus, SealedCapture};

use crate::{AppError, ErrorCategory};

const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000_000;

/// Finite native capture start settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStartRequest {
    /// Create-new native artifact path.
    pub path: PathBuf,
    /// Maximum complete file bytes.
    pub max_file_bytes: u64,
    /// Whether redacted bounded body samples are retained.
    #[serde(default)]
    pub retain_body_samples: bool,
}

/// Presentation-safe native capture state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum CaptureReadModel {
    /// No capture is active.
    Idle,
    /// Capture is streaming to a create-new path.
    Active {
        /// Destination path as selected by the caller.
        path: PathBuf,
        /// Bytes durably written as of the last event.
        bytes_written: u64,
    },
    /// Capture sealed cleanly.
    Sealed {
        /// Artifact path.
        path: PathBuf,
        /// Final bytes.
        bytes_written: u64,
    },
    /// Capture stopped after a recoverable-prefix failure.
    Failed {
        /// Artifact path, if writing had started.
        path: Option<PathBuf>,
        /// Operator-safe reason.
        message: String,
    },
    /// Capture worker has shut down.
    Shutdown,
}

/// Bounded native import settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    /// Native capture path.
    pub path: PathBuf,
    /// Explicit maximum input bytes, capped by the application maximum.
    pub max_file_bytes: u64,
}

/// Deterministic imported capture summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSummaryView {
    /// Total valid records.
    pub records: usize,
    /// Distinct exchanges.
    pub exchanges: usize,
    /// Explicit loss markers.
    pub loss_markers: usize,
    /// Retained body bytes.
    pub retained_body_bytes: u64,
    /// Whether a consistent seal was present.
    pub sealed: bool,
    /// Whether a partial final frame was safely ignored.
    pub truncated_tail: bool,
    /// Bytes in the valid native prefix.
    pub valid_bytes: u64,
}

/// Supported derived capture format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    /// Sealed native `TMCap` snapshot of every complete source record.
    Native,
    /// Streaming newline-delimited JSON.
    JsonLines,
    /// Conventional compatibility SAZ.
    SazStrict,
    /// SAZ plus a namespaced fidelity manifest.
    SazExtended,
}

/// Bounded create-new export request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// Authoritative native source.
    pub source: PathBuf,
    /// Create-new destination.
    pub destination: PathBuf,
    /// Derived format.
    pub format: ExportFormat,
    /// Explicit maximum source bytes.
    pub max_source_bytes: u64,
}

/// Export report and fidelity disclosure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// Destination path.
    pub destination: PathBuf,
    /// Records or sessions exported.
    pub records: usize,
    /// Output bytes.
    pub bytes: u64,
    /// Whether the source was sealed.
    pub source_sealed: bool,
    /// Whether recovery ignored an interrupted tail.
    pub source_truncated_tail: bool,
    /// Human-readable format fidelity disclosure.
    pub fidelity: String,
}

pub(crate) async fn start_capture(
    service: &ApplicationSessionService,
    request: CaptureStartRequest,
) -> Result<CaptureReadModel, AppError> {
    if request.max_file_bytes <= 1024 || request.max_file_bytes > MAX_IMPORT_BYTES {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "capture quota must be between one KiB and four GiB",
            false,
        ));
    }
    let mut policy = CapturePolicy::default();
    policy.retain_body_samples = request.retain_body_samples;
    service
        .start_capture(CaptureStart {
            path: request.path,
            limits: CaptureLimits {
                max_file_bytes: request.max_file_bytes,
                max_record_bytes: MAX_RECORD_BYTES,
                max_records: MAX_RECORDS,
            },
            policy,
        })
        .await
        .map_err(AppError::from)?;
    Ok(capture_status(service))
}

pub(crate) async fn stop_capture(
    service: &ApplicationSessionService,
) -> Result<CaptureReadModel, AppError> {
    let sealed = service.stop_capture().await.map_err(AppError::from)?;
    Ok(sealed_model(sealed))
}

pub(crate) fn capture_status(service: &ApplicationSessionService) -> CaptureReadModel {
    match service.capture().status() {
        CaptureStatus::Idle => CaptureReadModel::Idle,
        CaptureStatus::Active {
            path,
            bytes_written,
        } => CaptureReadModel::Active {
            path,
            bytes_written,
        },
        CaptureStatus::Sealed(sealed) => sealed_model(sealed),
        CaptureStatus::Failed(failure) => CaptureReadModel::Failed {
            path: failure.path,
            message: failure.message.chars().take(512).collect(),
        },
        CaptureStatus::Shutdown => CaptureReadModel::Shutdown,
    }
}

pub(crate) async fn import_capture(request: ImportRequest) -> Result<CaptureSummaryView, AppError> {
    tokio::task::spawn_blocking(move || {
        let capture = recover_path(&request.path, request.max_file_bytes)?;
        Ok(summary(&capture))
    })
    .await
    .map_err(|_| AppError::new(ErrorCategory::Internal, "capture import task failed", true))?
}

pub(crate) async fn export_capture(request: ExportRequest) -> Result<ExportResult, AppError> {
    tokio::task::spawn_blocking(move || export_blocking(&request))
        .await
        .map_err(|_| AppError::new(ErrorCategory::Internal, "capture export task failed", true))?
}

fn export_blocking(request: &ExportRequest) -> Result<ExportResult, AppError> {
    if request.source == request.destination {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "capture export destination must differ from its source",
            false,
        ));
    }
    let capture = recover_path(&request.source, request.max_source_bytes)?;
    let destination = request.destination.clone();
    let mut destination_created = false;
    let result = match request.format {
        ExportFormat::Native => create_new(&destination)
            .map_err(|error| error.to_string())
            .inspect(|_file| {
                destination_created = true;
            })
            .and_then(|file| export_native(file, &capture)),
        ExportFormat::JsonLines => create_new(&destination)
            .map_err(|error| error.to_string())
            .inspect(|_file| {
                destination_created = true;
            })
            .and_then(|file| export_json_lines(file, &capture)),
        ExportFormat::SazStrict | ExportFormat::SazExtended => {
            let mode = if request.format == ExportFormat::SazStrict {
                SazMode::Strict
            } else {
                SazMode::Extended
            };
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| error.to_string())
                .inspect(|_file| {
                    destination_created = true;
                })
                .and_then(|file| export_saz(file, mode, &capture))
        }
    };
    match result {
        Ok((records, bytes, fidelity)) => Ok(ExportResult {
            destination,
            records,
            bytes,
            source_sealed: capture.sealed,
            source_truncated_tail: capture.truncated_tail,
            fidelity,
        }),
        Err(error) => {
            if destination_created {
                let _ = std::fs::remove_file(&destination);
            }
            Err(AppError::new(
                ErrorCategory::Unavailable,
                format!("capture export failed: {error}"),
                true,
            ))
        }
    }
}

type ExportOutcome = Result<(usize, u64, String), String>;

fn export_native(file: File, capture: &transmog_capture::RecoveredCapture) -> ExportOutcome {
    let limits = CaptureLimits {
        max_file_bytes: MAX_IMPORT_BYTES,
        max_record_bytes: MAX_RECORD_BYTES,
        max_records: MAX_RECORDS,
    };
    let mut writer = CaptureWriter::new(file, limits).map_err(|error| error.to_string())?;
    let mut records = 0_usize;
    for record in capture
        .records
        .iter()
        .filter(|record| !matches!(record.kind, CaptureRecordKind::Seal { .. }))
    {
        writer.append(record).map_err(|error| error.to_string())?;
        records = records.saturating_add(1);
    }
    writer.seal().map_err(|error| error.to_string())?;
    Ok((
        records,
        writer.bytes_written(),
        "Native TMCap snapshot preserves every complete source record available at export time; an interrupted in-flight tail, if present, is omitted."
            .to_owned(),
    ))
}

fn export_json_lines(file: File, capture: &transmog_capture::RecoveredCapture) -> ExportOutcome {
    let report = JsonLinesExporter::new(file)
        .export(capture)
        .map_err(|error| error.to_string())?;
    Ok((
        report.records,
        report.bytes,
        "JSONL preserves every native record and streams sequentially.".to_owned(),
    ))
}

fn export_saz(
    file: File,
    mode: SazMode,
    capture: &transmog_capture::RecoveredCapture,
) -> ExportOutcome {
    let mut exporter =
        SazExporter::new(file, mode, SazLimits::default()).map_err(|error| error.to_string())?;
    let report = exporter
        .export(capture)
        .map_err(|error| error.to_string())?;
    let detail = exporter.report().unwrap_or_default();
    Ok((
        report.records,
        report.bytes,
        format!(
            "SAZ is finalized, not streaming; it omits non-HTTP-native evidence. skipped_incomplete={}, incomplete_bodies={}, extended_manifest={}",
            detail.skipped_incomplete,
            detail.incomplete_bodies,
            mode == SazMode::Extended
        ),
    ))
}

fn recover_path(
    path: &PathBuf,
    requested_max_bytes: u64,
) -> Result<transmog_capture::RecoveredCapture, AppError> {
    if requested_max_bytes == 0 || requested_max_bytes > MAX_IMPORT_BYTES {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "capture input bound must be between one byte and four GiB",
            false,
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "capture source is unavailable",
            false,
        )
    })?;
    if metadata.len() > requested_max_bytes {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "capture source exceeds the selected input bound",
            false,
        ));
    }
    let file = File::open(path).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "capture source is unreadable",
            false,
        )
    })?;
    recover(
        file,
        CaptureLimits {
            max_file_bytes: requested_max_bytes,
            max_record_bytes: MAX_RECORD_BYTES,
            max_records: MAX_RECORDS,
        },
    )
    .map_err(|error| {
        AppError::new(
            ErrorCategory::InvalidInput,
            format!("native capture is invalid: {error}"),
            false,
        )
    })
}

fn create_new(path: &PathBuf) -> Result<File, transmog_capture::CaptureError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(Into::into)
}

fn summary(capture: &transmog_capture::RecoveredCapture) -> CaptureSummaryView {
    let value = capture.summary();
    CaptureSummaryView {
        records: value.records,
        exchanges: value.exchanges,
        loss_markers: value.loss_markers,
        retained_body_bytes: value.retained_body_bytes,
        sealed: value.sealed,
        truncated_tail: value.truncated_tail,
        valid_bytes: capture.valid_bytes,
    }
}

fn sealed_model(sealed: SealedCapture) -> CaptureReadModel {
    CaptureReadModel::Sealed {
        path: sealed.path,
        bytes_written: sealed.bytes_written,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use transmog_capture::{CaptureRecord, CaptureRecordKind, CaptureWriter};

    use super::*;

    fn temp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("transmog-app-{label}-{}.tmcap", std::process::id()))
    }

    #[tokio::test]
    async fn partial_native_tail_recovers_and_exports_without_overwrite() {
        let source = temp("partial");
        let destination = temp("export");
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_file(&destination);
        let file = create_new(&source).unwrap();
        let mut writer = CaptureWriter::new(file, CaptureLimits::default()).unwrap();
        writer
            .append(&CaptureRecord {
                sequence: 1,
                exchange_id: 1,
                kind: CaptureRecordKind::Loss {
                    first_missing_sequence: 2,
                    count: 1,
                    reason: "test".to_owned(),
                },
            })
            .unwrap();
        drop(writer);
        OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(&[1, 2, 3])
            .unwrap();
        let imported = import_capture(ImportRequest {
            path: source.clone(),
            max_file_bytes: 1024 * 1024,
        })
        .await
        .unwrap();
        assert!(imported.truncated_tail);
        assert_eq!(imported.records, 1);
        let request = ExportRequest {
            source: source.clone(),
            destination: destination.clone(),
            format: ExportFormat::JsonLines,
            max_source_bytes: 1024 * 1024,
        };
        export_capture(request.clone()).await.unwrap();
        let first = std::fs::read(&destination).unwrap();
        assert!(export_capture(request).await.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), first);
        let native_destination = temp("native-export");
        let _ = std::fs::remove_file(&native_destination);
        let native = export_capture(ExportRequest {
            source: source.clone(),
            destination: native_destination.clone(),
            format: ExportFormat::Native,
            max_source_bytes: 1024 * 1024,
        })
        .await
        .unwrap();
        assert_eq!(native.records, 1);
        assert!(native.source_truncated_tail);
        let recovered = recover(
            File::open(&native_destination).unwrap(),
            CaptureLimits::default(),
        )
        .unwrap();
        assert!(recovered.sealed);
        assert!(!recovered.truncated_tail);
        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(destination);
        let _ = std::fs::remove_file(native_destination);
    }
}
