use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{AppError, ErrorCategory};

const MAX_PREVIEWS: usize = 64;
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const PREVIEW_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(crate) struct PreviewService {
    executable: Option<Arc<PathBuf>>,
    cache: Arc<Mutex<PreviewCache>>,
}

#[derive(Default)]
struct PreviewCache {
    entries: VecDeque<PreviewEntry>,
    bytes: usize,
}

struct PreviewEntry {
    handle: String,
    png: Arc<Vec<u8>>,
}

impl PreviewService {
    pub(crate) fn new(executable: Option<PathBuf>) -> Self {
        Self {
            executable: executable.map(Arc::new),
            cache: Arc::new(Mutex::new(PreviewCache::default())),
        }
    }

    pub(crate) async fn create(&self, source: Vec<u8>) -> Result<String, AppError> {
        let executable = self.executable.as_deref().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "packaged preview worker is unavailable",
                true,
            )
        })?;
        if !executable.is_absolute() || !executable.is_file() {
            return Err(AppError::new(
                ErrorCategory::Unavailable,
                "packaged preview worker is unavailable",
                true,
            ));
        }
        if source.is_empty() || source.len() > transmog_preview_worker::MAX_SOURCE_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "image preview source exceeds sixteen MiB",
                false,
            ));
        }
        let mut command = tokio::process::Command::new(executable.as_path());
        command
            .arg("--sandbox-bootstrap")
            .current_dir(executable.parent().ok_or_else(|| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker path is invalid",
                    false,
                )
            })?)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "preview worker could not start",
                true,
            )
        })?;
        let mut input = child.stdin.take().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Internal,
                "preview input pipe is unavailable",
                false,
            )
        })?;
        let mut output = child.stdout.take().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Internal,
                "preview output pipe is unavailable",
                false,
            )
        })?;
        let operation = async {
            let length = u32::try_from(source.len()).map_err(|_| {
                AppError::new(
                    ErrorCategory::Limit,
                    "image preview source is too large",
                    false,
                )
            })?;
            input.write_all(&length.to_be_bytes()).await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker input failed",
                    true,
                )
            })?;
            input.write_all(&source).await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker input failed",
                    true,
                )
            })?;
            input.shutdown().await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker input failed",
                    true,
                )
            })?;
            let status = output.read_u8().await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker exited without a result",
                    true,
                )
            })?;
            let length = output.read_u32().await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker returned a truncated result",
                    true,
                )
            })? as usize;
            let maximum = if status == 0 {
                transmog_preview_worker::MAX_OUTPUT_BYTES
            } else {
                512
            };
            if length == 0 || length > maximum {
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "preview worker response exceeds its limit",
                    false,
                ));
            }
            let mut bytes = vec![0_u8; length];
            output.read_exact(&mut bytes).await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker returned a truncated result",
                    true,
                )
            })?;
            let exit = child.wait().await.map_err(|_| {
                AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker status is unavailable",
                    true,
                )
            })?;
            if !exit.success() {
                return Err(AppError::new(
                    ErrorCategory::Unavailable,
                    "preview worker was terminated by its sandbox",
                    true,
                ));
            }
            if status != 0 {
                return Err(AppError::new(
                    ErrorCategory::InvalidInput,
                    String::from_utf8_lossy(&bytes).into_owned(),
                    false,
                ));
            }
            Ok(bytes)
        };
        let png = match tokio::time::timeout(PREVIEW_DEADLINE, operation).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "image preview exceeded its three-second deadline",
                    false,
                ));
            }
        };
        self.insert(png)
    }

    pub(crate) fn get(&self, handle: &str) -> Option<Arc<Vec<u8>>> {
        if handle.len() != 24
            || !handle
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return None;
        }
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| Arc::clone(&entry.png))
    }

    fn insert(&self, png: Vec<u8>) -> Result<String, AppError> {
        let mut random = [0_u8; 18];
        getrandom::fill(&mut random).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "preview handle generation failed",
                false,
            )
        })?;
        let handle = URL_SAFE_NO_PAD.encode(random);
        let length = png.len();
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while cache.entries.len() >= MAX_PREVIEWS
            || cache.bytes.saturating_add(length) > MAX_CACHE_BYTES
        {
            let Some(evicted) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes = cache.bytes.saturating_sub(evicted.png.len());
        }
        if length > MAX_CACHE_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "normalized preview exceeds the cache limit",
                false,
            ));
        }
        cache.bytes = cache.bytes.saturating_add(length);
        cache.entries.push_back(PreviewEntry {
            handle: handle.clone(),
            png: Arc::new(png),
        });
        Ok(handle)
    }
}
