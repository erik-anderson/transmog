//! The latest Composer response owns its bytes independently of Traffic retention.
use crate::{
    AppError, BodyStore, BodyStoreConfig, ErrorCategory, SessionDetail,
    body_store::SavedBodySource, inspector,
};
use bytes::Bytes;
use std::{
    io::{self, Read},
    sync::Arc,
    time::SystemTime,
};
use transmog_core::{
    ClientIdentity, ConnectionId, RequestHead, ResponseHead, StreamId,
    intercept::{CompletedExchange, ExchangeId, ExchangeMetadata, OriginalTarget},
    observe::ExchangeBoundary,
};
use transmog_session::{
    ObservedRequestHead, ObservedResponseHead, ReplayResponse, SessionSnapshot, SessionTerminal,
};

pub(crate) struct ComposerResponse {
    pub snapshot: SessionSnapshot,
    pub store: BodyStore,
    origin: Option<crate::ComposerOrigin>,
}

pub(crate) struct PreparedComposerResponse {
    exchange_id: ExchangeId,
    store: BodyStore,
}

#[derive(Debug)]
struct ResponseBytes(Bytes);
impl SavedBodySource for ResponseBytes {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        self.open_at(0)
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        if offset > self.0.len() as u64 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let mut cursor = io::Cursor::new(self.0.clone());
        cursor.set_position(offset);
        Ok(Box::new(cursor))
    }
    fn length(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }
}

impl ComposerResponse {
    pub(crate) fn prepare(
        service: &transmog_session::ApplicationSessionService,
    ) -> Result<PreparedComposerResponse, AppError> {
        let exchange_id = response_id(service)?;
        // Memory-only storage never creates or cleans a cache directory. The
        // replay service has already bounded the complete owned response.
        let mut config = BodyStoreConfig::product_default(std::path::PathBuf::new());
        config.use_product_preferences = false;
        let store = BodyStore::new(config).map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "Response bytes could not be retained for inspection",
                true,
            )
        })?;
        store.flush().map_err(|_| {
            AppError::new(
                ErrorCategory::Unavailable,
                "Response inspection could not be initialized",
                true,
            )
        })?;
        Ok(PreparedComposerResponse { exchange_id, store })
    }

    pub(crate) fn retain(
        prepared: PreparedComposerResponse,
        mut request: RequestHead,
        response: &ReplayResponse,
        started_at: SystemTime,
        redact: bool,
        origin: Option<crate::ComposerOrigin>,
        mut performance: transmog_core::performance::PerformanceEvidence,
    ) -> Result<(Arc<Self>, SessionDetail), AppError> {
        let PreparedComposerResponse { exchange_id, store } = prepared;
        let mut head = ResponseHead {
            status: response.status,
            headers: response.headers.clone(),
            source_version: response.source_version,
        };
        store.register_saved(
            exchange_id,
            ExchangeBoundary::ClientResponse,
            &head.headers,
            Some(head.clone()),
            Arc::new(ResponseBytes(response.body.clone())),
            response.body.len() as u64,
            true,
        );
        if redact {
            request.headers.redact_sensitive();
            head.headers.redact_sensitive();
        }
        request.source_version = response.source_version;
        let metadata = Arc::new(ExchangeMetadata {
            exchange_id,
            downstream_connection_id: ConnectionId(0),
            downstream_stream_id: StreamId(0),
            client_addr: "127.0.0.1:0".parse().expect("loopback address"),
            listener_addr: "127.0.0.1:0".parse().expect("loopback address"),
            client_identity: ClientIdentity::LocalProcess {
                pid: std::process::id(),
                name: Some("Transmog Composer".into()),
            },
            ingress_version: response.source_version,
            original_target: OriginalTarget::new(request.target.clone()),
            started_at,
        });
        let snapshot = SessionSnapshot {
            performance: {
                performance
                    .protocols
                    .push(transmog_core::performance::ProtocolObservation {
                        boundary: "client-response".into(),
                        version: match response.source_version {
                            transmog_core::HttpLegVersion::Http1 => "HTTP/1.1",
                            transmog_core::HttpLegVersion::Http2 => "HTTP/2",
                            transmog_core::HttpLegVersion::Http3 => "HTTP/3",
                        }
                        .into(),
                        reason: None,
                    });
                performance
            },
            imported: false,
            exchange_id,
            metadata: metadata.clone(),
            last_sequence: 0,
            sequence_loss: 0,
            request_heads: vec![ObservedRequestHead {
                boundary: ExchangeBoundary::ClientRequest,
                head: request.clone(),
            }],
            response_heads: vec![ObservedResponseHead {
                boundary: ExchangeBoundary::ClientResponse,
                head: head.clone(),
            }],
            bodies: Vec::new(),
            initialization_diagnostics: Vec::new(),
            hook_effects: Vec::new(),
            route_selection: None,
            route_attempts: Vec::new(),
            terminal: Some(SessionTerminal::Completed(CompletedExchange {
                metadata,
                request_head: request,
                response_head: head,
            })),
            terminal_at: Some(SystemTime::now()),
            websocket: None,
            stream: None,
            stream_history: transmog_core::stream::StreamHistory::default(),
        };
        let retained = Arc::new(Self {
            snapshot,
            store,
            origin,
        });
        let detail = retained.detail()?;
        Ok((retained, detail))
    }
    pub(crate) fn detail(&self) -> Result<SessionDetail, AppError> {
        // This store is filled synchronously once, so its immutable metadata
        // does not need to wait on an observer worker after the send completed.
        let mut detail = inspector::detail_from_snapshot(&self.snapshot, None)?;
        detail.stored_bodies = self.store.metadata(self.snapshot.exchange_id);
        detail.composer_origin.clone_from(&self.origin);
        detail
            .diagnostics
            .push("Response received from a live server by Composer.".into());
        Ok(detail)
    }
}

fn response_id(
    service: &transmog_session::ApplicationSessionService,
) -> Result<ExchangeId, AppError> {
    loop {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|_| {
            AppError::new(
                ErrorCategory::Internal,
                "Response identity could not be created",
                true,
            )
        })?;
        let id = ExchangeId(u128::from_le_bytes(bytes));
        if id.0 != 0 && service.catalog().get(id).is_none() {
            return Ok(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, Application, BodyRangeRequest, ComposerRequest};
    use std::sync::Mutex;
    use transmog_core::{
        HeaderBlock, HeaderField, HttpLegVersion, intercept::ExchangeCancellation,
    };
    use transmog_session::{
        BoxReplayFuture, ReplayExecutionError, ReplayExecutor, ValidatedReplayRequest,
    };

    struct FixtureExecutor(Mutex<ReplayResponse>);
    impl ReplayExecutor for FixtureExecutor {
        fn execute(
            &self,
            _: ValidatedReplayRequest,
            _: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            let response = self.0.lock().unwrap().clone();
            Box::pin(async move { Ok(response) })
        }
        fn execute_file(
            &self,
            _: ValidatedReplayRequest,
            _: std::fs::File,
            _: u64,
            _: ExchangeCancellation,
        ) -> BoxReplayFuture<'_> {
            Box::pin(async { Err(ReplayExecutionError::new("No fixture file body")) })
        }
    }
    fn input() -> ComposerRequest {
        ComposerRequest {
            method: "GET".into(),
            url: "https://example.test/binary.bin?query=full".into(),
            headers: Vec::new(),
            body: String::new(),
            body_source: None,
            source_entry_id: None,
            body_is_hex: false,
            acknowledge_non_idempotent: false,
            acknowledge_credentials: false,
        }
    }
    fn request(id: &str, offset: u64, length: usize) -> BodyRangeRequest {
        BodyRangeRequest {
            session_id: id.into(),
            boundary: "client-response".into(),
            offset: offset.to_string(),
            max_bytes: Some(length),
        }
    }

    #[tokio::test]
    async fn shared_inspection_retains_full_binary_bytes_without_traffic_and_leases_survive_replacement()
     {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let bytes = (0..300_019)
            .map(|index| u8::try_from(index % 256).unwrap())
            .collect::<Vec<_>>();
        let executor = Arc::new(FixtureExecutor(Mutex::new(ReplayResponse {
            source_version: HttpLegVersion::Http2,
            status: 200,
            body: Bytes::from(bytes.clone()),
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("Content-Type", "application/octet-stream").unwrap(),
                HeaderField::try_new("X-Duplicate", "one").unwrap(),
                HeaderField::try_new("X-Duplicate", "two").unwrap(),
                HeaderField::try_new("Set-Cookie", "private=value").unwrap(),
            ]),
        })));
        let app = Application::new(AppConfig {
            replay_executor: Some(executor.clone()),
            ..AppConfig::default()
        })
        .unwrap();
        assert!(app.body_store().is_none());
        let mut preferences = app.product_state();
        preferences.privacy.redact_sensitive_headers = true;
        app.save_product_state(preferences).unwrap();
        let result = app.execute_composer(input()).await.unwrap();
        assert!(
            result.truncated,
            "The legacy inline preview should be bounded"
        );
        let id = &result.detail.id;
        assert_eq!(result.detail.responses[0].protocol, "HTTP/2");
        assert_eq!(
            result.detail.stored_bodies[0].retained_bytes,
            bytes.len() as u64
        );
        assert!(
            app.query_sessions(crate::SessionQueryInput::default())
                .unwrap()
                .sessions
                .is_empty()
        );
        assert_eq!(
            app.copy_message_url(id, "client-response").unwrap(),
            input().url
        );
        let headers = app.copy_message_headers(id, "client-response").unwrap();
        assert!(headers.contains("X-Duplicate: one\r\nX-Duplicate: two"));
        assert!(headers.contains("Set-Cookie: [redacted]"));
        assert!(!headers.contains("private=value"));
        let page = app.read_body_range(request(id, 300_010, 9)).await.unwrap();
        assert_eq!(page.total_bytes, bytes.len().to_string());
        assert_eq!(
            STANDARD.decode(page.bytes_base64).unwrap(),
            bytes[300_010..]
        );
        let leased = app.prepare_response_file(id, "client-response").unwrap();
        assert_eq!(leased.suggested_name(), "binary.bin");
        executor.0.lock().unwrap().body = Bytes::from_static(b"replacement");
        let replacement = app.execute_composer(input()).await.unwrap();
        assert_ne!(id, &replacement.detail.id);
        assert!(app.session_detail(id).is_err());
        assert!(app.read_body_range(request(id, 0, 1)).await.is_err());
        drop(app);
        let root = tempfile::tempdir().unwrap();
        let saved = leased.save_to(root.path().join("saved.bin")).await.unwrap();
        assert_eq!(saved.bytes, bytes.len() as u64);
        assert_eq!(std::fs::read(root.path().join("saved.bin")).unwrap(), bytes);
    }

    #[tokio::test]
    async fn encoded_composer_response_uses_shared_decoding_and_saves_original_decoded_content() {
        use crate::{BodyInspectionRequest, BodyRepresentation};
        let original = br#"{"received":"complete response","notMarkup":true}"#.to_vec();
        let encoded = inspector::encode_content(&["gzip".into()], original.clone())
            .await
            .unwrap();
        let app = Application::new(AppConfig {
            replay_executor: Some(Arc::new(FixtureExecutor(Mutex::new(ReplayResponse {
                source_version: HttpLegVersion::Http1,
                status: 200,
                body: Bytes::from(encoded.clone()),
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("Content-Type", "application/json").unwrap(),
                    HeaderField::try_new("Content-Encoding", "gzip").unwrap(),
                ]),
            })))),
            ..AppConfig::default()
        })
        .unwrap();
        let result = app.execute_composer(input()).await.unwrap();
        let id = result.detail.id;
        let view = app
            .inspect_body(BodyInspectionRequest {
                session_id: id.clone(),
                boundary: "client-response".into(),
                representation: BodyRepresentation::Auto,
                decode_content: true,
                offset: 0,
                max_bytes: Some(262_144),
            })
            .await
            .unwrap();
        assert_eq!(view.representation, "formatted-json");
        assert!(view.decoded);
        assert!(view.display.contains("complete response"));
        assert_eq!(view.metadata.content_codings, ["gzip"]);
        let raw = app
            .read_body_range(request(&id, 0, encoded.len()))
            .await
            .unwrap();
        assert_eq!(raw.total_bytes, encoded.len().to_string());
        let root = tempfile::tempdir().unwrap();
        app.prepare_response_file(&id, "client-response")
            .unwrap()
            .save_to(root.path().join("decoded.json"))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("decoded.json")).unwrap(),
            original
        );
    }
}
