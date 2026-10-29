//! Immutable, automatically removed body files for replay evidence.
use crate::{
    AppError, BodyStore, BodyStoreConfig, ErrorCategory, body_store::SavedBodySource,
    composer_response::ComposerResponse, inspector,
};
use std::{
    io::{self, Read, Seek as _, Write as _},
    sync::Arc,
};
use transmog_session::SessionSnapshot;

#[derive(Debug)]
pub(crate) struct SnapshotFile {
    file: tempfile::NamedTempFile,
    length: u64,
}
impl SnapshotFile {
    pub(crate) fn reopen(&self) -> io::Result<std::fs::File> {
        self.file.reopen()
    }
}
impl SavedBodySource for SnapshotFile {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        self.open_at(0)
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        let mut file = self.file.reopen()?;
        file.seek(io::SeekFrom::Start(offset))?;
        Ok(Box::new(file))
    }
    fn length(&self) -> Option<u64> {
        Some(self.length)
    }
}
fn failed() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "The complete replay context could not be retained",
        false,
    )
}

pub(crate) async fn snapshot_file(
    mut input: std::fs::File,
    length: u64,
    cancel: transmog_core::intercept::ExchangeCancellation,
) -> Result<SnapshotFile, AppError> {
    tokio::task::spawn_blocking(move || {
        let mut file = tempfile::NamedTempFile::new().map_err(|_| failed())?;
        let mut total = 0_u64;
        let mut buffer = vec![0; 65536];
        loop {
            if cancel.is_cancelled() {
                return Err(failed());
            }
            let count = input.read(&mut buffer).map_err(|_| failed())?;
            if count == 0 {
                break;
            }
            total = total.saturating_add(count as u64);
            if total > length {
                return Err(failed());
            }
            file.write_all(&buffer[..count]).map_err(|_| failed())?;
        }
        if total != length {
            return Err(failed());
        }
        file.flush().map_err(|_| failed())?;
        Ok(SnapshotFile { file, length })
    })
    .await
    .map_err(|_| failed())?
}

pub(crate) fn freeze_exchange(
    snapshot: SessionSnapshot,
    source: &BodyStore,
    origin: Option<crate::ComposerOrigin>,
) -> Result<Arc<ComposerResponse>, AppError> {
    let mut config = BodyStoreConfig::product_default(std::path::PathBuf::new());
    config.use_product_preferences = false;
    let store = BodyStore::new(config).map_err(|_| failed())?;
    source.flush().map_err(|_| failed())?;
    for metadata in source.metadata(snapshot.exchange_id) {
        let boundary = inspector::parse_boundary(metadata.boundary)?;
        let mut lease = source
            .open_context(snapshot.exchange_id, boundary, false)
            .map_err(|_| failed())?;
        let mut file = tempfile::NamedTempFile::new().map_err(|_| failed())?;
        let copied = io::copy(&mut lease.by_ref().take(metadata.retained_bytes), &mut file)
            .map_err(|_| failed())?;
        if copied != metadata.retained_bytes {
            return Err(failed());
        }
        file.flush().map_err(|_| failed())?;
        let headers = snapshot
            .request_heads
            .iter()
            .find(|head| head.boundary == boundary)
            .map(|head| &head.head.headers)
            .or_else(|| {
                snapshot
                    .response_heads
                    .iter()
                    .find(|head| head.boundary == boundary)
                    .map(|head| &head.head.headers)
            })
            .ok_or_else(failed)?;
        let response = snapshot
            .response_heads
            .iter()
            .find(|head| head.boundary == boundary)
            .map(|head| head.head.clone());
        store.register_frozen(
            &metadata,
            headers,
            response,
            Arc::new(SnapshotFile {
                file,
                length: copied,
            }),
        );
    }
    Ok(ComposerResponse::from_snapshot(snapshot, store, origin))
}
