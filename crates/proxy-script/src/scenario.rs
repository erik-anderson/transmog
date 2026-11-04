//! Finite synthetic exchanges using the production interceptor and body pipelines.

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use transmog_core::intercept::{
    BodyPipeline, BodyPipelineError, BodyPipelineLimits, BufferedHead, CompletedExchange,
    ExchangeFailure, ExchangeFailureCategory, ExchangeFailureKind, ExchangeStage,
    HookExecutionError, HookLimits, InterceptorChainFactory, RequestHeadOutcome,
    ResponseHeadOutcome, TerminalReport,
};
use transmog_core::{
    BodyFrame, BodyLimitError, ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId,
};

/// A complete decoded fixture. No network, route authorization or asset lookup occurs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptScenario {
    /// Optional decoded response chunks and simulated arrival delays. Bytes must match `response_body`.
    #[serde(default)]
    pub response_chunks: Option<Vec<ScriptScenarioChunk>>,
    /// Original client request.
    pub request: ScriptRequest,
    /// Simulated upstream or local response; absent for a failed upstream exchange.
    pub response: Option<ScriptResponse>,
    /// Complete decoded upload bytes.
    pub request_body: Vec<u8>,
    /// Complete decoded response bytes.
    pub response_body: Vec<u8>,
    /// Whether the fixture response is local.
    pub local_response: bool,
    /// Simulated failure after upload, before receiving a response.
    pub failure: Option<ExchangeFailureCategory>,
    /// Protocol for both fixture messages.
    pub http_version: HttpLegVersion,
    /// Controlled wall-clock start in milliseconds since Unix epoch.
    pub started_unix_ms: u64,
    /// Controlled total duration; callbacks use evenly spaced phase times.
    pub duration_ms: u64,
}

/// One decoded input chunk with a controlled delay before arrival.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptScenarioChunk {
    /// Decoded input bytes, independent of HTTP frame boundaries.
    pub bytes: Vec<u8>,
    /// Simulated network wait; no wall-clock sleep or traffic occurs.
    #[serde(default)]
    pub delay_ms: u64,
}

/// Bounded incremental callback evidence without copying body payloads into the timeline.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptStreamCall {
    /// Pending bytes supplied to the callback.
    pub input_bytes: usize,
    /// EOF was known during this callback.
    pub end: bool,
    /// This callback could edit the deferred response head.
    pub can_edit_headers: bool,
    /// Controlled elapsed time at invocation.
    pub elapsed_ms: u64,
    /// Returned action, or failed.
    pub action: String,
}

/// Simulated source arrival, output and header release for one response processing step.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScenarioBodyStep {
    /// Controlled elapsed time.
    pub elapsed_ms: u64,
    /// Newly arrived input bytes, excluding previously queued bytes.
    pub input_bytes: usize,
    /// Bytes released by the composed body pipeline.
    pub output_bytes: usize,
    /// Simulated response headers have been released.
    pub headers_released: bool,
}

/// Sandbox execution verdict, independent of the fixture's exchange outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptScenarioVerdict {
    /// All selected callbacks and pipelines succeeded, including an expected upstream failure.
    Passed,
    /// A callback or pipeline failed.
    Failed,
    /// Execution stopped at a simulation boundary.
    Incomplete,
}

/// Native phase selection or execution outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptScenarioSelection {
    /// Callback returned a valid result.
    Invoked,
    /// Callback failed.
    Failed,
    /// Callback was not exported.
    NotExported,
    /// Original request did not match.
    RequestFilter,
    /// An earlier phase stopped execution.
    NotReached,
    /// Message did not match the body filter.
    BodyFilter,
    /// HTTP semantics prohibit this body.
    BodyForbidden,
    /// Selected body exceeded its input buffer before callback invocation.
    InputLimit,
    /// Body planning or processing failed; a returned callback result may still be available.
    PipelineFailed,
    /// A different body mode was selected for this response.
    BodyMode,
}

/// One phase's selection and execution evidence.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScenarioStep {
    /// Exported callback name.
    pub handler: ScriptHandler,
    /// Invoked, failed, or a concrete reason the callback was skipped.
    pub selection: ScriptScenarioSelection,
    /// State seen by this phase's last callback invocation, before its result.
    pub state: BTreeMap<String, serde_json::Value>,
    /// Last invocation's result, when successful.
    pub result: Option<ScriptResult>,
    /// Accepted streaming findings, including earlier calls, under the live evidence budget.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<transmog_core::intercept::ScriptReport>,
    /// Bounded runtime error, when unsuccessful.
    pub error: Option<ScriptFailure>,
}

/// Effective fixture messages and per-phase evidence from one fresh host.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScenarioResult {
    /// Controlled time when response headers were released, absent if never reached.
    pub headers_released_at_ms: Option<u64>,
    /// Source and output timing for the response body.
    pub body_steps: Vec<ScriptScenarioBodyStep>,
    /// First bounded set of incremental callback invocations.
    pub stream_calls: Vec<ScriptStreamCall>,
    /// Additional callbacks omitted from the timeline, with final phase evidence retained.
    pub omitted_stream_calls: usize,
    /// Whether sandbox execution succeeded; this is not an assertion about network behavior.
    pub verdict: ScriptScenarioVerdict,
    /// Completed, failed, or asset-unresolved.
    pub outcome: String,
    /// Original native request filter admission.
    pub admitted: bool,
    /// Every supported phase, including skipped phases.
    pub steps: Vec<ScriptScenarioStep>,
    /// Effective request after selected callbacks.
    pub request: ScriptRequest,
    /// Effective response, when reached.
    pub response: Option<ScriptResponse>,
    /// Effective decoded upload bytes.
    pub request_body: Vec<u8>,
    /// Accepted decoded response output; may be partial if body processing failed.
    pub response_body: Vec<u8>,
    /// Whether the effective response is local.
    pub local_response: bool,
    /// Why the scenario stopped, when applicable.
    pub error: Option<String>,
}

const PHASES: [ScriptHandler; 7] = [
    ScriptHandler::RequestHead,
    ScriptHandler::RequestBody,
    ScriptHandler::ResponseHead,
    ScriptHandler::ResponseChunk,
    ScriptHandler::ResponseBody,
    ScriptHandler::Completed,
    ScriptHandler::Failed,
];

struct RecordingRunner {
    elapsed: AtomicU64,
    calls: Mutex<Vec<ScriptStreamCall>>,
    call_count: AtomicU64,
    inner: Arc<dyn ScriptRunner>,
    steps: Mutex<Vec<ScriptScenarioStep>>,
    start: u64,
    duration: u64,
    manifest: ScriptManifest,
}
impl ScriptRunner for RecordingRunner {
    fn exchange_open(&self) -> Result<(), ScriptFailure> {
        self.inner.exchange_open()
    }
    fn exchange_closed(&self, exchange: &str, terminal: bool) {
        self.inner.exchange_closed(exchange, terminal);
    }
    fn debug_admit(
        &self,
        exchange: &str,
        method: &str,
        host: &str,
        path: &str,
    ) -> Result<(), ScriptFailure> {
        self.inner.debug_admit(exchange, method, host, path)
    }
    fn debug_pause_clock(
        &self,
        exchange: &str,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        self.inner.debug_pause_clock(exchange)
    }
    fn invoke(&self, mut event: ScriptInvocation) -> BoxScriptFuture<'_> {
        Box::pin(async move {
            let phase = event.context.handler;
            let index = PHASES
                .iter()
                .position(|p| *p == phase)
                .expect("known phase");
            let phase_time = match phase {
                ScriptHandler::RequestHead => 0,
                ScriptHandler::RequestBody => self.duration / 4,
                ScriptHandler::ResponseHead => self.duration / 2,
                ScriptHandler::ResponseBody => self.duration * 3 / 4,
                ScriptHandler::ResponseChunk => self.elapsed.load(Ordering::Acquire),
                ScriptHandler::Completed | ScriptHandler::Failed => self.duration,
            };
            let elapsed = self
                .elapsed
                .fetch_max(phase_time, Ordering::AcqRel)
                .max(phase_time);
            event.context.source = ScriptSource::Sandbox;
            event.context.started_unix_ms = self.start;
            event.context.elapsed_ms = elapsed;
            event.context.now_unix_ms = self.start + elapsed;
            let state = event.state.clone();
            let stream_window = (
                event.bytes.as_ref().map_or(0, Vec::len),
                event.end,
                event.can_edit_headers,
            );
            // Retain invocation evidence even if the outer hook gate cancels or contains a panic.
            {
                let mut steps = self
                    .steps
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                steps[index].selection = ScriptScenarioSelection::Invoked;
                steps[index].state = state.clone();
                steps[index].result = None;
                steps[index].error = None;
            }
            let result = match prepare_invocation(&self.manifest, &mut event) {
                Ok(request) => self.inner.invoke(event).await.and_then(|result| {
                    validate_result(&self.manifest, phase, &request, &result)?;
                    if phase == ScriptHandler::ResponseChunk {
                        super::adapter::validate_stream_window(
                            &self.manifest,
                            &result,
                            stream_window.0,
                            stream_window.1,
                            stream_window.2,
                        )?;
                    }
                    Ok(result)
                }),
                Err(error) => Err(error),
            };
            if phase == ScriptHandler::ResponseChunk {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                if count < 256 {
                    let action = match result.as_ref().map(|r| &r.decision) {
                        Ok(ScriptAction::Continue) => "continue",
                        Ok(ScriptAction::NeedMore) => "need-more",
                        Ok(ScriptAction::Forward { .. }) => "forward",
                        Ok(ScriptAction::Emit { .. }) => "emit",
                        Ok(ScriptAction::Edit { .. }) => "edit",
                        Ok(ScriptAction::Abort { .. }) => "abort",
                        Ok(ScriptAction::Respond { .. }) => "respond",
                        Err(_) => "failed",
                    };
                    self.calls
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(ScriptStreamCall {
                            input_bytes: stream_window.0,
                            end: stream_window.1,
                            can_edit_headers: stream_window.2,
                            elapsed_ms: elapsed,
                            action: action.into(),
                        });
                }
            }
            let mut steps = self
                .steps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            steps[index] = ScriptScenarioStep {
                handler: phase,
                selection: if result.is_ok() {
                    ScriptScenarioSelection::Invoked
                } else {
                    ScriptScenarioSelection::Failed
                },
                state,
                result: result.as_ref().ok().cloned(),
                reports: Vec::new(),
                error: result.as_ref().err().cloned(),
            };
            result
        })
    }
}

/// Runs all selected phases in order, carrying edits and exchange state through native pipelines.
/// # Errors
/// Rejects malformed or unbounded fixtures before any callback runs.
#[allow(clippy::too_many_lines)]
pub async fn test_scenario(
    manifest: ScriptManifest,
    runner: Arc<dyn ScriptRunner>,
    input: ScriptScenario,
) -> Result<ScriptScenarioResult, ScriptFailure> {
    validate_manifest(&manifest).map_err(|e| ScriptFailure::invalid(e.to_string()))?;
    if let Some(chunks) = &input.response_chunks {
        let bytes = chunks
            .iter()
            .try_fold(0usize, |sum, chunk| sum.checked_add(chunk.bytes.len()));
        let delay = chunks
            .iter()
            .try_fold(0u64, |sum, chunk| sum.checked_add(chunk.delay_ms));
        if chunks.len() > 1024
            || bytes != Some(input.response_body.len())
            || delay.is_none_or(|delay| delay > input.duration_ms / 2)
            || !chunks
                .iter()
                .flat_map(|chunk| chunk.bytes.iter())
                .eq(input.response_body.iter())
        {
            return Err(ScriptFailure::invalid(
                "Response chunks must match the body, with up to 1024 chunks and delays fitting the second half of the scenario duration",
            ));
        }
    }
    if input.request_body.len() > 16 * 1024 * 1024
        || input.response_body.len() > 16 * 1024 * 1024
        || input.duration_ms > 86_400_000
        || input.started_unix_ms > 9_007_199_254_740_991 - input.duration_ms.min(86_400_000)
        || input
            .started_unix_ms
            .checked_add(input.duration_ms)
            .is_none()
        || input.response.is_some() == input.failure.is_some()
        || (input.failure.is_some() && input.local_response)
    {
        return Err(ScriptFailure::invalid(
            "Provide a response or a failure, finite bodies, and a duration up to one day",
        ));
    }
    let mut validation = manifest.clone();
    validation.handlers.insert(ScriptHandler::RequestHead);
    validation.capabilities.read_sensitive_headers = true;
    let mut event = ScriptInvocation {
        bytes: None,
        end: false,
        can_edit_headers: false,
        context: ScriptContext {
            exchange_id: "scenario".into(),
            http_version: input.http_version,
            ..Default::default()
        },
        request: input.request,
        response: None,
        body: None,
        parameters: BTreeMap::new(),
        state: BTreeMap::new(),
        local_response: false,
        failure: None,
    };
    let mut request = prepare_invocation(&validation, &mut event)?;
    request.source_version = input.http_version;
    let mut response = input
        .response
        .map(|r| -> Result<ResponseHead, ScriptFailure> {
            if !(100..=599).contains(&r.status)
                || r.headers.len() > 256
                || r.headers
                    .iter()
                    .map(|h| h.name.len() + h.value.len())
                    .sum::<usize>()
                    > 128 * 1024
            {
                return Err(ScriptFailure::invalid("Invalid response fixture"));
            }
            let mut headers = HeaderBlock::new();
            for h in r.headers {
                headers.push(
                    HeaderField::try_new(h.name.as_str(), h.value)
                        .map_err(|_| ScriptFailure::invalid("Invalid response fixture header"))?,
                );
            }
            Ok(ResponseHead {
                status: r.status,
                headers,
                source_version: input.http_version,
            })
        })
        .transpose()?;
    if request.headers.values("content-encoding").next().is_some()
        || response
            .as_ref()
            .is_some_and(|r| r.headers.values("content-encoding").next().is_some())
    {
        return Err(ScriptFailure::invalid(
            "Scenario bodies must be decoded; remove Content-Encoding from fixture headers",
        ));
    }
    let admitted = manifest.prefilter.matches(&request);
    let recorder = Arc::new(RecordingRunner {
        elapsed: AtomicU64::new(0),
        calls: Mutex::new(Vec::new()),
        call_count: AtomicU64::new(0),
        inner: runner,
        start: input.started_unix_ms,
        duration: input.duration_ms,
        manifest: manifest.clone(),
        steps: Mutex::new(
            PHASES
                .iter()
                .map(|handler| ScriptScenarioStep {
                    handler: *handler,
                    selection: if !manifest.handlers.contains(handler) {
                        ScriptScenarioSelection::NotExported
                    } else if !admitted {
                        ScriptScenarioSelection::RequestFilter
                    } else {
                        ScriptScenarioSelection::NotReached
                    },
                    state: BTreeMap::new(),
                    result: None,
                    reports: Vec::new(),
                    error: None,
                })
                .collect(),
        ),
    });
    let metadata = ExchangeMetadata::from_session(
        &SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: ConnectionId(1),
            stream_id: StreamId(1),
            client_addr: std::net::SocketAddr::from(([127, 0, 0, 1], 1)),
            proxy_addr: std::net::SocketAddr::from(([127, 0, 0, 1], 2)),
            client_identity: transmog_core::ClientIdentity::default(),
            ingress_version: input.http_version,
            egress_version: None,
        },
        request.target.clone(),
    );
    let mut chain = InterceptorChainFactory::new(
        vec![
            registration(manifest.clone(), recorder.clone())
                .map_err(|e| ScriptFailure::invalid(e.to_string()))?,
        ],
        HookLimits::default(),
    )
    .create_exchange(metadata)
    .map_err(|e| ScriptFailure::invalid(e.to_string()))?;
    let mut request_body = input.request_body;
    let mut response_input = input.response_body;
    let mut response_body = Vec::new();
    let mut response_chunks = input.response_chunks;
    let mut headers_released_at_ms = None;
    let mut body_steps = Vec::new();
    let mut local = input.local_response;
    let mut outcome = "completed";
    let mut error = None;
    let mut stage = ExchangeStage::RequestHead;
    let execution: Result<(), String> = async {
        match chain
            .request_head(request.clone())
            .await
            .map_err(|e| e.to_string())?
        {
            RequestHeadOutcome::Continue { head, .. } => {
                request = head;
                stage = ExchangeStage::RequestBody;
                mark_body(
                    &recorder,
                    &manifest,
                    ScriptHandler::RequestBody,
                    &request.headers,
                    None,
                    false,
                );
                let mut pipeline = chain
                    .request_body_pipeline(&request, BodyPipelineLimits::default())
                    .await
                    .map_err(|error| body_error(&recorder, ScriptHandler::RequestBody, &error))?;
                request_body = run_body(
                    &recorder,
                    ScriptHandler::RequestBody,
                    &mut pipeline,
                    &request_body,
                )
                .await?;
                if let Some(BufferedHead::Request(head)) = pipeline.head() {
                    request = head.clone();
                }
            }
            RequestHeadOutcome::Respond {
                request_head,
                response: supplied,
            } => {
                request = request_head;
                response = Some(supplied.head);
                response_input = frames(supplied.body);
                response_chunks = None;
                local = true;
            }
            RequestHeadOutcome::RespondStreaming { .. } => {
                return Err("Scenario cannot resolve streaming response assets".into());
            }
            RequestHeadOutcome::Abort(reason) => return Err(format!("{reason:?}")),
        }
        if let Some(category) = input.failure.filter(|_| !local) {
            outcome = "failed";
            let report = chain
                .failed(ExchangeFailure {
                    metadata: Arc::clone(chain.context().metadata()),
                    stage: ExchangeStage::Upstream,
                    kind: failure_kind(category),
                    request_committed: false,
                    response_committed: false,
                    message: "Simulated upstream failure".into(),
                })
                .await;
            error = terminal_error(&recorder, ScriptHandler::Failed, &report);
            response = None;
            response_body.clear();
            return Ok(());
        }
        stage = ExchangeStage::ResponseHead;
        let head = response
            .take()
            .ok_or_else(|| "No response fixture".to_owned())?;
        match chain
            .response_head(
                &request,
                head,
                local.then(|| vec![BodyFrame::Data(Bytes::from(response_input.clone()))]),
                local,
            )
            .await
            .map_err(|e| e.to_string())?
        {
            ResponseHeadOutcome::Continue {
                head,
                replacement_body,
                local_response,
            } => {
                if let Some(body) = replacement_body {
                    response_input = frames(body);
                    response_chunks = None;
                }
                local = local_response;
                response = Some(head);
            }
            ResponseHeadOutcome::Abort(reason) => return Err(format!("{reason:?}")),
        }
        let head = response
            .as_ref()
            .ok_or_else(|| "No response fixture".to_owned())?;
        if transmog_core::body_semantics(&request.method, head.status)
            == transmog_core::BodySemantics::Forbidden
            || (request.method.eq_ignore_ascii_case("CONNECT") && (200..300).contains(&head.status))
        {
            mark_skip(
                &recorder,
                ScriptHandler::ResponseBody,
                ScriptScenarioSelection::BodyForbidden,
            );
            mark_skip(
                &recorder,
                ScriptHandler::ResponseChunk,
                ScriptScenarioSelection::BodyForbidden,
            );
            headers_released_at_ms = Some(
                recorder
                    .elapsed
                    .load(Ordering::Acquire)
                    .max(input.duration_ms / 2),
            );
            response_body.clear();
        } else {
            stage = ExchangeStage::ResponseBody;
            let mode = recorder
                .steps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|step| step.handler == ScriptHandler::ResponseHead)
                .and_then(|step| step.result.as_ref())
                .and_then(|result| result.body)
                .unwrap_or_else(|| super::adapter::default_response_mode(&manifest));
            let handler = if mode == ScriptBodyMode::Stream {
                ScriptHandler::ResponseChunk
            } else {
                ScriptHandler::ResponseBody
            };
            for candidate in [ScriptHandler::ResponseBody, ScriptHandler::ResponseChunk] {
                if candidate != handler || mode == ScriptBodyMode::Skip {
                    mark_skip(&recorder, candidate, ScriptScenarioSelection::BodyMode);
                }
                mark_body(
                    &recorder,
                    &manifest,
                    candidate,
                    &head.headers,
                    Some(head.status),
                    local,
                );
            }
            let mut pipeline = chain
                .response_body_pipeline(&request, head, local, BodyPipelineLimits::default())
                .await
                .map_err(|error| body_error(&recorder, handler, &error))?;
            response_body = std::mem::take(&mut response_input);
            let result = run_response_body(
                &recorder,
                handler,
                &mut pipeline,
                &mut response_body,
                response_chunks.as_deref(),
                &mut headers_released_at_ms,
                &mut body_steps,
            )
            .await;
            if let Some(BufferedHead::Response(head)) = pipeline.head() {
                response = Some(head.clone());
            }
            result?;
        }
        if let Some(head) = response.as_mut()
            && transmog_core::body_semantics(&request.method, head.status)
                == transmog_core::BodySemantics::Forbidden
        {
            response_body.clear();
            repair_bodyless_headers(head);
        }
        let report = chain
            .completed(CompletedExchange {
                metadata: Arc::clone(chain.context().metadata()),
                request_head: request.clone(),
                response_head: response
                    .clone()
                    .ok_or_else(|| "No response fixture".to_owned())?,
            })
            .await;
        error = terminal_error(&recorder, ScriptHandler::Completed, &report);
        Ok(())
    }
    .await;
    if let Err(message) = execution {
        outcome = if recorder
            .steps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|step| {
                step.result.as_ref().is_some_and(|r| {
                    matches!(
                        r.decision,
                        ScriptAction::Respond {
                            response: ScriptResponseSource::Asset { .. }
                        }
                    )
                })
            }) {
            "asset-unresolved"
        } else {
            "failed"
        };
        error = Some(message);
        if outcome != "asset-unresolved" {
            let callback_error = recorder
                .steps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find_map(|step| step.error.as_ref().map(|error| error.category));
            let kind = match callback_error {
                Some(ScriptFailureCategory::Timeout) => ExchangeFailureKind::HookTimedOut,
                Some(ScriptFailureCategory::ResourceLimit) => ExchangeFailureKind::Body,
                None if matches!(
                    stage,
                    ExchangeStage::RequestBody | ExchangeStage::ResponseBody
                ) =>
                {
                    ExchangeFailureKind::Body
                }
                _ => ExchangeFailureKind::HookAborted(HookAbort::Policy(
                    "Scenario callback failed".into(),
                )),
            };
            let report = chain
                .failed(ExchangeFailure {
                    metadata: Arc::clone(chain.context().metadata()),
                    stage,
                    kind,
                    request_committed: false,
                    response_committed: headers_released_at_ms.is_some(),
                    message: "Scenario stopped".into(),
                })
                .await;
            let _ = terminal_error(&recorder, ScriptHandler::Failed, &report);
        }
    }
    let mut steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(step) = steps
        .iter_mut()
        .find(|step| step.handler == ScriptHandler::ResponseChunk)
        && matches!(
            step.selection,
            ScriptScenarioSelection::Invoked
                | ScriptScenarioSelection::Failed
                | ScriptScenarioSelection::PipelineFailed
        )
    {
        step.reports = chain
            .hook_effects()
            .into_iter()
            .filter_map(|effect| {
                if effect.phase == transmog_core::intercept::HookPhase::ResponseBody
                    && let transmog_core::intercept::HookEffectAction::ScriptReport(report) =
                        effect.action
                {
                    Some(report)
                } else {
                    None
                }
            })
            .collect();
    }
    let verdict = if steps.iter().any(|step| step.error.is_some())
        || (error.is_some() && outcome != "asset-unresolved")
    {
        ScriptScenarioVerdict::Failed
    } else if outcome == "asset-unresolved" {
        ScriptScenarioVerdict::Incomplete
    } else {
        ScriptScenarioVerdict::Passed
    };
    if error.is_none() {
        error = steps
            .iter()
            .find_map(|step| step.error.as_ref().map(ToString::to_string));
    }
    Ok(ScriptScenarioResult {
        headers_released_at_ms,
        body_steps,
        stream_calls: recorder
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
        omitted_stream_calls: usize::try_from(recorder.call_count.load(Ordering::Relaxed))
            .unwrap_or(usize::MAX)
            .saturating_sub(256),
        verdict,
        outcome: outcome.into(),
        admitted,
        steps,
        request: script_request(&request, true),
        response: response.map(|r| ScriptResponse {
            status: r.status,
            headers: script_headers(&r.headers, true),
        }),
        request_body,
        response_body,
        local_response: local,
        error,
    })
}

fn repair_bodyless_headers(head: &mut ResponseHead) {
    match head.status {
        100..=199 | 204 => head.headers.remove_all("content-length"),
        205 => head
            .headers
            .replace_all(HeaderField::try_new("content-length", "0").expect("decimal length")),
        _ => {}
    }
}

async fn run_response_body(
    recorder: &RecordingRunner,
    handler: ScriptHandler,
    pipeline: &mut BodyPipeline,
    body: &mut Vec<u8>,
    chunks: Option<&[ScriptScenarioChunk]>,
    released: &mut Option<u64>,
    steps: &mut Vec<ScriptScenarioBodyStep>,
) -> Result<(), String> {
    // Retain only successfully processed output if a later chunk fails.
    let default = [ScriptScenarioChunk {
        bytes: std::mem::take(body),
        delay_ms: 0,
    }];
    let chunks = chunks.unwrap_or(&default);
    recorder
        .elapsed
        .fetch_max(recorder.duration / 2, Ordering::AcqRel);
    if pipeline.modifies_body()
        && let Some(head) = pipeline.head()
    {
        let mut headers = head.headers().clone();
        headers.remove_all("content-length");
        pipeline.set_head_headers(headers);
    }
    if !pipeline.head_pending() {
        *released = Some(recorder.elapsed.load(Ordering::Acquire));
    }
    for chunk in chunks {
        recorder.elapsed.fetch_add(chunk.delay_ms, Ordering::AcqRel);
        let result = pipeline
            .process(BodyFrame::Data(Bytes::copy_from_slice(&chunk.bytes)))
            .await
            .map_err(|error| body_error(recorder, handler, &error))?;
        if released.is_none() && !pipeline.head_pending() {
            *released = Some(recorder.elapsed.load(Ordering::Acquire));
        }
        let forbidden = response_body_forbidden(pipeline);
        let generated = if forbidden { 0 } else { output_bytes(&result) };
        steps.push(ScriptScenarioBodyStep {
            elapsed_ms: recorder.elapsed.load(Ordering::Acquire),
            input_bytes: chunk.bytes.len(),
            output_bytes: generated,
            headers_released: released.is_some(),
        });
        if forbidden {
            body.clear();
            return Ok(());
        }
        append_response_output(recorder, handler, body, result)?;
    }
    recorder
        .elapsed
        .fetch_max(recorder.duration * 3 / 4, Ordering::AcqRel);
    let result = pipeline
        .finish()
        .await
        .map_err(|error| body_error(recorder, handler, &error))?;
    let held_until_eof = released.is_none();
    if held_until_eof {
        *released = Some(recorder.elapsed.load(Ordering::Acquire));
    }
    let forbidden = response_body_forbidden(pipeline);
    let generated = if forbidden { 0 } else { output_bytes(&result) };
    steps.push(ScriptScenarioBodyStep {
        elapsed_ms: recorder.elapsed.load(Ordering::Acquire),
        input_bytes: 0,
        output_bytes: generated,
        headers_released: true,
    });
    if forbidden {
        body.clear();
        return Ok(());
    }
    append_response_output(recorder, handler, body, result)?;
    if held_until_eof && let Some(BufferedHead::Response(head)) = pipeline.head() {
        let mut headers = head.headers.clone();
        headers.replace_all(
            HeaderField::try_new("content-length", body.len().to_string()).expect("decimal length"),
        );
        pipeline.set_head_headers(headers);
    }
    Ok(())
}

fn response_body_forbidden(pipeline: &BodyPipeline) -> bool {
    matches!(pipeline.head(), Some(BufferedHead::Response(head))
        if transmog_core::body_semantics("GET", head.status) == transmog_core::BodySemantics::Forbidden)
}

fn output_bytes(frames: &[BodyFrame]) -> usize {
    frames
        .iter()
        .map(|frame| match frame {
            BodyFrame::Data(bytes) => bytes.len(),
            BodyFrame::Trailers(_) => 0,
        })
        .sum()
}

fn append_response_output(
    recorder: &RecordingRunner,
    handler: ScriptHandler,
    output: &mut Vec<u8>,
    frames_out: Vec<BodyFrame>,
) -> Result<(), String> {
    let bytes = frames(frames_out);
    let attempted = output.len().saturating_add(bytes.len());
    if attempted > 16 * 1024 * 1024 {
        return Err(body_error(
            recorder,
            handler,
            &BodyPipelineError::OutputByteLimit {
                limit: 16 * 1024 * 1024,
                attempted,
            },
        ));
    }
    output.extend(bytes);
    Ok(())
}

fn terminal_error(
    recorder: &RecordingRunner,
    handler: ScriptHandler,
    report: &TerminalReport,
) -> Option<String> {
    let error = report.errors().first()?;
    let mut steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let step = steps
        .iter_mut()
        .find(|step| step.handler == handler)
        .expect("phase");
    step.selection = ScriptScenarioSelection::Failed;
    step.error = Some(ScriptFailure {
        logs: Vec::new(),
        category: match error.source {
            HookExecutionError::TimedOut => ScriptFailureCategory::Timeout,
            HookExecutionError::Panicked => ScriptFailureCategory::Crash,
            HookExecutionError::Cancelled | HookExecutionError::ShuttingDown => {
                ScriptFailureCategory::Unavailable
            }
        },
        message: bounded(&error.to_string(), 512),
        line: None,
        column: None,
        exception: None,
    });
    Some(error.to_string())
}

fn mark_skip(recorder: &RecordingRunner, handler: ScriptHandler, reason: ScriptScenarioSelection) {
    let mut steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let step = steps
        .iter_mut()
        .find(|s| s.handler == handler)
        .expect("phase");
    if step.selection == ScriptScenarioSelection::NotReached {
        step.selection = reason;
    }
}
fn mark_body(
    recorder: &RecordingRunner,
    manifest: &ScriptManifest,
    handler: ScriptHandler,
    headers: &HeaderBlock,
    status: Option<u16>,
    local: bool,
) {
    let filter = if handler == ScriptHandler::RequestBody {
        &manifest.request_body_filter
    } else {
        &manifest.response_body_filter
    };
    if !super::adapter::admits(filter, headers, status, local) {
        mark_skip(recorder, handler, ScriptScenarioSelection::BodyFilter);
    }
}
async fn run_body(
    recorder: &RecordingRunner,
    handler: ScriptHandler,
    pipeline: &mut BodyPipeline,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let run = async {
        let mut output = pipeline
            .process(BodyFrame::Data(Bytes::copy_from_slice(body)))
            .await?;
        output.extend(pipeline.finish().await?);
        Ok(frames(output))
    }
    .await;
    run.map_err(|error| body_error(recorder, handler, &error))
}
fn body_error(
    recorder: &RecordingRunner,
    handler: ScriptHandler,
    error: &BodyPipelineError,
) -> String {
    let input_limit = matches!(
        error,
        BodyPipelineError::InvalidSequence(BodyLimitError::LimitExceeded { .. })
    );
    let mut steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let step = steps
        .iter_mut()
        .find(|step| step.handler == handler)
        .expect("phase");
    if matches!(
        step.selection,
        ScriptScenarioSelection::NotReached | ScriptScenarioSelection::Invoked
    ) {
        step.selection = if input_limit && step.selection == ScriptScenarioSelection::NotReached {
            ScriptScenarioSelection::InputLimit
        } else {
            ScriptScenarioSelection::PipelineFailed
        };
    }
    error.to_string()
}
fn frames(frames: Vec<BodyFrame>) -> Vec<u8> {
    frames
        .into_iter()
        .filter_map(|frame| match frame {
            BodyFrame::Data(bytes) => Some(bytes),
            BodyFrame::Trailers(_) => None,
        })
        .flatten()
        .collect()
}
fn failure_kind(category: ExchangeFailureCategory) -> ExchangeFailureKind {
    match category {
        ExchangeFailureCategory::HookInitialization => ExchangeFailureKind::HookInitialization,
        ExchangeFailureCategory::HookTimedOut => ExchangeFailureKind::HookTimedOut,
        ExchangeFailureCategory::HookPanicked => ExchangeFailureKind::HookPanicked,
        ExchangeFailureCategory::HookAbortedPolicy => {
            ExchangeFailureKind::HookAborted(HookAbort::Policy("Simulated".into()))
        }
        ExchangeFailureCategory::HookAbortedRejected => {
            ExchangeFailureKind::HookAborted(HookAbort::Rejected)
        }
        ExchangeFailureCategory::Cancelled => ExchangeFailureKind::Cancelled,
        ExchangeFailureCategory::Body => ExchangeFailureKind::Body,
        ExchangeFailureCategory::RequestTranslation => ExchangeFailureKind::RequestTranslation,
        ExchangeFailureCategory::ResponseTranslation => ExchangeFailureKind::ResponseTranslation,
        ExchangeFailureCategory::Route => ExchangeFailureKind::Route,
        ExchangeFailureCategory::Upstream => ExchangeFailureKind::Upstream,
        ExchangeFailureCategory::Shutdown => ExchangeFailureKind::Shutdown,
    }
}
