//! Bounded byte pages with exact 64-bit offsets for the virtual Hex viewer.
use crate::{AppError, BodyStore, ErrorCategory, StoredBodyMetadata, inspector};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

/// One raw retained-body byte range. Decimal strings avoid JavaScript precision loss.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BodyRangeRequest {
    /// Retained exchange identifier in this window.
    pub session_id: String,
    /// Original/effective message stage.
    pub boundary: String,
    /// Exact unsigned byte offset as decimal text.
    pub offset: String,
    /// Per-call byte budget; unrelated to the total body size.
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

/// A bounded raw byte page and the exact length of its retained source.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyBytePage {
    /// Original representation and retention context, never preview decoding.
    pub metadata: StoredBodyMetadata,
    /// Exact retained byte count in decimal text.
    pub total_bytes: String,
    /// Exact returned byte offset in decimal text.
    pub offset: String,
    /// Lossless bytes for this page.
    pub bytes_base64: String,
    /// Maximum bytes accepted per call for this store.
    pub read_limit: usize,
}

pub(crate) fn read(
    store: &BodyStore,
    request: &BodyRangeRequest,
) -> Result<BodyBytePage, AppError> {
    let id =
        transmog_core::intercept::ExchangeId(inspector::parse_session_id(&request.session_id)?);
    let boundary = inspector::parse_boundary(&request.boundary)?;
    let offset = request.offset.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorCategory::InvalidInput,
            "Choose a nonnegative whole byte offset",
            false,
        )
    })?;
    let requested = request.max_bytes.unwrap_or(64 * 1024);
    if requested == 0 || requested > crate::body_store::MAX_BODY_READ_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "One byte page exceeds the per-call read budget",
            false,
        ));
    }
    store.flush().map_err(|_| {
        AppError::new(
            ErrorCategory::Unavailable,
            "Body retention could not be synchronized",
            true,
        )
    })?;
    let read_limit = store.range_read_limit();
    let range = store
        .read_paged_range(id, boundary, offset, requested.min(read_limit))
        .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
    let mut metadata = store
        .metadata(id)
        .into_iter()
        .find(|metadata| metadata.boundary == request.boundary)
        .ok_or_else(|| {
            AppError::new(
                ErrorCategory::Unavailable,
                "The retained body is no longer available",
                false,
            )
        })?;
    metadata.retained_bytes = range.retained_bytes;
    metadata.availability = range.availability;
    Ok(BodyBytePage {
        metadata,
        total_bytes: range.retained_bytes.to_string(),
        offset: range.offset.to_string(),
        bytes_base64: STANDARD.encode(range.bytes),
        read_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodyStoreConfig, body_store::SavedBodySource};
    use std::{
        io::{self, Read},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };
    use transmog_core::{HeaderBlock, intercept::ExchangeId, observe::ExchangeBoundary};

    #[derive(Debug)]
    struct VirtualSource {
        bytes: u64,
        read: Arc<AtomicU64>,
    }
    struct VirtualReader {
        position: u64,
        end: u64,
        read: Arc<AtomicU64>,
    }
    impl Read for VirtualReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer
                .len()
                .min(usize::try_from(self.end - self.position).unwrap_or(usize::MAX));
            for byte in &mut buffer[..count] {
                *byte = self.position.to_le_bytes()[0];
                self.position += 1;
            }
            self.read.fetch_add(count as u64, Ordering::Relaxed);
            Ok(count)
        }
    }
    impl SavedBodySource for VirtualSource {
        fn open(&self) -> io::Result<Box<dyn Read + Send>> {
            panic!("A far byte page replayed the source from zero");
        }
        fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
            Ok(Box::new(VirtualReader {
                position: offset,
                end: self.bytes,
                read: self.read.clone(),
            }))
        }
        fn length(&self) -> Option<u64> {
            Some(self.bytes)
        }
    }
    #[test]
    fn far_byte_pages_are_exact_beyond_javascript_integer_precision_and_only_read_the_page() {
        let root = tempfile::tempdir().unwrap();
        let store =
            BodyStore::new(BodyStoreConfig::product_default(root.path().join("body"))).unwrap();
        let bytes = (1u64 << 53) + 17;
        let counter = Arc::new(AtomicU64::new(0));
        store.register_saved(
            ExchangeId(1),
            ExchangeBoundary::ClientResponse,
            &HeaderBlock::default(),
            None,
            Arc::new(VirtualSource {
                bytes,
                read: counter.clone(),
            }),
            bytes,
            true,
        );
        let mut request = BodyRangeRequest {
            session_id: format!("{:032x}", 1),
            boundary: "client-response".into(),
            offset: ((1u64 << 53) + 2).to_string(),
            max_bytes: Some(5),
        };
        let page = read(&store, &request).unwrap();
        assert_eq!(page.offset, "9007199254740994");
        assert_eq!(page.total_bytes, "9007199254741009");
        assert_eq!(
            STANDARD.decode(&page.bytes_base64).unwrap(),
            [2, 3, 4, 5, 6]
        );
        assert_eq!(counter.load(Ordering::Relaxed), 6);
        let serialized = serde_json::to_value(&page).unwrap();
        assert_eq!(serialized["totalBytes"], "9007199254741009");
        request.offset = bytes.to_string();
        assert!(read(&store, &request).is_err());
        request.offset = "-1".into();
        assert_eq!(
            read(&store, &request).unwrap_err().category,
            ErrorCategory::InvalidInput
        );
        request.offset = "0".into();
        request.max_bytes = Some(0);
        assert_eq!(
            read(&store, &request).unwrap_err().category,
            ErrorCategory::Limit
        );
    }

    #[tokio::test]
    async fn an_unencoded_large_body_auto_preview_reads_only_its_display_budget() {
        let root = tempfile::tempdir().unwrap();
        let store =
            BodyStore::new(BodyStoreConfig::product_default(root.path().join("body"))).unwrap();
        let bytes = 100 * 1024 * 1024;
        let counter = Arc::new(AtomicU64::new(0));
        store.register_saved(
            ExchangeId(2),
            ExchangeBoundary::ClientResponse,
            &HeaderBlock::default(),
            None,
            Arc::new(VirtualSource {
                bytes,
                read: counter.clone(),
            }),
            bytes,
            true,
        );
        let inspection = inspector::inspect_body(
            Some(&store),
            crate::BodyInspectionRequest {
                session_id: format!("{:032x}", 2),
                boundary: "client-response".into(),
                representation: crate::BodyRepresentation::Auto,
                decode_content: true,
                offset: 0,
                max_bytes: Some(32),
            },
        )
        .await
        .unwrap();
        assert_eq!(inspection.display_bytes, 32);
        assert!(inspection.truncated);
        assert!(!inspection.decoded);
        assert_eq!(counter.load(Ordering::Relaxed), 33);
    }
}
