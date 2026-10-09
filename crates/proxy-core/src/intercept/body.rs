use std::{future::Future, num::NonZeroUsize, pin::Pin};

use bytes::Bytes;
use thiserror::Error;

use crate::{
    BodyChannelClosed, BodyFrame, BodyLimitError, BodyStream, BodyStreamError, BodyStreamSender,
    BoundedBodyBuffer, HeaderBlock,
};

use super::{HookContext, HookExecutionError, chain::CallbackGate};

/// Representation of body bytes required by one hook plan.
///
/// Transfer framing has already been removed in every case. This requirement
/// concerns only HTTP content codings such as gzip or Brotli.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BodyRepresentation {
    /// The plan does not inspect, replace, discard, or transform body bytes.
    #[default]
    Neutral,
    /// The plan deliberately operates on the content-coded byte stream.
    Raw,
    /// The plan requires decoded identity representation bytes.
    DecodedRequired,
    /// The plan uses decoded bytes when every coding is supported, otherwise
    /// it is omitted without changing the body.
    DecodedIfSupported,
}

impl BodyRepresentation {
    fn combine(self, next: Self) -> Result<Self, BodyRepresentationError> {
        use BodyRepresentation::{DecodedIfSupported, DecodedRequired, Neutral, Raw};
        match (self, next) {
            (Neutral, value) | (value, Neutral) => Ok(value),
            (Raw, Raw) => Ok(Raw),
            (DecodedIfSupported, DecodedIfSupported) => Ok(DecodedIfSupported),
            (DecodedRequired, DecodedRequired | DecodedIfSupported)
            | (DecodedIfSupported, DecodedRequired) => Ok(DecodedRequired),
            (Raw, DecodedRequired | DecodedIfSupported)
            | (DecodedRequired | DecodedIfSupported, Raw) => {
                Err(BodyRepresentationError::RawDecodedConflict)
            }
        }
    }
}

/// Invalid representation requirements selected for one body pipeline.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyRepresentationError {
    /// A pass-through plan claimed a representation it never observes.
    #[error("pass-through body plans must use the neutral representation")]
    NonNeutralPassThrough,
    /// A body-changing plan omitted the representation of its output.
    #[error("body-changing plans must declare raw or decoded representation bytes")]
    NeutralBodyChange,
    /// Raw and decoded stages cannot be composed without an explicit conversion.
    #[error("raw and decoded body plans cannot be composed in one pipeline")]
    RawDecodedConflict,
    /// Optional decoded stages can only be declined when no required decoded
    /// stage is present.
    #[error("the body pipeline does not contain only optional decoded stages")]
    OptionalDecodedNotDeclinable,
}

/// Boxed asynchronous body-filter result.
pub type BoxBodyFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reason an interceptor intentionally terminates one exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookAbort {
    /// Policy-specific reason safe for operator display.
    Policy(String),
    /// Interceptor declined the exchange without a more specific reason.
    Rejected,
}

/// Bounded complete body passed to a buffering hook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BufferedBody {
    /// Concatenated data frames.
    data: Bytes,
    /// Optional terminal trailers.
    trailers: Option<HeaderBlock>,
}

impl BufferedBody {
    /// Creates a replacement body after validating its exact byte bound.
    ///
    /// # Errors
    ///
    /// Returns [`BodyLimitError::LimitExceeded`] when `data` is larger than
    /// `limit`.
    pub fn try_new(
        limit: usize,
        data: Bytes,
        trailers: Option<HeaderBlock>,
    ) -> Result<Self, BodyLimitError> {
        if data.len() > limit {
            return Err(BodyLimitError::LimitExceeded {
                limit,
                attempted: data.len(),
            });
        }
        Ok(Self { data, trailers })
    }

    /// Validates and collects canonical frames under `limit`.
    ///
    /// # Errors
    ///
    /// Returns [`BodyLimitError`] for overflow or invalid trailer ordering.
    pub fn try_from_frames(
        limit: usize,
        frames: impl IntoIterator<Item = BodyFrame>,
    ) -> Result<Self, BodyLimitError> {
        let mut buffer = BoundedBodyBuffer::new(limit);
        for frame in frames {
            buffer.push(frame)?;
        }
        let (data, trailers) = buffer.finish();
        Ok(Self { data, trailers })
    }

    /// Complete body bytes.
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// Optional terminal trailers.
    pub fn trailers(&self) -> Option<&HeaderBlock> {
        self.trailers.as_ref()
    }

    /// Converts the complete body back to canonical frames.
    pub fn into_frames(self) -> Vec<BodyFrame> {
        let mut frames = Vec::with_capacity(usize::from(self.trailers.is_some()) + 1);
        if !self.data.is_empty() {
            frames.push(BodyFrame::Data(self.data));
        }
        if let Some(trailers) = self.trailers {
            frames.push(BodyFrame::Trailers(trailers));
        }
        frames
    }
}

/// Stateful bounded streaming body transformation.
pub trait BodyFilter: Send {
    /// Processes one canonical body frame.
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>>;

    /// Flushes bounded output after the input stream ends.
    fn finish(&mut self) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Completion callback for an explicitly bounded body buffer.
pub trait BufferedBodyHandler: Send {
    /// Edits, replaces, or rejects one complete bounded body.
    fn on_body(
        &mut self,
        body: BufferedBody,
    ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>>;
}

/// Body behavior selected before a request or response pump starts.
#[must_use = "body plans have no effect unless returned to the exchange engine"]
pub enum BodyPlan {
    /// Forward frames without complete-body buffering.
    PassThrough,
    /// Run one stateful bounded streaming filter.
    Transform(Box<dyn BodyFilter>),
    /// Buffer up to the explicit limit and invoke a completion handler.
    Buffer {
        /// Nonzero complete-body byte limit.
        limit: NonZeroUsize,
        /// Handler invoked after the terminal input frame.
        handler: Box<dyn BufferedBodyHandler>,
    },
    /// Discard the incoming body and emit this already-bounded replacement.
    Replace(BufferedBody),
    /// Consume the incoming body without emitting frames.
    Discard,
}

impl BodyPlan {
    fn validates_representation(
        &self,
        representation: BodyRepresentation,
    ) -> Result<(), BodyRepresentationError> {
        match (self, representation) {
            (Self::PassThrough, BodyRepresentation::Neutral) => Ok(()),
            (Self::PassThrough, _) => Err(BodyRepresentationError::NonNeutralPassThrough),
            (_, BodyRepresentation::Neutral) => Err(BodyRepresentationError::NeutralBodyChange),
            _ => Ok(()),
        }
    }

    fn inspects_input_bytes(&self) -> bool {
        matches!(self, Self::Transform(_) | Self::Buffer { .. })
    }
}

/// One body plan paired with the representation it consumes and produces.
#[must_use = "selected body plans must be composed into a body pipeline"]
pub struct BodyPlanSelection {
    plan: BodyPlan,
    representation: BodyRepresentation,
    measurement_label: Option<std::sync::Arc<str>>,
}

impl BodyPlanSelection {
    /// Creates an explicitly represented plan.
    pub const fn new(plan: BodyPlan, representation: BodyRepresentation) -> Self {
        Self {
            plan,
            representation,
            measurement_label: None,
        }
    }

    /// Creates a neutral pass-through selection.
    pub const fn pass_through() -> Self {
        Self::new(BodyPlan::PassThrough, BodyRepresentation::Neutral)
    }

    /// Required representation for this plan.
    pub const fn representation(&self) -> BodyRepresentation {
        self.representation
    }

    /// Borrows the selected plan.
    pub const fn plan(&self) -> &BodyPlan {
        &self.plan
    }

    pub(crate) fn with_measurement_label(
        mut self,
        direction: &str,
        label: &std::sync::Arc<str>,
    ) -> Self {
        if !matches!(self.plan, BodyPlan::PassThrough) {
            self.measurement_label = Some(std::sync::Arc::from(format!("{direction} · {label}")));
        }
        self
    }
    pub(crate) fn into_parts(self) -> (BodyPlan, BodyRepresentation, Option<std::sync::Arc<str>>) {
        (self.plan, self.representation, self.measurement_label)
    }
}

impl std::fmt::Debug for BodyPlanSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BodyPlanSelection")
            .field("plan", &self.plan)
            .field("representation", &self.representation)
            .field("measurement_label", &self.measurement_label)
            .finish()
    }
}

impl std::fmt::Debug for BodyPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PassThrough => formatter.write_str("PassThrough"),
            Self::Transform(_) => formatter.write_str("Transform(..)"),
            Self::Buffer { limit, .. } => formatter
                .debug_struct("Buffer")
                .field("limit", limit)
                .finish_non_exhaustive(),
            Self::Replace(body) => formatter.debug_tuple("Replace").field(body).finish(),
            Self::Discard => formatter.write_str("Discard"),
        }
    }
}

/// Failure emitted by a body filter or bounded buffer handler.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyHookError {
    /// Hook intentionally aborted the exchange.
    #[error("body hook aborted the exchange: {0:?}")]
    Abort(HookAbort),
    /// Body sequence or configured bound was invalid.
    #[error(transparent)]
    Limit(#[from] BodyLimitError),
    /// Redacted implementation failure.
    #[error("body hook failed: {0}")]
    Failed(String),
}

/// Explicit resource limits applied while composing body plans.
#[derive(Clone, Copy, Debug)]
pub struct BodyPipelineLimits {
    /// Maximum frames one transform invocation or buffered handler may emit.
    pub max_output_frames_per_call: NonZeroUsize,
    /// Maximum data bytes one transform invocation or buffered handler may emit.
    pub max_output_bytes_per_call: NonZeroUsize,
}

impl Default for BodyPipelineLimits {
    fn default() -> Self {
        Self {
            max_output_frames_per_call: NonZeroUsize::new(1_024).expect("1024 is nonzero"),
            max_output_bytes_per_call: NonZeroUsize::new(8 * 1024 * 1024)
                .expect("8 MiB is nonzero"),
        }
    }
}

/// Failure processing one body through a composed plan pipeline.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BodyPipelineError {
    /// A body-planning callback failed before the pipeline was constructed.
    #[error(transparent)]
    Planning(#[from] super::ChainExecutionError),
    /// Selected body plans had invalid or incompatible representations.
    #[error(transparent)]
    Representation(#[from] BodyRepresentationError),
    /// Hook execution timed out, was cancelled, panicked, or is shutting down.
    #[error(transparent)]
    Execution(#[from] HookExecutionError),
    /// A transform or buffering hook returned a typed failure.
    #[error(transparent)]
    Hook(#[from] BodyHookError),
    /// Input or generated frames violated canonical trailer ordering.
    #[error(transparent)]
    InvalidSequence(#[from] BodyLimitError),
    /// A transform invocation emitted more frames than configured.
    #[error("body hook emitted {attempted} frames; limit is {limit}")]
    OutputFrameLimit {
        /// Configured maximum.
        limit: usize,
        /// Attempted frame count.
        attempted: usize,
    },
    /// A transform invocation emitted more bytes than configured.
    #[error("body hook emitted {attempted} bytes; limit is {limit}")]
    OutputByteLimit {
        /// Configured maximum.
        limit: usize,
        /// Attempted byte count.
        attempted: usize,
    },
    /// The source body stream failed.
    #[error("source body stream failed: {0}")]
    Input(BodyStreamError),
    /// The destination body consumer was dropped.
    #[error(transparent)]
    OutputClosed(BodyChannelClosed),
}

#[derive(Debug, Default)]
struct FrameSequence {
    trailers_seen: bool,
}

impl FrameSequence {
    fn accept(&mut self, frame: &BodyFrame) -> Result<(), BodyLimitError> {
        match frame {
            BodyFrame::Data(_) if self.trailers_seen => Err(BodyLimitError::DataAfterTrailers),
            BodyFrame::Trailers(_) if self.trailers_seen => Err(BodyLimitError::DuplicateTrailers),
            BodyFrame::Trailers(_) => {
                self.trailers_seen = true;
                Ok(())
            }
            BodyFrame::Data(_) => Ok(()),
        }
    }
}

enum BodyStageKind {
    Transform(Option<Box<dyn BodyFilter>>),
    Buffer {
        buffer: Option<BoundedBodyBuffer>,
        handler: Option<Box<dyn BufferedBodyHandler>>,
    },
    Replace(Option<BufferedBody>),
    Discard,
}

impl std::fmt::Debug for BodyStageKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transform(_) => formatter.write_str("Transform(..)"),
            Self::Buffer { buffer, .. } => formatter
                .debug_struct("Buffer")
                .field(
                    "buffered_bytes",
                    &buffer.as_ref().map(BoundedBodyBuffer::len),
                )
                .finish_non_exhaustive(),
            Self::Replace(body) => formatter.debug_tuple("Replace").field(body).finish(),
            Self::Discard => formatter.write_str("Discard"),
        }
    }
}

#[derive(Debug)]
struct BodyStage {
    measurement_label: Option<std::sync::Arc<str>>,
    kind: BodyStageKind,
    representation: BodyRepresentation,
    output_sequence: FrameSequence,
}

/// Protocol-neutral body-plan composition engine.
///
/// Pipelines are created by [`super::ExchangeChain`] so body callbacks share
/// the chain's global pause-permit pool. Pass-through frames are never
/// whole-body buffered.
#[derive(Debug)]
pub struct BodyPipeline {
    stages: Vec<BodyStage>,
    input_sequence: FrameSequence,
    context: HookContext,
    measurement_scope: &'static str,
    gate: CallbackGate,
    limits: BodyPipelineLimits,
    representation: BodyRepresentation,
    inspects_source_body: bool,
    modifies_body: bool,
    finished: bool,
}

impl BodyPipeline {
    pub(super) fn from_plans(
        plans: Vec<BodyPlanSelection>,
        context: HookContext,
        gate: CallbackGate,
        limits: BodyPipelineLimits,
    ) -> Result<Self, BodyRepresentationError> {
        let mut representation = BodyRepresentation::Neutral;
        let mut modifies_body = false;
        let mut inspects_source_body = false;
        let mut source_body_visible = true;
        let mut stages = Vec::new();
        for selection in plans {
            let (plan, selected_representation, measurement_label) = selection.into_parts();
            plan.validates_representation(selected_representation)?;
            representation = representation.combine(selected_representation)?;
            modifies_body |= !matches!(plan, BodyPlan::PassThrough);
            if source_body_visible && plan.inspects_input_bytes() {
                inspects_source_body = true;
            }
            if matches!(plan, BodyPlan::Replace(_) | BodyPlan::Discard) {
                source_body_visible = false;
            }
            let stage = {
                let kind = match plan {
                    BodyPlan::PassThrough => continue,
                    BodyPlan::Transform(filter) => BodyStageKind::Transform(Some(filter)),
                    BodyPlan::Buffer { limit, handler } => BodyStageKind::Buffer {
                        buffer: Some(BoundedBodyBuffer::new(limit.get())),
                        handler: Some(handler),
                    },
                    BodyPlan::Replace(body) => BodyStageKind::Replace(Some(body)),
                    BodyPlan::Discard => BodyStageKind::Discard,
                };
                BodyStage {
                    kind,
                    measurement_label,
                    representation: selected_representation,
                    output_sequence: FrameSequence::default(),
                }
            };
            stages.push(stage);
        }
        Ok(Self {
            stages,
            input_sequence: FrameSequence::default(),
            context,
            measurement_scope: "Body",
            gate,
            limits,
            representation,
            inspects_source_body,
            modifies_body,
            finished: false,
        })
    }

    /// Measures a content-codec operation on this pipeline's exchange clock.
    pub fn measure_work(
        &self,
        kind: &'static str,
        label: &str,
    ) -> Option<crate::performance::WorkTimer> {
        self.context
            .measure_work(kind, &format!("{} · {label}", self.measurement_scope))
    }

    pub(super) fn with_measurement_scope(mut self, scope: &'static str) -> Self {
        self.measurement_scope = scope;
        self
    }

    /// Aggregated representation required by all active stages.
    pub const fn representation(&self) -> BodyRepresentation {
        self.representation
    }

    /// Whether a stage before the first replacement/discard observes source bytes.
    pub const fn inspects_source_body(&self) -> bool {
        self.inspects_source_body
    }

    /// Omits every optional decoded stage after unsupported content coding was
    /// detected, retaining neutral stages and exact pass-through behavior.
    ///
    /// # Errors
    ///
    /// Returns [`BodyRepresentationError::OptionalDecodedNotDeclinable`] when
    /// the aggregate requirement is not [`BodyRepresentation::DecodedIfSupported`].
    pub fn decline_optional_decoded(&mut self) -> Result<(), BodyRepresentationError> {
        if self.representation != BodyRepresentation::DecodedIfSupported {
            return Err(BodyRepresentationError::OptionalDecodedNotDeclinable);
        }
        self.stages
            .retain(|stage| stage.representation != BodyRepresentation::DecodedIfSupported);
        self.representation = BodyRepresentation::Neutral;
        self.inspects_source_body = false;
        self.modifies_body = !self.stages.is_empty();
        Ok(())
    }

    /// Whether any selected plan can change body frames.
    pub fn modifies_body(&self) -> bool {
        self.modifies_body
    }

    /// Applies one canonical input frame and returns bounded output frames.
    ///
    /// # Errors
    ///
    /// Returns a typed sequence, resource-limit, hook, timeout, panic,
    /// cancellation, or shutdown failure.
    pub async fn process(&mut self, frame: BodyFrame) -> Result<Vec<BodyFrame>, BodyPipelineError> {
        if self.finished {
            return Err(BodyLimitError::DataAfterTrailers.into());
        }
        self.input_sequence.accept(&frame)?;
        self.process_from(0, vec![frame]).await
    }

    /// Flushes every stage in order and passes output through later stages.
    ///
    /// Repeated calls return no frames.
    ///
    /// # Errors
    ///
    /// Returns a typed resource-limit, hook, timeout, panic, cancellation, or
    /// shutdown failure.
    pub async fn finish(&mut self) -> Result<Vec<BodyFrame>, BodyPipelineError> {
        if std::mem::replace(&mut self.finished, true) {
            return Ok(Vec::new());
        }
        let mut final_output = Vec::new();
        for index in 0..self.stages.len() {
            let output = self.finish_stage(index).await?;
            final_output.extend(self.process_from(index + 1, output).await?);
        }
        Ok(final_output)
    }

    /// Pumps a bounded source stream into a bounded destination stream.
    ///
    /// # Errors
    ///
    /// Returns when the source fails, the destination closes, the exchange is
    /// cancelled, or one pipeline stage fails.
    pub async fn pump(
        mut self,
        mut input: BodyStream,
        output: BodyStreamSender,
    ) -> Result<(), BodyPipelineError> {
        let cancellation = self.context.cancellation().clone();
        loop {
            let next = tokio::select! {
                next = input.recv() => next,
                () = self.context.cancellation().cancelled() => {
                    return Err(HookExecutionError::Cancelled.into());
                }
            };
            let Some(frame) = next else {
                break;
            };
            let frame = frame.map_err(BodyPipelineError::Input)?;
            let frames = self.process(frame).await?;
            Self::send_all(&cancellation, &output, frames).await?;
        }
        let frames = self.finish().await?;
        Self::send_all(&cancellation, &output, frames).await
    }

    async fn send_all(
        cancellation: &super::ExchangeCancellation,
        output: &BodyStreamSender,
        frames: Vec<BodyFrame>,
    ) -> Result<(), BodyPipelineError> {
        for frame in frames {
            tokio::select! {
                result = output.send(Ok(frame)) => result.map_err(BodyPipelineError::OutputClosed)?,
                () = cancellation.cancelled() => {
                    return Err(HookExecutionError::Cancelled.into());
                }
            }
        }
        Ok(())
    }

    async fn process_from(
        &mut self,
        start: usize,
        mut frames: Vec<BodyFrame>,
    ) -> Result<Vec<BodyFrame>, BodyPipelineError> {
        for index in start..self.stages.len() {
            let mut output = Vec::new();
            for frame in frames {
                output.extend(self.process_stage(index, frame).await?);
            }
            frames = output;
        }
        Ok(frames)
    }

    async fn process_stage(
        &mut self,
        index: usize,
        frame: BodyFrame,
    ) -> Result<Vec<BodyFrame>, BodyPipelineError> {
        let _timing = self.context.measure_work(
            "transform",
            self.stages[index]
                .measurement_label
                .as_deref()
                .unwrap_or("Body transformation"),
        );
        let output = match &mut self.stages[index].kind {
            BodyStageKind::Transform(filter) => {
                let mut owned = filter
                    .take()
                    .expect("unfinished transform retains its filter");
                let permit = self.gate.permit(&self.context).await?;
                let task = tokio::spawn(async move {
                    let result = owned.on_frame(frame).await;
                    (owned, result)
                });
                let (owned, result) = self.gate.await_task(&self.context, task, permit).await?;
                *filter = Some(owned);
                result?
            }
            BodyStageKind::Buffer { buffer, .. } => {
                buffer
                    .as_mut()
                    .expect("unfinished buffer retains storage")
                    .push(frame)?;
                Vec::new()
            }
            BodyStageKind::Replace(_) | BodyStageKind::Discard => Vec::new(),
        };
        Self::validate_output(
            &mut self.stages[index].output_sequence,
            &output,
            self.limits,
        )?;
        Ok(output)
    }

    async fn finish_stage(&mut self, index: usize) -> Result<Vec<BodyFrame>, BodyPipelineError> {
        let _timing = self.context.measure_work(
            "transform",
            self.stages[index]
                .measurement_label
                .as_deref()
                .unwrap_or("Body transformation"),
        );
        let output = match &mut self.stages[index].kind {
            BodyStageKind::Transform(filter) => {
                let Some(mut owned) = filter.take() else {
                    return Ok(Vec::new());
                };
                let permit = self.gate.permit(&self.context).await?;
                let task = tokio::spawn(async move {
                    let result = owned.finish().await;
                    (owned, result)
                });
                let (owned, result) = self.gate.await_task(&self.context, task, permit).await?;
                *filter = Some(owned);
                result?
            }
            BodyStageKind::Buffer { buffer, handler } => {
                let Some(buffer) = buffer.take() else {
                    return Ok(Vec::new());
                };
                let (data, trailers) = buffer.finish();
                let body = BufferedBody { data, trailers };
                let mut owned = handler
                    .take()
                    .expect("unfinished buffer retains its handler");
                let permit = self.gate.permit(&self.context).await?;
                let task = tokio::spawn(async move {
                    let result = owned.on_body(body).await;
                    (owned, result)
                });
                let (owned, result) = self.gate.await_task(&self.context, task, permit).await?;
                *handler = Some(owned);
                result?.into_frames()
            }
            BodyStageKind::Replace(body) => {
                body.take().map_or_else(Vec::new, BufferedBody::into_frames)
            }
            BodyStageKind::Discard => Vec::new(),
        };
        Self::validate_output(
            &mut self.stages[index].output_sequence,
            &output,
            self.limits,
        )?;
        Ok(output)
    }

    fn validate_output(
        sequence: &mut FrameSequence,
        frames: &[BodyFrame],
        limits: BodyPipelineLimits,
    ) -> Result<(), BodyPipelineError> {
        if frames.len() > limits.max_output_frames_per_call.get() {
            return Err(BodyPipelineError::OutputFrameLimit {
                limit: limits.max_output_frames_per_call.get(),
                attempted: frames.len(),
            });
        }
        let mut bytes = 0usize;
        for frame in frames {
            sequence.accept(frame)?;
            if let BodyFrame::Data(data) = frame {
                bytes = bytes.saturating_add(data.len());
            }
        }
        if bytes > limits.max_output_bytes_per_call.get() {
            return Err(BodyPipelineError::OutputByteLimit {
                limit: limits.max_output_bytes_per_call.get(),
                attempted: bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, time::Duration};

    use super::*;
    use crate::{
        ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId, Target,
        intercept::{ExchangeMetadata, HookLimits},
    };

    struct Prefix(&'static [u8]);

    impl BodyFilter for Prefix {
        fn on_frame(
            &mut self,
            frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            let prefix = self.0;
            Box::pin(async move {
                Ok(match frame {
                    BodyFrame::Data(data) => {
                        let mut output = Vec::with_capacity(prefix.len() + data.len());
                        output.extend_from_slice(prefix);
                        output.extend_from_slice(&data);
                        vec![BodyFrame::Data(Bytes::from(output))]
                    }
                    BodyFrame::Trailers(trailers) => vec![BodyFrame::Trailers(trailers)],
                })
            })
        }
    }

    struct Flush;

    impl BodyFilter for Flush {
        fn on_frame(
            &mut self,
            frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            Box::pin(async move { Ok(vec![frame]) })
        }

        fn finish(&mut self) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            Box::pin(async { Ok(vec![BodyFrame::Data(Bytes::from_static(b"!"))]) })
        }
    }

    struct IdentityBuffer;

    impl BufferedBodyHandler for IdentityBuffer {
        fn on_body(
            &mut self,
            body: BufferedBody,
        ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>> {
            Box::pin(async move { Ok(body) })
        }
    }

    enum FailureFilter {
        Abort,
        Panic,
        Wait,
    }

    impl BodyFilter for FailureFilter {
        fn on_frame(
            &mut self,
            _frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            Box::pin(async move {
                match self {
                    Self::Abort => Err(BodyHookError::Abort(HookAbort::Rejected)),
                    Self::Panic => panic!("body filter panic"),
                    Self::Wait => pending().await,
                }
            })
        }
    }

    struct Expand(usize);

    impl BodyFilter for Expand {
        fn on_frame(
            &mut self,
            _frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            let count = self.0;
            Box::pin(async move {
                Ok((0..count)
                    .map(|_| BodyFrame::Data(Bytes::from_static(b"x")))
                    .collect())
            })
        }
    }

    struct InvalidSequence;

    impl BodyFilter for InvalidSequence {
        fn on_frame(
            &mut self,
            _frame: BodyFrame,
        ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
            Box::pin(async {
                Ok(vec![
                    BodyFrame::Trailers(HeaderBlock::new()),
                    BodyFrame::Data(Bytes::from_static(b"late")),
                ])
            })
        }
    }

    fn context(timeout: Duration) -> HookContext {
        let session = SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: ConnectionId(2),
            stream_id: StreamId(3),
            client_addr: "127.0.0.1:1000".parse().unwrap(),
            client_identity: crate::ClientIdentity::default(),
            proxy_addr: "127.0.0.1:2000".parse().unwrap(),
            ingress_version: HttpLegVersion::Http2,
            egress_version: None,
        };
        HookContext::new(
            ExchangeMetadata::from_session(
                &session,
                Target {
                    scheme: "https".to_owned(),
                    authority: "example.test".to_owned(),
                    host: "example.test".to_owned(),
                    port: 443,
                    path: "/".to_owned(),
                    query: None,
                },
            ),
            timeout,
        )
    }

    fn selected_pipeline(
        plans: Vec<BodyPlanSelection>,
    ) -> Result<BodyPipeline, BodyRepresentationError> {
        let timeout = Duration::from_secs(1);
        BodyPipeline::from_plans(
            plans,
            context(timeout),
            CallbackGate::new(HookLimits {
                callback_timeout: timeout,
                terminal_timeout: timeout,
                max_paused_exchanges: NonZeroUsize::new(4).unwrap(),
            }),
            BodyPipelineLimits::default(),
        )
    }

    fn pipeline(plans: Vec<BodyPlan>) -> BodyPipeline {
        selected_pipeline(
            plans
                .into_iter()
                .map(|plan| {
                    let representation = if matches!(plan, BodyPlan::PassThrough) {
                        BodyRepresentation::Neutral
                    } else {
                        BodyRepresentation::Raw
                    };
                    BodyPlanSelection::new(plan, representation)
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn representation_requirements_are_validated_and_aggregated() {
        let conflict = selected_pipeline(vec![
            BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"raw"))),
                BodyRepresentation::Raw,
            ),
            BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"decoded"))),
                BodyRepresentation::DecodedRequired,
            ),
        ])
        .unwrap_err();
        assert_eq!(conflict, BodyRepresentationError::RawDecodedConflict);

        let invalid = selected_pipeline(vec![BodyPlanSelection::new(
            BodyPlan::Discard,
            BodyRepresentation::Neutral,
        )])
        .unwrap_err();
        assert_eq!(invalid, BodyRepresentationError::NeutralBodyChange);

        let pipeline = selected_pipeline(vec![
            BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"optional"))),
                BodyRepresentation::DecodedIfSupported,
            ),
            BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"required"))),
                BodyRepresentation::DecodedRequired,
            ),
        ])
        .unwrap();
        assert_eq!(
            pipeline.representation(),
            BodyRepresentation::DecodedRequired
        );
    }

    #[test]
    fn replacement_before_decoded_filters_does_not_inspect_source_bytes() {
        let replacement = BufferedBody::try_new(8, Bytes::from_static(b"new"), None).unwrap();
        let pipeline = selected_pipeline(vec![
            BodyPlanSelection::new(
                BodyPlan::Replace(replacement),
                BodyRepresentation::DecodedRequired,
            ),
            BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"after"))),
                BodyRepresentation::DecodedRequired,
            ),
        ])
        .unwrap();
        assert!(!pipeline.inspects_source_body());
    }

    #[test]
    fn optional_decoded_stages_can_be_declined_before_processing() {
        let mut pipeline = selected_pipeline(vec![BodyPlanSelection::new(
            BodyPlan::Transform(Box::new(Prefix(b"optional"))),
            BodyRepresentation::DecodedIfSupported,
        )])
        .unwrap();
        pipeline.decline_optional_decoded().unwrap();
        assert_eq!(pipeline.representation(), BodyRepresentation::Neutral);
        assert!(!pipeline.modifies_body());
        assert!(matches!(
            pipeline.decline_optional_decoded(),
            Err(BodyRepresentationError::OptionalDecodedNotDeclinable)
        ));
    }

    #[test]
    fn buffered_body_validates_bounds_and_trailers() {
        let body = BufferedBody::try_from_frames(3, [BodyFrame::Data(Bytes::from_static(b"abc"))])
            .unwrap();
        assert_eq!(body.data(), &Bytes::from_static(b"abc"));
        assert!(matches!(
            BufferedBody::try_from_frames(2, [BodyFrame::Data(Bytes::from_static(b"abc"))]),
            Err(BodyLimitError::LimitExceeded { .. })
        ));
    }

    #[tokio::test]
    async fn pass_through_streams_before_the_producer_finishes() {
        let pipeline = pipeline(vec![BodyPlan::PassThrough]);
        let (input_sender, input) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let (output_sender, mut output) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let task = tokio::spawn(pipeline.pump(input, output_sender));
        input_sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"first"))))
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), output.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            BodyFrame::Data(Bytes::from_static(b"first"))
        );
        drop(input_sender);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn transforms_compose_and_flush_through_later_stages() {
        let mut pipeline = pipeline(vec![
            BodyPlan::Transform(Box::new(Flush)),
            BodyPlan::Transform(Box::new(Prefix(b"A"))),
            BodyPlan::Transform(Box::new(Prefix(b"B"))),
        ]);
        assert_eq!(
            pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"x")))
                .await
                .unwrap(),
            [BodyFrame::Data(Bytes::from_static(b"BAx"))]
        );
        assert_eq!(
            pipeline.finish().await.unwrap(),
            [BodyFrame::Data(Bytes::from_static(b"BA!"))]
        );
        assert!(pipeline.finish().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn buffer_enforces_below_exact_and_above_limit() {
        for size in [3, 4] {
            let mut pipeline = pipeline(vec![BodyPlan::Buffer {
                limit: NonZeroUsize::new(4).unwrap(),
                handler: Box::new(IdentityBuffer),
            }]);
            assert!(
                pipeline
                    .process(BodyFrame::Data(Bytes::from(vec![b'x'; size])))
                    .await
                    .unwrap()
                    .is_empty()
            );
            let output = pipeline.finish().await.unwrap();
            assert_eq!(output, [BodyFrame::Data(Bytes::from(vec![b'x'; size]))]);
        }

        let mut pipeline = pipeline(vec![BodyPlan::Buffer {
            limit: NonZeroUsize::new(4).unwrap(),
            handler: Box::new(IdentityBuffer),
        }]);
        assert!(matches!(
            pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"12345")))
                .await,
            Err(BodyPipelineError::InvalidSequence(
                BodyLimitError::LimitExceeded {
                    limit: 4,
                    attempted: 5
                }
            ))
        ));
    }

    #[tokio::test]
    async fn replace_and_discard_consume_input_without_forwarding_it() {
        let replacement = BufferedBody::try_new(
            16,
            Bytes::from_static(b"replacement"),
            Some(HeaderBlock::new()),
        )
        .unwrap();
        let mut replacement_pipeline = pipeline(vec![BodyPlan::Replace(replacement)]);
        assert!(
            replacement_pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"original")))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            replacement_pipeline.finish().await.unwrap(),
            [
                BodyFrame::Data(Bytes::from_static(b"replacement")),
                BodyFrame::Trailers(HeaderBlock::new())
            ]
        );

        let mut discard_pipeline = pipeline(vec![BodyPlan::Discard]);
        assert!(
            discard_pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"discarded")))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(discard_pipeline.finish().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_input_and_transform_output_sequences_are_rejected() {
        let mut input_pipeline = pipeline(Vec::new());
        input_pipeline
            .process(BodyFrame::Trailers(HeaderBlock::new()))
            .await
            .unwrap();
        assert_eq!(
            input_pipeline
                .process(BodyFrame::Data(Bytes::from_static(b"late")))
                .await
                .unwrap_err(),
            BodyPipelineError::InvalidSequence(BodyLimitError::DataAfterTrailers)
        );

        let mut invalid_pipeline = pipeline(vec![BodyPlan::Transform(Box::new(InvalidSequence))]);
        assert_eq!(
            invalid_pipeline
                .process(BodyFrame::Data(Bytes::new()))
                .await
                .unwrap_err(),
            BodyPipelineError::InvalidSequence(BodyLimitError::DataAfterTrailers)
        );
    }

    #[tokio::test]
    async fn bounded_frame_sequence_model_smoke_is_total_and_matches_http_ordering() {
        // Exhaust every sequence of up to five data/trailer symbols. This is a
        // deterministic fuzz-smoke corpus that is also suitable for Miri.
        for length in 0..=5_u32 {
            let combinations = 2_u32.pow(length);
            for bits in 0..combinations {
                let frames = (0..length)
                    .map(|index| {
                        if bits & (1 << index) == 0 {
                            BodyFrame::Data(Bytes::from_static(b"x"))
                        } else {
                            BodyFrame::Trailers(HeaderBlock::new())
                        }
                    })
                    .collect::<Vec<_>>();
                let first_trailer = frames
                    .iter()
                    .position(|frame| matches!(frame, BodyFrame::Trailers(_)));
                let valid = first_trailer.is_none_or(|index| {
                    index + 1 == frames.len()
                        && frames
                            .iter()
                            .filter(|frame| matches!(frame, BodyFrame::Trailers(_)))
                            .count()
                            == 1
                });
                let mut pipeline = pipeline(vec![BodyPlan::PassThrough]);
                let mut accepted = true;
                for frame in frames {
                    if pipeline.process(frame).await.is_err() {
                        accepted = false;
                        break;
                    }
                }
                assert_eq!(accepted, valid, "sequence bits={bits:b} length={length}");
            }
        }
    }

    #[tokio::test]
    async fn transform_output_limits_are_enforced() {
        let limits = BodyPipelineLimits {
            max_output_frames_per_call: NonZeroUsize::new(2).unwrap(),
            max_output_bytes_per_call: NonZeroUsize::new(2).unwrap(),
        };
        let timeout = Duration::from_secs(1);
        let gate = CallbackGate::new(HookLimits {
            callback_timeout: timeout,
            terminal_timeout: timeout,
            max_paused_exchanges: NonZeroUsize::new(1).unwrap(),
        });
        let mut pipeline = BodyPipeline::from_plans(
            vec![BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Expand(3))),
                BodyRepresentation::Raw,
            )],
            context(timeout),
            gate.clone(),
            limits,
        )
        .unwrap();
        assert!(matches!(
            pipeline.process(BodyFrame::Data(Bytes::new())).await,
            Err(BodyPipelineError::OutputFrameLimit {
                limit: 2,
                attempted: 3
            })
        ));

        let mut pipeline = BodyPipeline::from_plans(
            vec![BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(Prefix(b"abc"))),
                BodyRepresentation::Raw,
            )],
            context(timeout),
            gate,
            limits,
        )
        .unwrap();
        assert!(matches!(
            pipeline.process(BodyFrame::Data(Bytes::new())).await,
            Err(BodyPipelineError::OutputByteLimit {
                limit: 2,
                attempted: 3
            })
        ));
    }

    #[tokio::test]
    async fn transform_abort_panic_timeout_and_cancellation_are_typed() {
        let mut abort = pipeline(vec![BodyPlan::Transform(Box::new(FailureFilter::Abort))]);
        assert!(matches!(
            abort.process(BodyFrame::Data(Bytes::new())).await,
            Err(BodyPipelineError::Hook(BodyHookError::Abort(
                HookAbort::Rejected
            )))
        ));

        let mut panicking = pipeline(vec![BodyPlan::Transform(Box::new(FailureFilter::Panic))]);
        assert_eq!(
            panicking
                .process(BodyFrame::Data(Bytes::new()))
                .await
                .unwrap_err(),
            BodyPipelineError::Execution(HookExecutionError::Panicked)
        );

        let timeout = Duration::from_millis(5);
        let mut waiting = BodyPipeline::from_plans(
            vec![BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(FailureFilter::Wait)),
                BodyRepresentation::Raw,
            )],
            context(timeout),
            CallbackGate::new(HookLimits {
                callback_timeout: timeout,
                terminal_timeout: timeout,
                max_paused_exchanges: NonZeroUsize::new(1).unwrap(),
            }),
            BodyPipelineLimits::default(),
        )
        .unwrap();
        assert_eq!(
            waiting
                .process(BodyFrame::Data(Bytes::new()))
                .await
                .unwrap_err(),
            BodyPipelineError::Execution(HookExecutionError::TimedOut)
        );

        let timeout = Duration::from_secs(1);
        let mut cancelled = BodyPipeline::from_plans(
            vec![BodyPlanSelection::new(
                BodyPlan::Transform(Box::new(FailureFilter::Wait)),
                BodyRepresentation::Raw,
            )],
            context(timeout),
            CallbackGate::new(HookLimits {
                callback_timeout: timeout,
                terminal_timeout: timeout,
                max_paused_exchanges: NonZeroUsize::new(1).unwrap(),
            }),
            BodyPipelineLimits::default(),
        )
        .unwrap();
        let cancellation = cancelled.context.cancellation().clone();
        let task =
            tokio::spawn(async move { cancelled.process(BodyFrame::Data(Bytes::new())).await });
        tokio::task::yield_now().await;
        cancellation.cancel();
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            BodyPipelineError::Execution(HookExecutionError::Cancelled)
        );
    }

    #[tokio::test]
    async fn pump_reports_dropped_consumer() {
        let pipeline = pipeline(Vec::new());
        let (input_sender, input) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        let (output_sender, output) = BodyStream::channel(NonZeroUsize::new(1).unwrap());
        drop(output);
        input_sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"x"))))
            .await
            .unwrap();
        drop(input_sender);
        assert_eq!(
            pipeline.pump(input, output_sender).await.unwrap_err(),
            BodyPipelineError::OutputClosed(BodyChannelClosed)
        );
    }
}
