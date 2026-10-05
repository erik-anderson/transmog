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
    asset: PreviewAsset,
}

#[derive(Clone)]
pub(crate) struct PreviewAsset {
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) mime_type: &'static str,
}

impl PreviewService {
    pub(crate) fn new(executable: Option<PathBuf>) -> Self {
        Self {
            executable: executable.map(Arc::new),
            cache: Arc::new(Mutex::new(PreviewCache::default())),
        }
    }

    pub(crate) async fn create(
        &self,
        source: Vec<u8>,
        media_type: Option<&str>,
    ) -> Result<(String, &'static str), AppError> {
        if source.is_empty() || source.len() > transmog_preview_worker::MAX_SOURCE_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "image preview source exceeds sixteen MiB",
                false,
            ));
        }
        if media_type.is_some_and(is_svg_media_type) {
            validate_svg(&source)?;
            let handle = self.insert(source, "image/svg+xml")?;
            return Ok((handle, "image/svg+xml"));
        }
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
        let mut command = tokio::process::Command::new(executable.as_path());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
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
        let input = child.stdin.take().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Internal,
                "preview input pipe is unavailable",
                false,
            )
        })?;
        let output = child.stdout.take().ok_or_else(|| {
            AppError::new(
                ErrorCategory::Internal,
                "preview output pipe is unavailable",
                false,
            )
        })?;
        let operation = communicate(&mut child, input, output, &source);
        let png = if let Ok(result) = tokio::time::timeout(PREVIEW_DEADLINE, operation).await {
            result?
        } else {
            let _ = child.kill().await;
            return Err(AppError::new(
                ErrorCategory::Limit,
                "image preview exceeded its three-second deadline",
                false,
            ));
        };
        let handle = self.insert(png, "image/png")?;
        Ok((handle, "image/png"))
    }

    pub(crate) fn get(&self, handle: &str) -> Option<PreviewAsset> {
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
            .map(|entry| entry.asset.clone())
    }

    fn insert(&self, bytes: Vec<u8>, mime_type: &'static str) -> Result<String, AppError> {
        let mut random = [0_u8; 18];
        getrandom::fill(&mut random).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "preview handle generation failed",
                false,
            )
        })?;
        let handle = URL_SAFE_NO_PAD.encode(random);
        let length = bytes.len();
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
            cache.bytes = cache.bytes.saturating_sub(evicted.asset.bytes.len());
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
            asset: PreviewAsset {
                bytes: Arc::new(bytes),
                mime_type,
            },
        });
        Ok(handle)
    }
}

fn is_svg_media_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("image/svg+xml"))
}

fn validate_svg(source: &[u8]) -> Result<(), AppError> {
    let text = std::str::from_utf8(source).map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "SVG preview must be valid UTF-8",
            false,
        )
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let prefix = text
        .get(..text.len().min(64 * 1024))
        .unwrap_or(text)
        .to_ascii_lowercase();
    if !prefix.contains("<svg") {
        return Err(AppError::new(
            ErrorCategory::InvalidInput,
            "SVG preview does not contain an SVG root element",
            false,
        ));
    }
    Ok(())
}

async fn communicate(
    child: &mut tokio::process::Child,
    mut input: tokio::process::ChildStdin,
    mut output: tokio::process::ChildStdout,
    source: &[u8],
) -> Result<Vec<u8>, AppError> {
    let length = u32::try_from(source.len()).map_err(|_| {
        AppError::new(
            ErrorCategory::Limit,
            "image preview source is too large",
            false,
        )
    })?;
    input
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|_| preview_unavailable("preview worker input failed"))?;
    input
        .write_all(source)
        .await
        .map_err(|_| preview_unavailable("preview worker input failed"))?;
    input
        .shutdown()
        .await
        .map_err(|_| preview_unavailable("preview worker input failed"))?;
    let (status, bytes) = read_response(&mut output).await?;
    let exit = child
        .wait()
        .await
        .map_err(|_| preview_unavailable("preview worker status is unavailable"))?;
    if !exit.success() {
        return Err(preview_unavailable(
            "preview worker was terminated by its sandbox",
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
}

async fn read_response(
    output: &mut tokio::process::ChildStdout,
) -> Result<(u8, Vec<u8>), AppError> {
    let status = output
        .read_u8()
        .await
        .map_err(|_| preview_unavailable("preview worker exited without a result"))?;
    let length = output
        .read_u32()
        .await
        .map_err(|_| preview_unavailable("preview worker returned a truncated result"))?
        as usize;
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
    output
        .read_exact(&mut bytes)
        .await
        .map_err(|_| preview_unavailable("preview worker returned a truncated result"))?;
    Ok((status, bytes))
}

fn preview_unavailable(message: &'static str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_cache_evicts_oldest_handle_at_entry_limit() {
        let service = PreviewService::new(None);
        let mut handles = Vec::new();
        for value in 0..=MAX_PREVIEWS {
            handles.push(
                service
                    .insert(vec![u8::try_from(value).unwrap()], "image/png")
                    .unwrap(),
            );
        }
        assert!(service.get(&handles[0]).is_none());
        assert_eq!(
            service
                .get(handles.last().unwrap())
                .unwrap()
                .bytes
                .as_slice(),
            &[64]
        );
        assert!(service.get("../../not-a-handle").is_none());
    }

    #[tokio::test]
    async fn svg_uses_opaque_direct_image_asset_without_a_worker() {
        let service = PreviewService::new(None);
        let source = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><rect width="1" height="1"/></svg>"#.to_vec();
        let (handle, mime_type) = service
            .create(source.clone(), Some("image/svg+xml; charset=utf-8"))
            .await
            .unwrap();
        let asset = service.get(&handle).unwrap();
        assert_eq!(mime_type, "image/svg+xml");
        assert_eq!(asset.mime_type, "image/svg+xml");
        assert_eq!(asset.bytes.as_slice(), source);
    }

    #[tokio::test]
    async fn mislabeled_non_svg_is_rejected_without_starting_a_worker() {
        let service = PreviewService::new(None);
        let error = service
            .create(b"not an image".to_vec(), Some("image/svg+xml"))
            .await
            .unwrap_err();
        assert_eq!(error.category, ErrorCategory::InvalidInput);
    }
}
