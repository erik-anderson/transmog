use std::{
    io::{Read, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use bytes::Bytes;
use serde::Serialize;
use transmog_content::{ContentCodingStack, ContentDecoder, ContentLimits};
use transmog_core::{
    BodyFrame, HeaderBlock, HeaderField, intercept::ExchangeId, observe::ExchangeBoundary,
};
use transmog_session::ApplicationSessionService;

use crate::{
    AppError, BodyAvailability, BodyStore, ErrorCategory, StoredBodyMetadata,
    inspector::parse_session_id, response_filename::suggested_filename,
};

const MAX_FILE_BYTES: u64 = crate::body_store::DEFAULT_BODY_STORE_BYTES;

/// A complete response body protected from cache eviction while Save as is open.
pub struct ResponseFile {
    name: String,
    metadata: StoredBodyMetadata,
    reader: Box<dyn Read + Send>,
}

/// Confirmation of an explicitly selected response-file destination.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseFileResult {
    /// The selected filename, without its directory.
    pub file_name: String,
    /// Original decoded body bytes written, independent of preview limits.
    pub bytes: u64,
}

impl ResponseFile {
    /// Advisory filename from Content-Disposition, the URL, or the media type.
    pub fn suggested_name(&self) -> &str {
        &self.name
    }

    /// Saves the original content-decoded bytes at an explicitly chosen path.
    ///
    /// Uses a temporary file beside the destination, then atomically replaces
    /// it only after complete decoding and successful writes. The caller owns
    /// any overwrite confirmation, such as the native Save as dialog.
    ///
    /// # Errors
    /// Returns bounded read, coding, byte-limit, or filesystem failures.
    pub async fn save_to(self, destination: PathBuf) -> Result<ResponseFileResult, AppError> {
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || runtime.block_on(self.write_to(&destination)))
            .await
            .map_err(|_| unavailable("Response save worker failed"))?
    }

    async fn write_to(mut self, destination: &Path) -> Result<ResponseFileResult, AppError> {
        if !destination.is_absolute() || destination.file_name().is_none() {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "Select an absolute response-file destination",
                false,
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| unavailable("Response destination is unavailable"))?;
        let mut output = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| unavailable("Response file could not be created"))?;
        let mut decoders = response_decoders(&self.metadata)?;
        let (mut encoded_bytes, mut decoded_bytes) = (0_u64, 0_u64);
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = self
                .reader
                .read(&mut buffer)
                .map_err(|_| unavailable("Retained response bytes could not be read"))?;
            if count == 0 {
                break;
            }
            encoded_bytes = encoded_bytes.saturating_add(count as u64);
            if encoded_bytes > self.metadata.retained_bytes || encoded_bytes > MAX_FILE_BYTES {
                return Err(unavailable("Retained response length changed"));
            }
            write_frames(
                &mut output,
                &mut decoded_bytes,
                decode_frames(
                    &mut decoders,
                    vec![BodyFrame::Data(Bytes::copy_from_slice(&buffer[..count]))],
                )
                .await?,
            )?;
        }
        if self.metadata.length_known && encoded_bytes != self.metadata.retained_bytes {
            return Err(unavailable("Retained response is incomplete"));
        }
        for index in 0..decoders.len() {
            let frames = decoders[index]
                .finish()
                .await
                .map_err(|error| decode_error(&error))?;
            write_frames(
                &mut output,
                &mut decoded_bytes,
                decode_frames(&mut decoders[index + 1..], frames).await?,
            )?;
        }
        output
            .as_file()
            .sync_all()
            .map_err(|_| unavailable("Response file could not be flushed"))?;
        output.persist(destination).map_err(|_| {
            unavailable("Response file could not be saved at the selected destination")
        })?;
        Ok(ResponseFileResult {
            file_name: destination
                .file_name()
                .expect("validated filename")
                .to_string_lossy()
                .into_owned(),
            bytes: decoded_bytes,
        })
    }
}

pub(crate) fn prepare(
    service: &ApplicationSessionService,
    store: Option<&BodyStore>,
    session_id: &str,
    boundary: &str,
) -> Result<ResponseFile, AppError> {
    let exchange_id = ExchangeId(parse_session_id(session_id)?);
    let boundary_id = match boundary {
        "client-response" => ExchangeBoundary::ClientResponse,
        "upstream-response" => ExchangeBoundary::UpstreamResponse,
        _ => {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "Select a response body to save",
                false,
            ));
        }
    };
    let store = store.ok_or_else(|| unavailable("Response body retention is not configured"))?;
    store
        .flush()
        .map_err(|_| unavailable("Response body metadata is unavailable"))?;
    let metadata = store
        .metadata(exchange_id)
        .into_iter()
        .find(|body| body.boundary == boundary)
        .ok_or_else(|| unavailable("No response body was retained at this message stage"))?;
    if metadata.availability != BodyAvailability::Complete {
        return Err(unavailable(format!(
            "Only complete response bodies can be saved: {}",
            metadata
                .reason
                .as_deref()
                .unwrap_or("body is still being captured or is unavailable")
        )));
    }
    if metadata.retained_bytes > MAX_FILE_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "Response file exceeds the one-GiB limit",
            false,
        ));
    }
    let reader: Box<dyn Read + Send> = if metadata.retained_bytes == 0 {
        Box::new(std::io::Cursor::new(Vec::<u8>::new()))
    } else {
        Box::new(
            store
                .open_complete(exchange_id, boundary_id)
                .map_err(|_| unavailable("Complete response bytes are unavailable"))?,
        )
    };
    let snapshot = service.catalog().get(exchange_id);
    let head = store.response_head(exchange_id, boundary_id);
    let dispositions = head
        .as_ref()
        .map(|head| {
            head.headers
                .values("content-disposition")
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let disposition = if dispositions.len() == 1 {
        std::str::from_utf8(dispositions[0]).ok()
    } else {
        None
    };
    let name = suggested_filename(
        disposition,
        snapshot.as_ref().map_or("", |snapshot| {
            snapshot.metadata.original_target.as_target().path.as_str()
        }),
        metadata.media_type.as_deref(),
    );
    Ok(ResponseFile {
        name,
        metadata,
        reader,
    })
}

fn response_decoders(metadata: &StoredBodyMetadata) -> Result<Vec<ContentDecoder>, AppError> {
    if metadata.content_codings.is_empty() || metadata.retained_bytes == 0 {
        return Ok(Vec::new());
    }
    let bound = NonZeroUsize::new(usize::try_from(MAX_FILE_BYTES).expect("file limit fits usize"))
        .expect("nonzero file limit");
    let defaults = ContentLimits::default();
    let limits = ContentLimits::new(
        bound,
        bound,
        bound,
        defaults.max_decoder_window_bytes(),
        defaults.max_expansion_ratio(),
        defaults.expansion_slack_bytes(),
        defaults.max_coding_layers(),
    );
    let field = HeaderField::try_new("content-encoding", metadata.content_codings.join(", "))
        .map_err(|_| unavailable("Stored content-coding metadata is invalid"))?;
    let stack = ContentCodingStack::from_headers(
        &HeaderBlock::from_fields(vec![field]),
        limits.max_coding_layers(),
    )
    .map_err(|error| unavailable(error.to_string()))?;
    stack
        .decode_order()
        .map(|coding| ContentDecoder::new(coding, limits).map_err(|error| decode_error(&error)))
        .collect()
}

async fn decode_frames(
    decoders: &mut [ContentDecoder],
    mut frames: Vec<BodyFrame>,
) -> Result<Vec<BodyFrame>, AppError> {
    for decoder in decoders {
        let mut output = Vec::new();
        for frame in frames {
            output.extend(
                decoder
                    .on_frame(frame)
                    .await
                    .map_err(|error| decode_error(&error))?,
            );
        }
        frames = output;
    }
    Ok(frames)
}

fn write_frames(
    output: &mut impl Write,
    count: &mut u64,
    frames: Vec<BodyFrame>,
) -> Result<(), AppError> {
    for frame in frames {
        if let BodyFrame::Data(bytes) = frame {
            *count = count.saturating_add(bytes.len() as u64);
            if *count > MAX_FILE_BYTES {
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "Decoded response exceeds the one-GiB file limit",
                    false,
                ));
            }
            output
                .write_all(&bytes)
                .map_err(|_| unavailable("Response file write failed"))?;
        }
    }
    Ok(())
}

fn decode_error(error: &transmog_content::ContentCodecError) -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        format!("Response content decoding failed: {error}"),
        false,
    )
}
fn unavailable(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppConfig, Application, BodyStoreConfig};
    use transmog_core::{
        ClientIdentity, ConnectionId, HttpLegVersion, RequestHead, ResponseHead, SessionId,
        SessionMetadata, StreamId, Target,
        intercept::{CompletedExchange, ExchangeMetadata},
        observe::{ObservedBodyChunk, Observer, ObserverEvent, ObserverEventKind},
    };

    fn metadata(length: usize, codings: Vec<String>) -> StoredBodyMetadata {
        StoredBodyMetadata {
            length_known: true,
            exchange_id: format!("{:032x}", 1),
            boundary: "client-response",
            observed_bytes: length as u64,
            retained_bytes: length as u64,
            availability: BodyAvailability::Complete,
            media_type: Some("image/webp".to_owned()),
            charset: None,
            content_codings: codings,
            sha256: None,
            reason: None,
        }
    }

    #[tokio::test]
    async fn saves_exact_original_bytes_across_codings_and_chunks() {
        let root = tempfile::tempdir().unwrap();
        let mut entropy = 0x1234_5678_u32;
        let payload: Vec<u8> = (0..128 * 1024)
            .map(|_| {
                entropy ^= entropy << 13;
                entropy ^= entropy >> 17;
                entropy ^= entropy << 5;
                u8::try_from(entropy & 255).unwrap()
            })
            .collect();
        for codings in [
            vec![],
            vec!["gzip"],
            vec!["deflate"],
            vec!["br"],
            vec!["zstd"],
            vec!["gzip", "br"],
        ] {
            let codings: Vec<String> = codings.into_iter().map(str::to_owned).collect();
            let encoded = crate::inspector::encode_content(&codings, payload.clone())
                .await
                .unwrap();
            let response = ResponseFile {
                name: "original.webp".to_owned(),
                metadata: metadata(encoded.len(), codings),
                reader: Box::new(std::io::Cursor::new(encoded)),
            };
            let result = response
                .save_to(root.path().join("original.webp"))
                .await
                .unwrap();
            assert_eq!(result.file_name, "original.webp");
            assert_eq!(result.bytes, payload.len() as u64);
            assert_eq!(
                std::fs::read(root.path().join("original.webp")).unwrap(),
                payload
            );
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_failed_decode_or_read_keeps_the_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("response.txt");
        std::fs::write(&destination, b"existing file").unwrap();
        for metadata in [metadata(7, vec!["gzip".to_owned()]), metadata(8, vec![])] {
            let response = ResponseFile {
                name: "response.txt".to_owned(),
                metadata,
                reader: Box::new(std::io::Cursor::new(b"invalid".to_vec())),
            };
            assert!(response.save_to(destination.clone()).await.is_err());
            assert_eq!(std::fs::read(&destination).unwrap(), b"existing file");
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }

    async fn captured_response(application: &Application, sequence_gap: bool) {
        let target = Target {
            scheme: "https".to_owned(),
            authority: "example.test".to_owned(),
            host: "example.test".to_owned(),
            port: 443,
            path: "/download.php".to_owned(),
            query: None,
        };
        let metadata = std::sync::Arc::new(ExchangeMetadata::from_session_at(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                client_identity: ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: Some(HttpLegVersion::Http2),
            },
            target.clone(),
            std::time::SystemTime::UNIX_EPOCH,
        ));
        let head = ResponseHead {
            status: 200,
            source_version: HttpLegVersion::Http2,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("content-type", "image/webp").unwrap(),
                HeaderField::try_new(
                    "content-disposition",
                    "inline;filename*=UTF-8''Jack_of_the_United_States.svg.webp",
                )
                .unwrap(),
            ]),
        };
        let gap = u64::from(sequence_gap);
        for (sequence, kind) in [
            (
                1,
                ObserverEventKind::ExchangeStarted {
                    metadata: metadata.clone(),
                },
            ),
            (
                2,
                ObserverEventKind::ResponseHeadObserved {
                    boundary: ExchangeBoundary::ClientResponse,
                    head: head.clone(),
                },
            ),
            (
                3 + gap,
                ObserverEventKind::BodyChunk(ObservedBodyChunk {
                    boundary: ExchangeBoundary::ClientResponse,
                    byte_count: 4,
                    sample: Some(Bytes::from_static(b"body")),
                    truncated: false,
                }),
            ),
            (
                4 + gap,
                ObserverEventKind::Completed(CompletedExchange {
                    metadata,
                    request_head: RequestHead {
                        method: "GET".to_owned(),
                        target,
                        source_version: HttpLegVersion::Http2,
                        headers: HeaderBlock::default(),
                    },
                    response_head: head,
                }),
            ),
        ] {
            let event = ObserverEvent {
                exchange_id: ExchangeId(1),
                sequence,
                kind,
            };
            application.service.catalog().apply(event.clone());
            application
                .body_store()
                .unwrap()
                .on_event(event)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn preparation_uses_original_headers_and_protects_bytes_until_save_or_cancel() {
        let root = tempfile::tempdir().unwrap();
        let application = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        })
        .unwrap();
        captured_response(&application, false).await;
        let response = application
            .prepare_response_file(&format!("{:032x}", 1), "client-response")
            .unwrap();
        assert_eq!(
            response.suggested_name(),
            "Jack_of_the_United_States.svg.webp"
        );
        assert_eq!(
            application.body_store().unwrap().purge_terminal().unwrap(),
            0
        );
        response
            .save_to(root.path().join("saved.webp"))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("saved.webp")).unwrap(),
            b"body"
        );
        let cancelled = application
            .prepare_response_file(&format!("{:032x}", 1), "client-response")
            .unwrap();
        drop(cancelled);
        assert_eq!(
            application.body_store().unwrap().purge_terminal().unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn incomplete_bodies_are_rejected_before_a_destination_is_requested() {
        let root = tempfile::tempdir().unwrap();
        let application = Application::new(AppConfig {
            body_store: Some(BodyStoreConfig::product_default(root.path().join("cache"))),
            ..AppConfig::default()
        })
        .unwrap();
        captured_response(&application, true).await;
        match application.prepare_response_file(&format!("{:032x}", 1), "client-response") {
            Err(error) => assert!(error.message.contains("observer delivery sequence gap")),
            Ok(_) => panic!("incomplete body was accepted for saving"),
        }
    }
}
