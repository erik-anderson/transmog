//! Complete retained sources for agent inspection, independent of display previews.
use crate::{AppError, Application, ErrorCategory, StoredBodyMetadata, inspector};
use std::io::Read as _;
use transmog_core::{HeaderBlock, observe::ExchangeBoundary};

/// Finite whole-document decoding budget; byte-range reads have no whole-body ceiling.
pub const MAX_CONTEXT_DECODE_BYTES: usize = 256 * 1024 * 1024;

/// Complete retained message content, with honest source coverage.
pub struct CompleteBodyContext {
    /// Authoritative retained-source metadata.
    pub metadata: StoredBodyMetadata,
    /// Full content-decoded bytes, converted to UTF-8 when text decoding is possible.
    pub bytes: Vec<u8>,
    /// The returned text encoding when Unicode conversion succeeded.
    pub text_encoding: Option<String>,
}

fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, false)
}

impl Application {
    fn context_snapshot(&self, id: &str) -> Result<transmog_session::SessionSnapshot, AppError> {
        if let Some(response) = self.composer.response(id) {
            return Ok(response.snapshot.clone());
        }
        self.service
            .catalog()
            .get(transmog_core::intercept::ExchangeId(
                inspector::parse_session_id(id)?,
            ))
            .ok_or_else(|| unavailable("The exchange is no longer retained"))
    }
    /// Reads the complete method and URL without requiring text header values.
    /// # Errors
    /// Returns invalid or unavailable message errors.
    pub fn message_request(&self, id: &str, boundary: &str) -> Result<(String, String), AppError> {
        let snapshot = self.context_snapshot(id)?;
        let head = snapshot
            .request_heads
            .iter()
            .find(|head| inspector::boundary(head.boundary) == boundary)
            .ok_or_else(|| unavailable("The request boundary is unavailable"))?;
        Ok((
            head.head.method.clone(),
            inspector::copy_message_url_from_snapshot(&snapshot, boundary)?,
        ))
    }
    /// Reads retained exchange metadata without inspector display limits; body bytes are read separately.
    /// # Errors
    /// Returns invalid or unavailable exchange errors.
    pub fn retained_exchange_context(&self, id: &str) -> Result<serde_json::Value, AppError> {
        let snapshot = self.context_snapshot(id)?;
        let mut result = serde_json::to_value(self.session_detail(id)?)
            .map_err(|_| unavailable("Exchange metadata could not be encoded"))?;
        let mut diagnostics: Vec<_> = snapshot
            .initialization_diagnostics
            .iter()
            .map(|item| format!("{item:?}"))
            .collect();
        if let Some(entry) = self.traces.entry(id) {
            diagnostics.extend(entry.diagnostics);
        }
        if snapshot.request_number == 0 {
            diagnostics.push("Response received from a live server by Composer.".into());
        }
        result["diagnostics"] = serde_json::json!(diagnostics);
        result["hookEffects"] = serde_json::json!(
            snapshot
                .hook_effects
                .iter()
                .map(|item| format!(
                    "{} · {} ({}, position {}) · {:?} · {} · changed={}",
                    item.sequence,
                    item.interceptor.id.as_str(),
                    item.interceptor.name,
                    item.interceptor.chain_position,
                    item.phase,
                    inspector::hook_action_summary(&item.action),
                    item.changed
                ))
                .collect::<Vec<_>>()
        );
        result["headerEffects"] =
            serde_json::to_value(inspector::header_effects(&snapshot, usize::MAX))
                .map_err(|_| unavailable("Header effects could not be encoded"))?;
        result["bodyEdits"] = serde_json::to_value(inspector::body_edits(&snapshot, usize::MAX))
            .map_err(|_| unavailable("Body edits could not be encoded"))?;
        result["routeSelection"] = serde_json::json!(
            snapshot
                .route_selection
                .as_ref()
                .map(|(policy, reason)| format!("{policy}: {reason}"))
        );
        result["routeAttempts"] = serde_json::json!(
            snapshot
                .route_attempts
                .iter()
                .map(|item| format!("{item:?}"))
                .collect::<Vec<_>>()
        );
        Ok(result)
    }

    /// Reads complete ordered header fields without display truncation.
    /// # Errors
    /// Returns unavailable or invalid boundary/reference errors.
    pub fn message_headers(&self, id: &str, boundary: &str) -> Result<HeaderBlock, AppError> {
        let snapshot = self.context_snapshot(id)?;
        if let Some(head) = snapshot
            .request_heads
            .iter()
            .find(|head| inspector::boundary(head.boundary) == boundary)
        {
            return Ok(head.head.headers.clone());
        }
        snapshot
            .response_heads
            .iter()
            .find(|head| inspector::boundary(head.boundary) == boundary)
            .map(|head| head.head.headers.clone())
            .ok_or_else(|| unavailable("The message boundary is unavailable"))
    }

    /// Reads and decodes a complete retained body for extraction, testing or paged context.
    /// This never treats a prefix as a complete document.
    /// # Errors
    /// Returns missing/incomplete retention, decoding or finite resource-limit errors.
    pub async fn complete_body_context(
        &self,
        id: &str,
        boundary: &str,
    ) -> Result<CompleteBodyContext, AppError> {
        let store = self
            .inspection_body_store(id)
            .ok_or_else(|| unavailable("Body retention is unavailable"))?;
        let exchange = transmog_core::intercept::ExchangeId(inspector::parse_session_id(id)?);
        let boundary: ExchangeBoundary = inspector::parse_boundary(boundary)?;
        let (metadata, bytes) =
            tokio::task::spawn_blocking(move || read_complete_bytes(&store, exchange, boundary))
                .await
                .map_err(|_| unavailable("The body context worker failed"))??;
        let decoded = inspector::decode_content_with_limit(
            &metadata.content_codings,
            bytes,
            MAX_CONTEXT_DECODE_BYTES,
        )
        .await?;
        let (bytes, text_encoding) =
            inspector::decode_unicode(&decoded, metadata.charset.as_deref()).map_or_else(
                || (decoded, None),
                |text| (text.into_bytes(), Some("utf-8".into())),
            );
        if bytes.len() > MAX_CONTEXT_DECODE_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "The UTF-8 document exceeds the 256 MiB context budget; retained byte ranges remain available",
                false,
            ));
        }
        Ok(CompleteBodyContext {
            metadata,
            bytes,
            text_encoding,
        })
    }
}

fn context_limit() -> AppError {
    AppError::new(
        ErrorCategory::Limit,
        "The complete document exceeds the 256 MiB context budget; retained byte ranges remain available",
        false,
    )
}
fn read_complete_bytes(
    store: &crate::BodyStore,
    exchange: transmog_core::intercept::ExchangeId,
    boundary: ExchangeBoundary,
) -> Result<(StoredBodyMetadata, Vec<u8>), AppError> {
    store
        .flush()
        .map_err(|_| unavailable("Body retention could not be synchronized"))?;
    let lease=store.open_complete(exchange,boundary).map_err(|_|unavailable("The complete retained body is unavailable; inspect retention metadata or read retained byte ranges"))?;
    let mut metadata = lease.metadata().clone();
    if metadata.retained_bytes > MAX_CONTEXT_DECODE_BYTES as u64 {
        return Err(context_limit());
    }
    let mut bytes = Vec::new();
    lease
        .take(MAX_CONTEXT_DECODE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable("Retained body bytes could not be read"))?;
    if bytes.len() > MAX_CONTEXT_DECODE_BYTES {
        return Err(context_limit());
    }
    if metadata.length_known && bytes.len() as u64 != metadata.retained_bytes {
        return Err(unavailable("The complete retained body could not be read"));
    }
    if !metadata.length_known {
        metadata.retained_bytes = bytes.len() as u64;
        metadata.length_known = true;
    }
    Ok((metadata, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[derive(Debug)]
    struct DeferredSource;
    impl crate::body_store::SavedBodySource for DeferredSource {
        fn length(&self) -> Option<u64> {
            None
        }
        fn open(&self) -> std::io::Result<Box<dyn std::io::Read + Send>> {
            Ok(Box::new(std::io::Cursor::new(
                b"complete deferred context".to_vec(),
            )))
        }
    }
    #[tokio::test]
    async fn complete_saved_context_resolves_a_deferred_length_by_reading_to_eof() {
        let root = tempfile::tempdir().unwrap();
        let app = Application::new(crate::AppConfig {
            body_store: Some(crate::BodyStoreConfig::product_default(
                root.path().join("bodies"),
            )),
            ..Default::default()
        })
        .unwrap();
        let id = transmog_core::intercept::ExchangeId(17);
        app.body_store().unwrap().register_saved(
            id,
            ExchangeBoundary::ClientResponse,
            &HeaderBlock::new(),
            None,
            Arc::new(DeferredSource),
            0,
            true,
        );
        let context = app
            .complete_body_context(&format!("{:032x}", id.0), "client-response")
            .await
            .unwrap();
        assert_eq!(context.bytes, b"complete deferred context");
        assert!(context.metadata.length_known);
        assert_eq!(context.metadata.retained_bytes, 25);
    }
}
