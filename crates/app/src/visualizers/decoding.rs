//! On-demand HTTP decoding above exact frozen body grants, using the existing codecs.
use super::{
    AppError, ErrorCategory, VisualizerSourceInfo,
    evidence::{Resources, Source},
    invalid, unavailable,
};
use bytes::Bytes;
use std::{collections::BTreeMap, num::NonZeroUsize, sync::Arc};
use transmog_content::{
    ContentCodecError, ContentCodingError, ContentCodingStack, ContentDecoder, ContentLimits,
};
use transmog_core::{BodyFrame, HeaderBlock, HeaderField};
use transmog_extensions::{
    BodyAccess, BodyReference, DecodeFailure, DecodedBody, HttpDecodeResult, InputAccess, Span,
    sha256,
};
#[cfg(test)]
mod tests;

pub(super) fn grant(
    headers: &HeaderBlock,
    access: &InputAccess,
) -> Option<Result<Vec<String>, DecodeFailure>> {
    if !access.decode_http_content
        || access.body == BodyAccess::None
        || !access.permits_header("content-encoding")
    {
        return None;
    }
    if headers
        .iter()
        .any(|field| field.name().eq_ignore_ascii_case(b"content-encoding") && field.is_redacted())
    {
        return Some(Err(DecodeFailure::MetadataUnavailable));
    }
    Some(stack(headers).map(|stack| {
        stack
            .encode_order()
            .map(|coding| coding.as_str().to_owned())
            .collect()
    }))
}
fn stack(headers: &HeaderBlock) -> Result<ContentCodingStack, DecodeFailure> {
    ContentCodingStack::from_headers(headers, ContentLimits::default().max_coding_layers()).map_err(
        |error| match error {
            ContentCodingError::Unsupported(_) => DecodeFailure::Unsupported,
            ContentCodingError::TooManyLayers { .. } => DecodeFailure::Limited,
            _ => DecodeFailure::MetadataUnavailable,
        },
    )
}
fn stored_stack(codings: &[String]) -> Result<ContentCodingStack, DecodeFailure> {
    if codings.is_empty() {
        return stack(&HeaderBlock::default());
    }
    let field = HeaderField::try_new("content-encoding", codings.join(", "))
        .map_err(|_| DecodeFailure::MetadataUnavailable)?;
    stack(&HeaderBlock::from_fields(vec![field]))
}

impl Resources {
    pub(super) async fn decode_http(
        self: &Arc<Self>,
        id: &str,
        budget: usize,
    ) -> Result<HttpDecodeResult, AppError> {
        let source = self
            .sources
            .lock()
            .map_err(|_| unavailable())?
            .get(id)
            .cloned()
            .ok_or_else(unavailable)?;
        source.authorize(&self.stopped)?;
        let (available, codings) = coding_grant(&source)?;
        let codings = match codings {
            Ok(codings) => codings,
            Err(reason) => return Ok(HttpDecodeResult::Unavailable { reason }),
        };
        if available != crate::BodyAvailability::Complete {
            return Ok(HttpDecodeResult::Unavailable {
                reason: DecodeFailure::Incomplete,
            });
        }
        let parent = Span {
            source_id: id.into(),
            offset: "0".into(),
            length: source.length().to_string(),
        };
        if codings.is_empty() {
            return Ok(HttpDecodeResult::Ready {
                decoded: DecodedBody {
                    body: BodyReference {
                        source_id: id.into(),
                        length: parent.length.clone(),
                        availability: "complete".into(),
                    },
                    parent,
                    codings,
                },
            });
        }
        let decoded_id = format!(
            "decoded-{}",
            &sha256(format!("http:{id}:{}", codings.join(",")).as_bytes())[..48]
        );
        if let Some(info) = self
            .info
            .lock()
            .map_err(|_| unavailable())?
            .iter()
            .find(|info| info.id == decoded_id)
            && let Some(decoded) = &info.http_decoded
        {
            return Ok(HttpDecodeResult::Ready {
                decoded: decoded.clone(),
            });
        }
        let budget = budget.min(crate::body_store::MAX_BODY_READ_BYTES);
        if source.length() > budget as u64 {
            return Ok(HttpDecodeResult::Unavailable {
                reason: DecodeFailure::Limited,
            });
        }
        let original_info = self
            .info
            .lock()
            .map_err(|_| unavailable())?
            .iter()
            .find(|info| info.id == id)
            .cloned()
            .ok_or_else(unavailable)?;
        let output_budget = self.decoded_budget(budget)?;
        if output_budget == 0 {
            return Ok(HttpDecodeResult::Unavailable {
                reason: DecodeFailure::Limited,
            });
        }
        let input = read_encoded(self, &source).await?;
        let bytes = match decode(&codings, input, budget, output_budget).await {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(HttpDecodeResult::Unavailable { reason }),
        };
        source.authorize(&self.stopped)?;
        let decoded = DecodedBody {
            body: BodyReference {
                source_id: decoded_id.clone(),
                length: bytes.len().to_string(),
                availability: "complete".into(),
            },
            parent,
            codings,
        };
        self.retain_decoded(source, original_info, decoded, bytes, budget)
    }
    fn decoded_budget(&self, budget: usize) -> Result<usize, AppError> {
        let sources = self.sources.lock().map_err(|_| unavailable())?;
        if sources.len() >= 4096 {
            return Ok(0);
        }
        Ok(budget.saturating_sub(
            sources
                .values()
                .map(|source| source.retained_bytes())
                .fold(0usize, usize::saturating_add),
        ))
    }
    fn retain_decoded(
        &self,
        source: Arc<Source>,
        original_info: VisualizerSourceInfo,
        decoded: DecodedBody,
        bytes: Vec<u8>,
        budget: usize,
    ) -> Result<HttpDecodeResult, AppError> {
        let info = VisualizerSourceInfo {
            id: decoded.body.source_id.clone(),
            name: format!("{} · HTTP decoded", original_info.name),
            length: decoded.body.length.clone(),
            availability: "complete".into(),
            session_id: original_info.session_id,
            boundary: original_info.boundary,
            http_decoded: Some(decoded.clone()),
        };
        let result = self.append(
            BTreeMap::from([(
                decoded.body.source_id.clone(),
                Source::Decoded {
                    bytes: Arc::from(bytes),
                    original: source,
                },
            )]),
            vec![info],
            budget,
        );
        match result {
            Ok(()) => Ok(HttpDecodeResult::Ready { decoded }),
            Err(error) if error.category == ErrorCategory::Limit => {
                Ok(HttpDecodeResult::Unavailable {
                    reason: DecodeFailure::Limited,
                })
            }
            Err(error) => Err(error),
        }
    }
}
async fn read_encoded(
    resources: &Arc<Resources>,
    source: &Arc<Source>,
) -> Result<Vec<u8>, AppError> {
    let retained = Arc::clone(resources);
    let input_source = Arc::clone(source);
    tokio::task::spawn_blocking(move || {
        let length = usize::try_from(input_source.length())
            .map_err(|_| invalid("Invalid encoded body extent"))?;
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            let part = input_source.read(
                bytes.len() as u64,
                (length - bytes.len()).min(64 * 1024),
                &retained.stopped,
            )?;
            bytes.extend_from_slice(&part);
        }
        input_source.authorize(&retained.stopped)?;
        Ok::<_, AppError>(bytes)
    })
    .await
    .map_err(|_| unavailable())?
}
fn coding_grant(
    source: &Source,
) -> Result<(crate::BodyAvailability, Result<Vec<String>, DecodeFailure>), AppError> {
    match source {
        Source::Related { source, .. } => coding_grant(source),
        Source::Body {
            metadata,
            decoding: Some(grant),
            ..
        } => {
            let grant = grant.clone().and_then(|codings| {
                let stored = stored_stack(&metadata.content_codings)?;
                if stored
                    .encode_order()
                    .map(transmog_content::ContentCoding::as_str)
                    .eq(codings.iter().map(String::as_str))
                {
                    Ok(codings)
                } else {
                    Err(DecodeFailure::MetadataUnavailable)
                }
            });
            Ok((metadata.availability, grant))
        }
        _ => Err(invalid("HTTP decoding was not declared for this body")),
    }
}
fn codec_failure(error: &ContentCodecError) -> DecodeFailure {
    match error {
        ContentCodecError::Limit(_)
        | ContentCodecError::DecoderWindowTooSmall { .. }
        | ContentCodecError::ResourceExhausted { .. }
        | ContentCodecError::Timeout { .. } => DecodeFailure::Limited,
        _ => DecodeFailure::Malformed,
    }
}
async fn decode(
    codings: &[String],
    encoded: Vec<u8>,
    budget: usize,
    output_budget: usize,
) -> Result<Vec<u8>, DecodeFailure> {
    let defaults = ContentLimits::default();
    let bytes = NonZeroUsize::new(budget).ok_or(DecodeFailure::Limited)?;
    let output = NonZeroUsize::new(output_budget).ok_or(DecodeFailure::Limited)?;
    let limits = ContentLimits::new(
        bytes,
        output,
        output,
        defaults.max_decoder_window_bytes(),
        defaults.max_expansion_ratio(),
        defaults.expansion_slack_bytes(),
        defaults.max_coding_layers(),
    )
    .with_work_limits(defaults.work_limits());
    let stack = stored_stack(codings)?;
    let mut current = encoded;
    for coding in stack.decode_order() {
        let mut codec =
            ContentDecoder::new(coding, limits).map_err(|error| codec_failure(&error))?;
        let mut frames = codec
            .on_frame(BodyFrame::Data(Bytes::from(current)))
            .await
            .map_err(|error| codec_failure(&error))?;
        frames.extend(
            codec
                .finish()
                .await
                .map_err(|error| codec_failure(&error))?,
        );
        current = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                current.extend_from_slice(&bytes);
            }
        }
    }
    Ok(current)
}
