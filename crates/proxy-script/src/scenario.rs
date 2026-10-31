//! Finite synthetic exchanges using the production interceptor and body pipelines.

use super::*;
use std::sync::Mutex;
use transmog_core::intercept::{
    BodyPipeline, BodyPipelineLimits, BufferedHead, CompletedExchange, ExchangeFailure,
    ExchangeFailureCategory, ExchangeFailureKind, ExchangeStage, HookLimits,
    InterceptorChainFactory, RequestHeadOutcome, ResponseHeadOutcome,
};
use transmog_core::{
    BodyFrame, ConnectionId, HttpLegVersion, SessionId, SessionMetadata, StreamId,
};

/// A complete decoded fixture. No network, route authorization or asset lookup occurs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptScenario {
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

/// One phase's selection and execution evidence.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScenarioStep {
    /// Exported callback name.
    pub handler: ScriptHandler,
    /// Invoked, failed, or a concrete reason the callback was skipped.
    pub selection: String,
    /// State seen by the callback, before its result.
    pub state: BTreeMap<String, serde_json::Value>,
    /// Returned result, when successful.
    pub result: Option<ScriptResult>,
    /// Bounded runtime error, when unsuccessful.
    pub error: Option<ScriptFailure>,
}

/// Effective fixture messages and per-phase evidence from one fresh host.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptScenarioResult {
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
    /// Effective decoded response bytes.
    pub response_body: Vec<u8>,
    /// Whether the effective response is local.
    pub local_response: bool,
    /// Why the scenario stopped, when applicable.
    pub error: Option<String>,
}

const PHASES: [ScriptHandler; 6] = [
    ScriptHandler::RequestHead,
    ScriptHandler::RequestBody,
    ScriptHandler::ResponseHead,
    ScriptHandler::ResponseBody,
    ScriptHandler::Completed,
    ScriptHandler::Failed,
];

struct RecordingRunner {
    inner: Arc<dyn ScriptRunner>,
    steps: Mutex<Vec<ScriptScenarioStep>>,
    start: u64,
    duration: u64,
    manifest: ScriptManifest,
}
impl ScriptRunner for RecordingRunner {
    fn invoke(&self, mut event: ScriptInvocation) -> BoxScriptFuture<'_> {
        Box::pin(async move {
            let phase = event.context.handler;
            let index = PHASES
                .iter()
                .position(|p| *p == phase)
                .expect("known phase");
            let elapsed = self.duration * u64::try_from(index.min(4)).expect("small index") / 4;
            event.context.source = ScriptSource::Sandbox;
            event.context.started_unix_ms = self.start;
            event.context.elapsed_ms = elapsed;
            event.context.now_unix_ms = self.start + elapsed;
            let state = event.state.clone();
            let result = match prepare_invocation(&self.manifest, &mut event) {
                Ok(request) => self.inner.invoke(event).await.and_then(|result| {
                    validate_result(&self.manifest, phase, &request, &result)?;
                    Ok(result)
                }),
                Err(error) => Err(error),
            };
            let mut steps = self
                .steps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            steps[index] = ScriptScenarioStep {
                handler: phase,
                selection: if result.is_ok() { "invoked" } else { "failed" }.into(),
                state,
                result: result.as_ref().ok().cloned(),
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
                        "not-exported"
                    } else if !admitted {
                        "request-filter"
                    } else {
                        "not-reached"
                    }
                    .into(),
                    state: BTreeMap::new(),
                    result: None,
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
    let mut response_body = input.response_body;
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
                    .map_err(|e| e.to_string())?;
                request_body = run_body(&mut pipeline, &request_body).await?;
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
                response_body = frames(supplied.body);
                local = true;
            }
            RequestHeadOutcome::RespondStreaming { .. } => {
                return Err("Scenario cannot resolve streaming response assets".into());
            }
            RequestHeadOutcome::Abort(reason) => return Err(format!("{reason:?}")),
        }
        if let Some(category) = input.failure.filter(|_| !local) {
            outcome = "failed";
            chain
                .failed(ExchangeFailure {
                    metadata: Arc::clone(chain.context().metadata()),
                    stage: ExchangeStage::Upstream,
                    kind: failure_kind(category),
                    request_committed: false,
                    response_committed: false,
                    message: "Simulated upstream failure".into(),
                })
                .await;
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
                local.then(|| vec![BodyFrame::Data(Bytes::from(response_body.clone()))]),
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
                    response_body = frames(body);
                }
                local = local_response;
                response = Some(head);
            }
            ResponseHeadOutcome::Abort(reason) => return Err(format!("{reason:?}")),
        }
        let head = response
            .as_ref()
            .ok_or_else(|| "No response fixture".to_owned())?;
        if request.method.eq_ignore_ascii_case("HEAD")
            || (100..200).contains(&head.status)
            || matches!(head.status, 204 | 304)
            || (request.method.eq_ignore_ascii_case("CONNECT") && (200..300).contains(&head.status))
        {
            mark_skip(&recorder, ScriptHandler::ResponseBody, "body-forbidden");
            response_body.clear();
        } else {
            stage = ExchangeStage::ResponseBody;
            mark_body(
                &recorder,
                &manifest,
                ScriptHandler::ResponseBody,
                &head.headers,
                Some(head.status),
                local,
            );
            let mut pipeline = chain
                .response_body_pipeline(&request, head, local, BodyPipelineLimits::default())
                .await
                .map_err(|e| e.to_string())?;
            response_body = run_body(&mut pipeline, &response_body).await?;
            if let Some(BufferedHead::Response(head)) = pipeline.head() {
                response = Some(head.clone());
            }
            if response
                .as_ref()
                .is_some_and(|r| matches!(r.status, 204 | 304))
            {
                response_body.clear();
            }
        }
        chain
            .completed(CompletedExchange {
                metadata: Arc::clone(chain.context().metadata()),
                request_head: request.clone(),
                response_head: response
                    .clone()
                    .ok_or_else(|| "No response fixture".to_owned())?,
            })
            .await;
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
            chain
                .failed(ExchangeFailure {
                    metadata: Arc::clone(chain.context().metadata()),
                    stage,
                    kind,
                    request_committed: false,
                    response_committed: false,
                    message: "Scenario stopped".into(),
                })
                .await;
        }
    }
    let steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(ScriptScenarioResult {
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

fn mark_skip(recorder: &RecordingRunner, handler: ScriptHandler, reason: &str) {
    let mut steps = recorder
        .steps
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let step = steps
        .iter_mut()
        .find(|s| s.handler == handler)
        .expect("phase");
    if step.selection == "not-reached" {
        step.selection = reason.into();
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
        mark_skip(recorder, handler, "body-filter");
    }
}
async fn run_body(pipeline: &mut BodyPipeline, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = pipeline
        .process(BodyFrame::Data(Bytes::copy_from_slice(body)))
        .await
        .map_err(|e| e.to_string())?;
    output.extend(pipeline.finish().await.map_err(|e| e.to_string())?);
    Ok(frames(output))
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
