//! Imported metadata is owned by the automation commit, not the separate asset index.
use super::{
    MAX_ASSET_BYTES, ResponseAsset, ResponseAssetProvenance, ResponseAssetStore,
    encode_file_temporary, invalid, repair_headers, set_content_encoding, snapshot, store_body,
    unavailable,
};
use crate::AppError;
use sha2::Digest as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    sync::{
        Arc, MutexGuard, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Default)]
pub(super) struct ImportedCatalog {
    fallback: Arc<BTreeMap<String, ResponseAsset>>,
    live: Option<Weak<RwLock<Arc<crate::automation::ActiveAutomation>>>>,
}

impl ResponseAssetStore {
    pub(crate) fn portable_lock(&self) -> MutexGuard<'_, ()> {
        self.mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn with_imports(&self, assets: &[ResponseAsset]) -> Self {
        Self {
            imported: Arc::new(RwLock::new(ImportedCatalog {
                fallback: Arc::new(assets.iter().map(|a| (a.asset_ref(), a.clone())).collect()),
                live: None,
            })),
            ..self.clone()
        }
    }
    pub(crate) fn publish_imports(&self, assets: &[ResponseAsset]) {
        self.imported
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fallback = Arc::new(assets.iter().map(|a| (a.asset_ref(), a.clone())).collect());
    }
    pub(crate) fn bind_import_catalog(
        &self,
        active: &Arc<RwLock<Arc<crate::automation::ActiveAutomation>>>,
    ) {
        self.imported
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .live = Some(Arc::downgrade(active));
    }
    pub(super) fn imported_catalog(&self) -> Arc<BTreeMap<String, ResponseAsset>> {
        // Drop the catalog lock before acquiring the registry lock. Publication
        // can update fallback storage while readers retain an older generation.
        let catalog = self
            .imported
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(active) = catalog.live.and_then(|weak| weak.upgrade()) {
            active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .imported_assets
                .clone()
        } else {
            catalog.fallback
        }
    }
    pub(crate) fn stage_portable_body(
        &self,
        body: File,
        expected: &transmog_portable::Object,
        canceled: &AtomicBool,
    ) -> Result<(), AppError> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| unavailable("Response asset storage is unavailable"))?;
        let (bytes, sha) = store_body(root, CanceledReader { body, canceled })?;
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Rule import canceled"));
        }
        if sha != expected.sha256 || bytes.to_string() != expected.bytes {
            return Err(invalid("Staged response integrity changed"));
        }
        // An existing immutable blob must also be verified, not trusted by filename/length.
        let mut stored = File::open(super::body_path(root, &sha))
            .map_err(|_| unavailable("Staged body unavailable"))?;
        let mut hash = sha2::Sha256::default();
        let mut buffer = vec![0; 128 * 1024];
        loop {
            let count = stored
                .read(&mut buffer)
                .map_err(|_| unavailable("Staged body unreadable"))?;
            if count == 0 {
                break;
            }
            if canceled.load(Ordering::Acquire) {
                return Err(unavailable("Rule import canceled"));
            }
            hash.update(&buffer[..count]);
        }
        if hex::encode(hash.finalize()) != sha {
            return Err(invalid(
                "Existing response blob failed integrity verification",
            ));
        }
        Ok(())
    }
    pub(crate) fn portable_metadata(
        value: &transmog_portable::Asset,
        id: String,
    ) -> Result<ResponseAsset, AppError> {
        let bytes = transmog_portable::decimal(value.body.bytes.as_deref().unwrap_or(""), true)
            .map_err(super::super::rule_interchange::portable_error)?;
        let fields = value
            .headers
            .iter()
            .map(|field| {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&field.value_base64)
                    .map_err(|_| invalid("Invalid header value"))?;
                transmog_core::HeaderField::try_new(field.name.as_bytes(), bytes)
                    .map_err(|_| invalid("Invalid header field"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut headers = transmog_core::HeaderBlock::new();
        for field in fields {
            headers.push(field);
        }
        let media_type = headers
            .values("content-type")
            .next()
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(str::to_owned);
        Ok(ResponseAsset {
            id,
            revision: transmog_portable::decimal(&value.revision, false)
                .map_err(super::super::rule_interchange::portable_error)?,
            status: value.status,
            headers: repair_headers(&headers, bytes, media_type.as_deref())?,
            body_bytes: bytes,
            sha256: value
                .body
                .sha256
                .clone()
                .ok_or_else(|| invalid("Missing body digest"))?,
            media_type,
            provenance: ResponseAssetProvenance::Imported,
            linked_body: None,
            copied_from: None,
        })
    }
    pub(crate) fn portable_snapshot(
        &self,
        reference: &str,
        canceled: &AtomicBool,
    ) -> Result<(ResponseAsset, tempfile::NamedTempFile), AppError> {
        let mut asset = self.get(reference)?;
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| unavailable("Response storage is unavailable"))?;
        let file = if let Some(link) = &asset.linked_body {
            snapshot::open(&link.path)?
        } else {
            self.open_body(&asset)?
        };
        let mut frozen = snapshot::acquire(file, root, MAX_ASSET_BYTES, || {
            canceled.load(Ordering::Acquire)
        })?;
        if let Some(link) = &asset.linked_body {
            let codings = if link.apply_original_compression {
                link.original_content_codings.as_slice()
            } else {
                &[]
            };
            if !codings.is_empty() {
                let encoded = encode_file_temporary(
                    frozen
                        .file
                        .reopen()
                        .map_err(|_| unavailable("Snapshot unavailable"))?,
                    frozen.bytes,
                    codings,
                    root,
                )?;
                frozen = snapshot::owned(encoded, MAX_ASSET_BYTES)?;
            }
            set_content_encoding(&mut asset.headers, codings)?;
            asset.headers =
                repair_headers(&asset.headers, frozen.bytes, asset.media_type.as_deref())?;
        } else if frozen.bytes != asset.body_bytes || frozen.sha256 != asset.sha256 {
            return Err(invalid(
                "Stored response body failed integrity verification",
            ));
        }
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Rule export canceled"));
        }
        asset.body_bytes = frozen.bytes;
        asset.sha256 = frozen.sha256;
        Ok((asset, frozen.file))
    }
    pub(crate) fn import_catalog_valid(&self, assets: &[ResponseAsset]) -> bool {
        let own = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut seen = BTreeSet::new();
        assets.iter().all(|a| {
            a.linked_body.is_none()
                && a.copied_from.is_none()
                && transmog_portable::valid_digest(&a.sha256)
                && a.body_bytes <= MAX_ASSET_BYTES
                && !own.contains_key(&a.asset_ref())
                && seen.insert(a.asset_ref())
        })
    }
}
struct CanceledReader<'a> {
    body: File,
    canceled: &'a AtomicBool,
}
impl Read for CanceledReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Rule import canceled"));
        }
        self.body.read(buffer)
    }
}
