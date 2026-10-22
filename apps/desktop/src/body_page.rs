//! Tauri body response: a bounded UTF-8 metadata header followed by raw bytes.
use serde::Serialize;
use transmog_app::{AppError, BodyBytePage, ErrorCategory, StoredBodyMetadata};

const MAX_METADATA_BYTES: usize = 256 * 1024;

pub(crate) fn encode(page: &BodyBytePage) -> Result<Vec<u8>, AppError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Header<'a> {
        metadata: &'a StoredBodyMetadata,
        total_bytes: String,
        offset: String,
        read_limit: usize,
    }
    let header = serde_json::to_vec(&Header {
        metadata: &page.metadata,
        total_bytes: page.total_bytes.to_string(),
        offset: page.offset.to_string(),
        read_limit: page.read_limit,
    })
    .map_err(|_| AppError {
        category: ErrorCategory::Internal,
        message: "Body metadata could not be encoded".into(),
        retryable: false,
    })?;
    if header.len() > MAX_METADATA_BYTES {
        return Err(AppError {
            category: ErrorCategory::Limit,
            message: "Body metadata exceeds the per-call budget".into(),
            retryable: false,
        });
    }
    let length = u32::try_from(header.len()).expect("Metadata budget fits u32");
    let mut response = Vec::with_capacity(4 + header.len() + page.bytes.len());
    response.extend_from_slice(&length.to_le_bytes());
    response.extend_from_slice(&header);
    response.extend_from_slice(&page.bytes);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_app::BodyAvailability;

    fn page() -> BodyBytePage {
        BodyBytePage {
            metadata: StoredBodyMetadata {
                length_known: true,
                exchange_id: "body".into(),
                boundary: "client-response",
                observed_bytes: (1 << 53) + 17,
                wire_body_bytes: None,
                retained_bytes: (1 << 53) + 17,
                availability: BodyAvailability::Complete,
                media_type: Some("application/octet-stream".into()),
                charset: None,
                content_codings: vec!["gzip".into()],
                sha256: None,
                reason: None,
            },
            total_bytes: (1 << 53) + 17,
            offset: (1 << 53) + 2,
            bytes: vec![0, 255, 128, 13, 10],
            read_limit: 65_536,
        }
    }

    #[test]
    fn one_response_keeps_exact_offsets_and_raw_binary_bytes() {
        let page = page();
        let metadata = serde_json::to_value(&page.metadata).unwrap();
        let response = encode(&page).unwrap();
        let length = u32::from_le_bytes(response[..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&response[4..4 + length]).unwrap();
        assert_eq!(header["offset"], "9007199254740994");
        assert_eq!(header["totalBytes"], "9007199254741009");
        assert_eq!(header["metadata"], metadata);
        assert_eq!(header["readLimit"], 65_536);
        assert_eq!(header.as_object().unwrap().len(), 4);
        assert_eq!(&response[4 + length..], &[0, 255, 128, 13, 10]);
    }

    #[test]
    fn empty_pages_and_metadata_budget() {
        let mut empty = page();
        empty.bytes.clear();
        empty.offset = 0;
        empty.total_bytes = 0;
        empty.metadata.retained_bytes = 0;
        let response = encode(&empty).unwrap();
        assert_eq!(
            response.len(),
            4 + u32::from_le_bytes(response[..4].try_into().unwrap()) as usize
        );
        let mut oversized = page();
        oversized.metadata.reason = Some("x".repeat(MAX_METADATA_BYTES));
        assert_eq!(
            encode(&oversized).unwrap_err().category,
            ErrorCategory::Limit
        );
    }
}
