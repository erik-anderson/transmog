use thiserror::Error;
use transmog_core::{
    BodyFrame, HeaderBlock,
    intercept::{BodyPipeline, BodyPipelineError, BodyRepresentation, BodyRepresentationError},
};

use crate::{
    ContentBudget, ContentCodecError, ContentCodingError, ContentCodingStack, ContentDecoder,
    ContentDecoderOptions, ContentEncoder, ContentLength, ContentLimitError, ContentLimits,
    ContentMode, ContentOutput, ContentPlan, ContentPolicy, plan::repaired_raw_output_headers,
};

/// A Hooks v2 body pipeline wrapped in bounded content decoding and encoding.
///
/// Construction resolves the aggregate representation requirement before any
/// bytes are processed. Neutral pipelines retain exact coded bytes, raw
/// pipelines never invoke codecs, and decoded pipelines reverse the source
/// coding stack before hooks and optionally restore it afterward.
#[derive(Debug)]
pub struct ContentBodyPipeline {
    body: BodyPipeline,
    plan: Option<ContentPlan>,
    decoders: DecoderStack,
    encoders: EncoderStack,
    budget: ContentBudget,
    hook_headers: HeaderBlock,
    output_headers: HeaderBlock,
    decoded_source_frames: bool,
    terminal: bool,
}

impl ContentBodyPipeline {
    /// Constructs a pipeline from an embedding application's listener policy.
    ///
    /// Disabled policy preserves coded bodies exactly when every semantic hook
    /// is optional. A required semantic hook instead fails before consuming the
    /// body. Identity bodies remain available to semantic hooks because no
    /// decoding capability is required.
    ///
    /// # Errors
    ///
    /// Returns a typed policy, coding, codec, or representation failure.
    pub fn from_policy(
        mut body: BodyPipeline,
        source_headers: &HeaderBlock,
        policy: ContentPolicy,
    ) -> Result<Self, ContentPipelineError> {
        if policy.mode() != ContentMode::Disabled {
            let output = match policy.mode() {
                ContentMode::InspectToIdentity => ContentOutput::Identity,
                ContentMode::PreserveOriginalOutput => ContentOutput::PreserveOriginal,
                ContentMode::Disabled => unreachable!("disabled mode handled above"),
            };
            return Self::new(
                body,
                source_headers,
                output,
                policy.limits(),
                policy.decoder_options(),
            );
        }

        match body.representation() {
            BodyRepresentation::Neutral | BodyRepresentation::Raw => Self::new(
                body,
                source_headers,
                ContentOutput::Identity,
                policy.limits(),
                policy.decoder_options(),
            ),
            BodyRepresentation::DecodedRequired | BodyRepresentation::DecodedIfSupported => {
                let optional = body.representation() == BodyRepresentation::DecodedIfSupported;
                match ContentCodingStack::from_headers(
                    source_headers,
                    policy.limits().max_coding_layers(),
                ) {
                    Ok(stack) if stack.is_empty() => Self::new(
                        body,
                        source_headers,
                        ContentOutput::Identity,
                        policy.limits(),
                        policy.decoder_options(),
                    ),
                    Ok(_) | Err(ContentCodingError::Unsupported(_)) if optional => {
                        body.decline_optional_decoded()?;
                        Ok(Self::direct(body, source_headers, policy.limits()))
                    }
                    Ok(_) => Err(ContentPipelineError::DecodingDisabled),
                    Err(error) => Err(error.into()),
                }
            }
        }
    }

    /// Resolves content policy and constructs one body pipeline.
    ///
    /// Unsupported coding disables a wholly optional decoded pipeline and
    /// preserves exact bytes. Required decoding, malformed coding syntax, and
    /// incompatible codec configuration fail before body processing starts.
    ///
    /// # Errors
    ///
    /// Returns a typed coding, codec, or representation-policy failure.
    pub fn new(
        mut body: BodyPipeline,
        source_headers: &HeaderBlock,
        output: ContentOutput,
        limits: ContentLimits,
        decoder_options: ContentDecoderOptions,
    ) -> Result<Self, ContentPipelineError> {
        let requirement = body.representation();
        if requirement == BodyRepresentation::Neutral {
            return Ok(Self::direct(body, source_headers, limits));
        }
        if requirement == BodyRepresentation::Raw {
            let output_headers = if body.modifies_body() {
                repaired_raw_output_headers(source_headers)
            } else {
                source_headers.clone()
            };
            return Ok(Self {
                body,
                plan: None,
                decoders: DecoderStack::default(),
                encoders: EncoderStack::default(),
                budget: ContentBudget::new(limits),
                hook_headers: source_headers.clone(),
                output_headers,
                decoded_source_frames: false,
                terminal: false,
            });
        }

        let plan = match ContentPlan::for_body_pipeline(
            source_headers,
            output,
            limits,
            body.modifies_body(),
        ) {
            Ok(plan) => plan,
            Err(ContentCodingError::Unsupported(_))
                if requirement == BodyRepresentation::DecodedIfSupported =>
            {
                body.decline_optional_decoded()?;
                return Ok(Self::direct(body, source_headers, limits));
            }
            Err(error) => return Err(error.into()),
        };

        let decoded_source_frames = plan.original().is_empty() || body.inspects_source_body();
        let decoders = if plan.original().is_empty() || !body.inspects_source_body() {
            DecoderStack::default()
        } else {
            DecoderStack::new(&plan, limits, decoder_options)?
        };
        let encoders = match output {
            ContentOutput::Identity => EncoderStack::default(),
            ContentOutput::PreserveOriginal => EncoderStack::new(&plan, limits)?,
        };
        let hook_headers = plan.headers_for_decoded_hooks(source_headers);
        let output_headers = plan.headers_for_output(&hook_headers, ContentLength::Streaming);

        Ok(Self {
            body,
            plan: Some(plan),
            decoders,
            encoders,
            budget: ContentBudget::new(limits),
            hook_headers,
            output_headers,
            decoded_source_frames,
            terminal: false,
        })
    }

    fn direct(body: BodyPipeline, source_headers: &HeaderBlock, limits: ContentLimits) -> Self {
        Self {
            body,
            plan: None,
            decoders: DecoderStack::default(),
            encoders: EncoderStack::default(),
            budget: ContentBudget::new(limits),
            hook_headers: source_headers.clone(),
            output_headers: source_headers.clone(),
            decoded_source_frames: false,
            terminal: false,
        }
    }

    /// Head fields presented alongside body events.
    pub const fn hook_headers(&self) -> &HeaderBlock {
        &self.hook_headers
    }

    /// Streaming output head fields, repaired before head commitment.
    pub const fn output_headers(&self) -> &HeaderBlock {
        &self.output_headers
    }

    /// Final aggregate representation after optional-policy resolution.
    pub const fn representation(&self) -> BodyRepresentation {
        self.body.representation()
    }

    /// Whether body bytes or representation metadata can change.
    pub fn modifies_body(&self) -> bool {
        self.body.modifies_body()
            || self
                .plan
                .as_ref()
                .is_some_and(ContentPlan::transforms_representation)
    }

    /// Processes one canonical source frame.
    ///
    /// # Errors
    ///
    /// Fails closed on invalid sequencing, coding, resource limits, hook
    /// failures, or use after a terminal result.
    pub async fn process(
        &mut self,
        frame: BodyFrame,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        self.ensure_active()?;
        if self.plan.is_some()
            && let BodyFrame::Data(bytes) = &frame
        {
            self.budget.record_encoded(bytes.len())?;
        }
        let result = self.process_inner(frame).await;
        if result.is_err() {
            self.terminal = true;
        }
        result
    }

    /// Completes every decoder, hook stage, and encoder in dependency order.
    ///
    /// # Errors
    ///
    /// Fails closed on truncation, invalid sequencing, resource limits, hook
    /// failures, or repeated completion.
    pub async fn finish(&mut self) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        self.ensure_active()?;
        self.terminal = true;

        let decoded = self.decoders.finish().await?;
        let mut output = self.process_hook_input(decoded, true).await?;
        let body_output = self.body.finish().await?;
        output.extend(self.process_hook_output(body_output).await?);
        let encoded = self.encoders.finish().await?;
        output.extend(self.record_output(encoded)?);
        Ok(output)
    }

    async fn process_inner(
        &mut self,
        frame: BodyFrame,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        if self.decoders.is_empty() {
            self.process_hook_input(vec![frame], self.decoded_source_frames)
                .await
        } else {
            let decoded = self.decoders.process(frame).await?;
            self.process_hook_input(decoded, true).await
        }
    }

    async fn process_hook_input(
        &mut self,
        frames: Vec<BodyFrame>,
        decoded: bool,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        let mut output = Vec::new();
        for mut frame in frames {
            if decoded {
                if let BodyFrame::Data(bytes) = &frame {
                    self.budget.record_decoded(bytes.len())?;
                }
                self.repair_trailers(&mut frame);
            }
            output.extend(self.body.process(frame).await?);
        }
        self.process_hook_output(output).await
    }

    async fn process_hook_output(
        &mut self,
        mut frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        if self.plan.is_some() {
            for frame in &mut frames {
                self.repair_trailers(frame);
            }
        }
        let encoded = self.encoders.process(frames).await?;
        self.record_output(encoded)
    }

    fn repair_trailers(&self, frame: &mut BodyFrame) {
        let (Some(plan), BodyFrame::Trailers(trailers)) = (&self.plan, frame) else {
            return;
        };
        *trailers = plan.repaired_trailers(trailers);
    }

    fn record_output(
        &mut self,
        frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        if self.plan.is_some() {
            for frame in &frames {
                if let BodyFrame::Data(bytes) = frame {
                    self.budget.record_output(bytes.len())?;
                }
            }
        }
        Ok(frames)
    }

    fn ensure_active(&self) -> Result<(), ContentPipelineError> {
        if self.terminal {
            Err(ContentPipelineError::AlreadyFinished)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Default)]
struct DecoderStack(Vec<ContentDecoder>);

impl DecoderStack {
    fn new(
        plan: &ContentPlan,
        limits: ContentLimits,
        options: ContentDecoderOptions,
    ) -> Result<Self, ContentCodecError> {
        plan.original()
            .decode_order()
            .map(|coding| ContentDecoder::with_options(coding, limits, options))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    async fn process(&mut self, frame: BodyFrame) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.process_from(0, vec![frame]).await
    }

    async fn finish(&mut self) -> Result<Vec<BodyFrame>, ContentCodecError> {
        let mut output = Vec::new();
        for index in 0..self.0.len() {
            let frames = self.0[index].finish().await?;
            output.extend(self.process_from(index + 1, frames).await?);
        }
        Ok(output)
    }

    async fn process_from(
        &mut self,
        start: usize,
        mut frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentCodecError> {
        for decoder in &mut self.0[start..] {
            let mut output = Vec::new();
            for frame in frames {
                output.extend(decoder.on_frame(frame).await?);
            }
            frames = output;
        }
        Ok(frames)
    }
}

#[derive(Debug, Default)]
struct EncoderStack(Vec<ContentEncoder>);

impl EncoderStack {
    fn new(plan: &ContentPlan, limits: ContentLimits) -> Result<Self, ContentCodecError> {
        plan.original()
            .encode_order()
            .map(|coding| ContentEncoder::new(coding, limits))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    async fn process(
        &mut self,
        frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentCodecError> {
        self.process_from(0, frames).await
    }

    async fn finish(&mut self) -> Result<Vec<BodyFrame>, ContentCodecError> {
        let mut output = Vec::new();
        for index in 0..self.0.len() {
            let frames = self.0[index].finish().await?;
            output.extend(self.process_from(index + 1, frames).await?);
        }
        Ok(output)
    }

    async fn process_from(
        &mut self,
        start: usize,
        mut frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentCodecError> {
        for encoder in &mut self.0[start..] {
            let mut output = Vec::new();
            for frame in frames {
                output.extend(encoder.on_frame(frame).await?);
            }
            frames = output;
        }
        Ok(frames)
    }
}

/// Failure constructing or running a content-aware body pipeline.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContentPipelineError {
    /// A required semantic hook encountered a coded body while decoding was disabled.
    #[error("content decoding is disabled but a hook requires decoded bytes")]
    DecodingDisabled,
    /// Selected body plans had an invalid representation policy.
    #[error(transparent)]
    Representation(#[from] BodyRepresentationError),
    /// The content-coding header was malformed, unsupported, or too deep.
    #[error(transparent)]
    Coding(#[from] ContentCodingError),
    /// A codec rejected data or configuration.
    #[error(transparent)]
    Codec(#[from] ContentCodecError),
    /// Hooks v2 planning or body processing failed.
    #[error(transparent)]
    Body(#[from] BodyPipelineError),
    /// A whole-pipeline content budget was exceeded.
    #[error(transparent)]
    Limit(#[from] ContentLimitError),
    /// The content pipeline was used after completion or failure.
    #[error("content body pipeline is already terminal")]
    AlreadyFinished,
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroUsize,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use bytes::Bytes;
    use transmog_core::{
        ConnectionId, HeaderField, HttpLegVersion, RequestHead, SessionId, SessionMetadata,
        StreamId, Target,
        intercept::{
            BodyFilter, BodyHookError, BodyPipelineLimits, BodyPlan, BoxBodyFuture, BoxHookFuture,
            BufferedBody, ExchangeInterceptor, ExchangeMetadata, HookInitError, HookLimits,
            InterceptorChainFactory, InterceptorFactory, InterceptorRegistration,
            InterceptorRequirement, RequestBodyAction, RequestBodyEvent, RequestHeadOutcome,
        },
    };

    use super::*;
    use crate::{ContentCoding, ContentWorkLimits};

    #[derive(Clone)]
    enum TestPlan {
        PassThrough,
        RawPrefix,
        DecodedPrefix,
        OptionalPrefix(Arc<AtomicUsize>),
        Replace(Bytes),
    }

    struct TestFactory(TestPlan);

    impl InterceptorFactory for TestFactory {
        fn create(
            &self,
            _metadata: &ExchangeMetadata,
        ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
            Ok(Arc::new(TestInterceptor(self.0.clone())))
        }
    }

    struct TestInterceptor(TestPlan);

    impl ExchangeInterceptor for TestInterceptor {
        fn on_request_body(
            &self,
            _event: RequestBodyEvent,
        ) -> BoxHookFuture<'_, RequestBodyAction> {
            let action = match &self.0 {
                TestPlan::PassThrough => RequestBodyAction::pass_through(),
                TestPlan::RawPrefix => {
                    RequestBodyAction::raw(BodyPlan::Transform(Box::new(PrefixOnce::default())))
                }
                TestPlan::DecodedPrefix => {
                    RequestBodyAction::decoded(BodyPlan::Transform(Box::new(PrefixOnce::default())))
                }
                TestPlan::OptionalPrefix(calls) => RequestBodyAction::decoded_if_supported(
                    BodyPlan::Transform(Box::new(PrefixOnce {
                        seen: false,
                        calls: Some(Arc::clone(calls)),
                    })),
                ),
                TestPlan::Replace(bytes) => RequestBodyAction::decoded(BodyPlan::Replace(
                    BufferedBody::try_new(bytes.len(), bytes.clone(), None).unwrap(),
                )),
            };
            Box::pin(async move { action })
        }
    }

    #[derive(Default)]
    struct PrefixOnce {
        seen: bool,
        calls: Option<Arc<AtomicUsize>>,
    }

    impl BodyFilter for PrefixOnce {
        fn on_frame(
            &mut self,
            frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            if let Some(calls) = &self.calls {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            Box::pin(async move {
                match frame {
                    BodyFrame::Data(bytes) if !bytes.is_empty() && !self.seen => {
                        self.seen = true;
                        let mut output = Vec::with_capacity(bytes.len() + 1);
                        output.push(b'x');
                        output.extend_from_slice(&bytes);
                        Ok(vec![BodyFrame::Data(Bytes::from(output))])
                    }
                    frame => Ok(vec![frame]),
                }
            })
        }
    }

    fn metadata() -> ExchangeMetadata {
        ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(2),
                stream_id: StreamId(3),
                client_addr: "127.0.0.1:1000".parse().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:2000".parse().unwrap(),
                ingress_version: HttpLegVersion::Http2,
                egress_version: None,
            },
            Target {
                scheme: "https".to_owned(),
                authority: "example.test".to_owned(),
                host: "example.test".to_owned(),
                port: 443,
                path: "/".to_owned(),
                query: None,
            },
        )
    }

    fn request(headers: HeaderBlock) -> RequestHead {
        RequestHead {
            method: "POST".to_owned(),
            target: metadata().original_target.as_target().clone(),
            headers,
            source_version: HttpLegVersion::Http2,
        }
    }

    async fn body_pipeline(headers: HeaderBlock, plan: TestPlan) -> BodyPipeline {
        let factory = InterceptorChainFactory::new(
            vec![InterceptorRegistration::new(
                "content-test",
                Arc::new(TestFactory(plan)),
                InterceptorRequirement::Required,
            )],
            HookLimits::default(),
        );
        let mut chain = factory.create_exchange(metadata()).unwrap();
        let request = request(headers);
        let RequestHeadOutcome::Continue { head, .. } = chain.request_head(request).await.unwrap()
        else {
            panic!("test interceptor unexpectedly terminated at request head");
        };
        chain
            .request_body_pipeline(&head, BodyPipelineLimits::default())
            .await
            .unwrap()
    }

    fn headers(coding: &str) -> HeaderBlock {
        HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-encoding", coding).unwrap(),
            HeaderField::try_new("content-length", "123").unwrap(),
            HeaderField::try_new("etag", "\"old\"").unwrap(),
            HeaderField::try_new("x-preserved", "yes").unwrap(),
        ])
    }

    async fn encode_layer(coding: ContentCoding, input: &[u8]) -> Vec<u8> {
        let mut encoder = ContentEncoder::new(coding, ContentLimits::default()).unwrap();
        let mut frames = encoder
            .on_frame(BodyFrame::Data(Bytes::copy_from_slice(input)))
            .await
            .unwrap();
        frames.extend(encoder.finish().await.unwrap());
        data(frames)
    }

    async fn decode_layer(coding: ContentCoding, input: &[u8]) -> Vec<u8> {
        let mut decoder = ContentDecoder::new(coding, ContentLimits::default()).unwrap();
        let mut frames = decoder
            .on_frame(BodyFrame::Data(Bytes::copy_from_slice(input)))
            .await
            .unwrap();
        frames.extend(decoder.finish().await.unwrap());
        data(frames)
    }

    fn data(frames: Vec<BodyFrame>) -> Vec<u8> {
        frames
            .into_iter()
            .filter_map(|frame| match frame {
                BodyFrame::Data(bytes) => Some(bytes),
                BodyFrame::Trailers(_) => None,
            })
            .flatten()
            .collect()
    }

    async fn drive(
        pipeline: &mut ContentBodyPipeline,
        frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, ContentPipelineError> {
        let mut output = Vec::new();
        for frame in frames {
            output.extend(pipeline.process(frame).await?);
        }
        output.extend(pipeline.finish().await?);
        Ok(output)
    }

    #[tokio::test]
    async fn decoded_multilayer_pipeline_uses_reverse_decode_and_forward_encode_order() {
        let gzip = encode_layer(ContentCoding::Gzip, b"hello").await;
        let encoded = encode_layer(ContentCoding::Brotli, &gzip).await;
        let source_headers = headers("gzip, br");
        let body = body_pipeline(source_headers.clone(), TestPlan::DecodedPrefix).await;
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::PreserveOriginal,
            ContentLimits::default(),
            ContentDecoderOptions::default(),
        )
        .unwrap();

        assert!(
            pipeline
                .hook_headers()
                .values("content-encoding")
                .next()
                .is_none()
        );
        assert!(pipeline.hook_headers().values("etag").next().is_none());
        assert_eq!(
            pipeline
                .output_headers()
                .values("content-encoding")
                .collect::<Vec<_>>(),
            [b"gzip, br".as_slice()]
        );

        let trailers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-digest", "sha-256=:stale:").unwrap(),
            HeaderField::try_new("x-trailer", "kept").unwrap(),
        ]);
        let mut input = encoded
            .chunks(1)
            .map(|chunk| BodyFrame::Data(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        input.push(BodyFrame::Trailers(trailers));
        let output = drive(&mut pipeline, input).await.unwrap();

        let final_trailers = output
            .iter()
            .find_map(|frame| match frame {
                BodyFrame::Trailers(trailers) => Some(trailers),
                BodyFrame::Data(_) => None,
            })
            .unwrap();
        assert!(final_trailers.values("content-digest").next().is_none());
        assert_eq!(
            final_trailers.values("x-trailer").collect::<Vec<_>>(),
            [b"kept".as_slice()]
        );

        let output = data(output);
        let brotli_decoded = decode_layer(ContentCoding::Brotli, &output).await;
        let decoded = decode_layer(ContentCoding::Gzip, &brotli_decoded).await;
        assert_eq!(decoded, b"xhello");
    }

    #[tokio::test]
    async fn neutral_pipeline_preserves_headers_frames_and_boundaries() {
        let source_headers = headers("gzip");
        let body = body_pipeline(source_headers.clone(), TestPlan::PassThrough).await;
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::Identity,
            ContentLimits::default(),
            ContentDecoderOptions::default(),
        )
        .unwrap();
        let trailers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-digest", "unchanged").unwrap(),
        ]);
        let input = vec![
            BodyFrame::Data(Bytes::from_static(b"first")),
            BodyFrame::Data(Bytes::from_static(b"second")),
            BodyFrame::Trailers(trailers),
        ];
        let output = drive(&mut pipeline, input.clone()).await.unwrap();
        assert_eq!(output, input);
        assert_eq!(pipeline.hook_headers(), &source_headers);
        assert_eq!(pipeline.output_headers(), &source_headers);
        assert!(!pipeline.modifies_body());
    }

    #[tokio::test]
    async fn raw_pipeline_never_invokes_codecs_and_repairs_stale_output_metadata() {
        let source_headers = headers("gzip");
        let body = body_pipeline(source_headers.clone(), TestPlan::RawPrefix).await;
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::Identity,
            ContentLimits::default(),
            ContentDecoderOptions::default(),
        )
        .unwrap();
        let output = drive(
            &mut pipeline,
            vec![BodyFrame::Data(Bytes::from_static(b"not-valid-gzip"))],
        )
        .await
        .unwrap();
        assert_eq!(data(output), b"xnot-valid-gzip");
        assert_eq!(
            pipeline
                .output_headers()
                .values("content-encoding")
                .collect::<Vec<_>>(),
            [b"gzip".as_slice()]
        );
        assert!(
            pipeline
                .output_headers()
                .values("content-length")
                .next()
                .is_none()
        );
        assert!(pipeline.output_headers().values("etag").next().is_none());
    }

    #[tokio::test]
    async fn unsupported_coding_declines_optional_but_rejects_required_decoding() {
        let source_headers = headers("compress");
        let calls = Arc::new(AtomicUsize::new(0));
        let body = body_pipeline(
            source_headers.clone(),
            TestPlan::OptionalPrefix(Arc::clone(&calls)),
        )
        .await;
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::Identity,
            ContentLimits::default(),
            ContentDecoderOptions::default(),
        )
        .unwrap();
        let input = vec![BodyFrame::Data(Bytes::from_static(b"opaque"))];
        assert_eq!(drive(&mut pipeline, input.clone()).await.unwrap(), input);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(pipeline.output_headers(), &source_headers);

        let required = body_pipeline(source_headers.clone(), TestPlan::DecodedPrefix).await;
        assert!(matches!(
            ContentBodyPipeline::new(
                required,
                &source_headers,
                ContentOutput::Identity,
                ContentLimits::default(),
                ContentDecoderOptions::default(),
            ),
            Err(ContentPipelineError::Coding(
                ContentCodingError::Unsupported(_)
            ))
        ));

        let malformed_headers = headers("gzip,,br");
        let malformed = body_pipeline(
            malformed_headers.clone(),
            TestPlan::OptionalPrefix(Arc::clone(&calls)),
        )
        .await;
        assert!(matches!(
            ContentBodyPipeline::new(
                malformed,
                &malformed_headers,
                ContentOutput::Identity,
                ContentLimits::default(),
                ContentDecoderOptions::default(),
            ),
            Err(ContentPipelineError::Coding(
                ContentCodingError::EmptyElement
            ))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disabled_policy_is_explicit_for_coded_and_identity_bodies() {
        let coded_headers = headers("gzip");
        let required = body_pipeline(coded_headers.clone(), TestPlan::DecodedPrefix).await;
        assert!(matches!(
            ContentBodyPipeline::from_policy(required, &coded_headers, ContentPolicy::disabled()),
            Err(ContentPipelineError::DecodingDisabled)
        ));

        let calls = Arc::new(AtomicUsize::new(0));
        let optional = body_pipeline(
            coded_headers.clone(),
            TestPlan::OptionalPrefix(Arc::clone(&calls)),
        )
        .await;
        let mut optional =
            ContentBodyPipeline::from_policy(optional, &coded_headers, ContentPolicy::disabled())
                .unwrap();
        let opaque = vec![BodyFrame::Data(Bytes::from_static(b"opaque"))];
        assert_eq!(drive(&mut optional, opaque.clone()).await.unwrap(), opaque);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let identity_headers = HeaderBlock::new();
        let identity = body_pipeline(identity_headers.clone(), TestPlan::DecodedPrefix).await;
        let mut identity = ContentBodyPipeline::from_policy(
            identity,
            &identity_headers,
            ContentPolicy::disabled(),
        )
        .unwrap();
        let output = drive(
            &mut identity,
            vec![BodyFrame::Data(Bytes::from_static(b"identity"))],
        )
        .await
        .unwrap();
        assert_eq!(data(output), b"xidentity");
    }

    #[tokio::test]
    async fn inspect_to_identity_policy_removes_coding_after_decoded_hooks() {
        let source_headers = headers("gzip");
        let encoded = encode_layer(ContentCoding::Gzip, b"identity-output").await;
        let body = body_pipeline(source_headers.clone(), TestPlan::DecodedPrefix).await;
        let mut pipeline = ContentBodyPipeline::from_policy(
            body,
            &source_headers,
            ContentPolicy::inspect_to_identity(ContentLimits::default()),
        )
        .unwrap();

        assert!(
            pipeline
                .output_headers()
                .values("content-encoding")
                .next()
                .is_none()
        );
        let output = drive(&mut pipeline, vec![BodyFrame::Data(Bytes::from(encoded))])
            .await
            .unwrap();
        assert_eq!(data(output), b"xidentity-output");
    }

    #[tokio::test]
    async fn replacement_drains_corrupt_encoded_input_without_decoding_it() {
        let source_headers = headers("gzip");
        let body = body_pipeline(
            source_headers.clone(),
            TestPlan::Replace(Bytes::from_static(b"replacement")),
        )
        .await;
        assert!(!body.inspects_source_body());
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::PreserveOriginal,
            ContentLimits::default(),
            ContentDecoderOptions::default(),
        )
        .unwrap();
        let output = drive(
            &mut pipeline,
            vec![BodyFrame::Data(Bytes::from_static(b"not-valid-gzip"))],
        )
        .await
        .unwrap();
        assert_eq!(
            decode_layer(ContentCoding::Gzip, &data(output)).await,
            b"replacement"
        );
    }

    #[tokio::test]
    async fn whole_stack_limits_and_terminal_state_fail_closed() {
        let source_headers = headers("gzip");
        let encoded = encode_layer(ContentCoding::Gzip, &vec![b'a'; 4096]).await;
        let body = body_pipeline(source_headers.clone(), TestPlan::DecodedPrefix).await;
        let limits = ContentLimits::new(
            NonZeroUsize::new(encoded.len()).unwrap(),
            NonZeroUsize::new(4095).unwrap(),
            NonZeroUsize::new(8192).unwrap(),
            NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
            NonZeroUsize::new(1000).unwrap(),
            8192,
            NonZeroUsize::new(4).unwrap(),
        );
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::Identity,
            limits,
            ContentDecoderOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            pipeline
                .process(BodyFrame::Data(Bytes::from(encoded)))
                .await,
            Err(ContentPipelineError::Codec(ContentCodecError::Limit(
                ContentLimitError::DecodedBytes { .. },
            )) | ContentPipelineError::Limit(ContentLimitError::DecodedBytes { .. }))
        ));
        assert_eq!(
            pipeline.finish().await.unwrap_err(),
            ContentPipelineError::AlreadyFinished
        );
    }

    #[tokio::test]
    async fn codec_work_timeout_propagates_and_terminates_the_pipeline() {
        let source_headers = headers("gzip");
        let encoded = encode_layer(ContentCoding::Gzip, &vec![b'a'; 64 * 1024]).await;
        let body = body_pipeline(source_headers.clone(), TestPlan::DecodedPrefix).await;
        let work = ContentWorkLimits::new(
            NonZeroUsize::new(1).unwrap(),
            Duration::from_nanos(1),
            Duration::from_nanos(1),
        )
        .unwrap();
        let mut pipeline = ContentBodyPipeline::new(
            body,
            &source_headers,
            ContentOutput::Identity,
            ContentLimits::default().with_work_limits(work),
            ContentDecoderOptions::default(),
        )
        .unwrap();

        assert_eq!(
            pipeline
                .process(BodyFrame::Data(Bytes::from(encoded)))
                .await,
            Err(ContentPipelineError::Codec(ContentCodecError::Timeout {
                coding: ContentCoding::Gzip
            }))
        );
        assert_eq!(
            pipeline.finish().await,
            Err(ContentPipelineError::AlreadyFinished)
        );
    }
}
