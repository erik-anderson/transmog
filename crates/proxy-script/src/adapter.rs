//! Validation and per-exchange execution of the script contract.
mod response_stream;

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use transmog_core::intercept::{BufferedHead, CompletedExchange, ExchangeFailure, ScriptReport};

use super::{
    Arc, BTreeMap, BodyHookError, BodyPlan, BoxBodyFuture, BoxHookFuture, BufferedBody,
    BufferedBodyHandler, Bytes, CanonicalResponse, ExchangeInterceptor, ExchangeMetadata,
    HeaderBlock, HeaderField, HookAbort, HookInitError, InterceptorFactory, NonZeroUsize,
    RequestBodyAction, RequestBodyEvent, RequestHead, RequestHeadAction, RequestHeadEvent,
    ResponseBodyAction, ResponseBodyEvent, ResponseHead, ResponseHeadAction, ResponseHeadEvent,
    ScriptAction, ScriptBody, ScriptBodyFilter, ScriptContext, ScriptFailure, ScriptHandler,
    ScriptHeaderOperation, ScriptInlineResponse, ScriptInvocation, ScriptManifest,
    ScriptResolvedResponse, ScriptResponse, ScriptResponseAssetResolver, ScriptResponseSource,
    ScriptResult, ScriptRunner, apply_header_operations, script_abort, script_failure_abort,
    script_headers, script_request, valid_asset_ref,
};

pub(super) struct ScriptFactory {
    pub manifest: Arc<ScriptManifest>,
    pub runner: Arc<dyn ScriptRunner>,
    pub assets: Option<Arc<dyn ScriptResponseAssetResolver>>,
}

impl InterceptorFactory for ScriptFactory {
    fn create(
        &self,
        metadata: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, HookInitError> {
        self.runner
            .exchange_open()
            .map_err(|failure| HookInitError::new(failure.message))?;
        Ok(Arc::new(ScriptInterceptor {
            shared: Arc::new(Shared {
                manifest: Arc::clone(&self.manifest),
                runner: Arc::clone(&self.runner),
                metadata: metadata.clone(),
                state: tokio::sync::Mutex::new(BTreeMap::new()),
                started: std::time::Instant::now(),
                request: Mutex::new(None),
                terminal: AtomicBool::new(false),
            }),
            admitted: AtomicBool::new(false),
            local: AtomicBool::new(false),
            assets: self.assets.clone(),
            report: Mutex::new(None),
            debug_admission: Mutex::new(None),
            response_mode: Mutex::new(None),
            defer_headers: AtomicBool::new(false),
        }))
    }
}

struct Shared {
    manifest: Arc<ScriptManifest>,
    runner: Arc<dyn ScriptRunner>,
    metadata: ExchangeMetadata,
    state: tokio::sync::Mutex<BTreeMap<String, serde_json::Value>>,
    started: std::time::Instant,
    request: Mutex<Option<RequestHead>>,
    terminal: AtomicBool,
}
impl Drop for Shared {
    fn drop(&mut self) {
        self.runner.exchange_closed(
            &format!("{:032x}", self.metadata.exchange_id.0),
            self.terminal.load(Ordering::Acquire),
        );
    }
}

impl Shared {
    async fn invoke(
        &self,
        handler: ScriptHandler,
        request: &RequestHead,
        response: Option<&ResponseHead>,
        body: Option<&Bytes>,
        local: bool,
        failure: Option<transmog_core::intercept::ExchangeFailureCategory>,
    ) -> Result<ScriptResult, ScriptFailure> {
        self.invoke_input(handler, request, response, body, local, failure, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn invoke_input(
        &self,
        handler: ScriptHandler,
        request: &RequestHead,
        response: Option<&ResponseHead>,
        body: Option<&Bytes>,
        local: bool,
        failure: Option<transmog_core::intercept::ExchangeFailureCategory>,
        chunk: Option<(&[u8], bool, bool)>,
    ) -> Result<ScriptResult, ScriptFailure> {
        let mut state = self.state.lock().await;
        let mut event = invocation(
            &self.manifest,
            &format!("{:032x}", self.metadata.exchange_id.0),
            handler,
            request,
            response,
            body,
        );
        event.state = state.clone();
        event.local_response = local;
        event.failure = failure;
        if let Some((bytes, end, can_edit_headers)) = chunk {
            event.bytes = Some(bytes.to_vec());
            event.end = end;
            event.can_edit_headers = can_edit_headers;
        }
        event.context.source = super::ScriptSource::Proxy;
        event.context.http_version = self.metadata.ingress_version;
        event.context.started_unix_ms = u64::try_from(
            self.metadata
                .started_at
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        event.context.elapsed_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let result = self.runner.invoke(event).await?;
        validate_result(&self.manifest, handler, request, &result)?;
        if let Some((bytes, end, editable)) = chunk {
            validate_stream_window(&self.manifest, &result, bytes.len(), end, editable)?;
        }
        if let Some(next) = &result.state {
            state.clone_from(next);
        }
        Ok(result)
    }
}

struct ScriptInterceptor {
    response_mode: Mutex<Option<super::ScriptBodyMode>>,
    defer_headers: AtomicBool,
    debug_admission: Mutex<Option<ScriptFailure>>,
    admitted: AtomicBool,
    local: AtomicBool,
    shared: Arc<Shared>,
    assets: Option<Arc<dyn ScriptResponseAssetResolver>>,
    report: Mutex<Option<ScriptReport>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn report(result: &ScriptResult) -> Option<ScriptReport> {
    (!result.annotations.is_empty() || !result.diagnostics.is_empty() || !result.logs.is_empty())
        .then(|| ScriptReport {
            logs: result.logs.clone(),
            details_omitted: false,
            annotations: result.annotations.clone(),
            diagnostics: result.diagnostics.clone(),
            exception: None,
        })
}

fn failure_report(failure: &ScriptFailure) -> ScriptReport {
    ScriptReport {
        logs: failure.logs.clone(),
        details_omitted: false,
        annotations: Vec::new(),
        diagnostics: vec![format!("Script callback failed: {:?}", failure.category)],
        exception: failure.exception.clone(),
    }
}

impl ScriptInterceptor {
    fn selected(&self, handler: ScriptHandler) -> bool {
        self.shared.manifest.handlers.contains(&handler) && self.admitted.load(Ordering::Acquire)
    }

    fn body_handler(
        &self,
        handler: ScriptHandler,
        request: RequestHead,
        response: Option<ResponseHead>,
        local: bool,
    ) -> BodyPlan {
        BodyPlan::Buffer {
            limit: NonZeroUsize::new(self.shared.manifest.limits.max_input_body_bytes)
                .expect("validated limit"),
            handler: Box::new(ScriptBodyHandler {
                shared: Arc::clone(&self.shared),
                handler,
                request,
                response,
                local,
                report: None,
            }),
        }
    }

    async fn terminal(
        &self,
        handler: ScriptHandler,
        request: RequestHead,
        response: Option<ResponseHead>,
        failure: Option<transmog_core::intercept::ExchangeFailureCategory>,
    ) {
        if self.selected(handler) {
            match self
                .shared
                .invoke(
                    handler,
                    &request,
                    response.as_ref(),
                    None,
                    self.local.load(Ordering::Acquire),
                    failure,
                )
                .await
            {
                Ok(result) => *lock(&self.report) = report(&result),
                Err(error) => {
                    *lock(&self.report) = Some(ScriptReport {
                        logs: error.logs,
                        details_omitted: false,
                        annotations: Vec::new(),
                        diagnostics: vec![format!(
                            "Terminal script callback failed: {:?}",
                            error.category
                        )],
                        exception: error.exception,
                    });
                }
            }
        }
        self.shared.state.lock().await.clear();
        self.shared.terminal.store(true, Ordering::Release);
    }
}

impl ExchangeInterceptor for ScriptInterceptor {
    fn prepare_debug_request(&self, head: &RequestHead) {
        if self.shared.manifest.prefilter.matches(head) {
            *lock(&self.debug_admission) = self
                .shared
                .runner
                .debug_admit(
                    &format!("{:032x}", self.shared.metadata.exchange_id.0),
                    &head.method,
                    &head.target.host,
                    &head.target.path,
                )
                .err();
        }
    }
    fn debug_pause_clock(
        &self,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        self.shared
            .runner
            .debug_pause_clock(&format!("{:032x}", self.shared.metadata.exchange_id.0))
    }
    fn take_script_report(&self) -> Option<ScriptReport> {
        lock(&self.report).take()
    }

    fn on_request_head(&self, event: RequestHeadEvent) -> BoxHookFuture<'_, RequestHeadAction> {
        self.admitted.store(
            self.shared.manifest.prefilter.matches(&event.head),
            Ordering::Release,
        );
        *lock(&self.shared.request) = Some(event.head.clone());
        Box::pin(async move {
            if let Some(error) = lock(&self.debug_admission).take() {
                *lock(&self.report) = Some(failure_report(&error));
                return RequestHeadAction::Abort(script_failure_abort(
                    &self.shared.manifest,
                    &error,
                ));
            }
            if !self.selected(ScriptHandler::RequestHead) {
                return RequestHeadAction::Continue;
            }
            let result = match self
                .shared
                .invoke(
                    ScriptHandler::RequestHead,
                    &event.head,
                    None,
                    None,
                    false,
                    None,
                )
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    *lock(&self.report) = Some(failure_report(&error));
                    return RequestHeadAction::Abort(script_failure_abort(
                        &self.shared.manifest,
                        &error,
                    ));
                }
            };
            *lock(&self.report) = report(&result);
            match result.decision {
                ScriptAction::NeedMore
                | ScriptAction::Forward { .. }
                | ScriptAction::Emit { .. } => {
                    unreachable!("validated request head")
                }
                ScriptAction::Continue => RequestHeadAction::Continue,
                ScriptAction::Edit { edits } => {
                    let mut head = event.head;
                    let original = head.target.clone();
                    if apply_header_operations(&mut head.headers, &edits.headers).is_err() {
                        return RequestHeadAction::Abort(script_abort("invalid header edit"));
                    }
                    if let Some(method) = edits.method {
                        head.method = method;
                    }
                    if let Some(url) = edits.url {
                        head.target = target(&url).expect("validated URL");
                    }
                    *lock(&self.shared.request) = Some(head.clone());
                    if same_origin(&original, &head.target) {
                        RequestHeadAction::Replace(head)
                    } else {
                        RequestHeadAction::Reroute {
                            target: head.target.clone(),
                            head,
                        }
                    }
                }
                ScriptAction::Respond {
                    response: ScriptResponseSource::Inline(response),
                } => RequestHeadAction::Respond(inline(response, event.head.source_version)),
                ScriptAction::Respond {
                    response: ScriptResponseSource::Asset { asset_ref },
                } => match self
                    .assets
                    .as_ref()
                    .ok_or_else(|| "No response asset resolver".to_owned())
                    .and_then(|resolver| resolver.resolve(&asset_ref))
                {
                    Ok(ScriptResolvedResponse::Buffered(response)) => {
                        RequestHeadAction::Respond(response)
                    }
                    Ok(ScriptResolvedResponse::Streaming(response)) => {
                        RequestHeadAction::RespondStreaming(response)
                    }
                    Err(error) => RequestHeadAction::Abort(script_abort(&error)),
                },
                ScriptAction::Abort { reason } => {
                    RequestHeadAction::Abort(HookAbort::Policy(reason))
                }
            }
        })
    }

    fn on_response_head(&self, event: ResponseHeadEvent) -> BoxHookFuture<'_, ResponseHeadAction> {
        self.local.store(event.local_response, Ordering::Release);
        *lock(&self.shared.request) = Some(event.request_head.clone());
        Box::pin(async move {
            if !self.selected(ScriptHandler::ResponseHead) {
                return ResponseHeadAction::Continue;
            }
            let result = match self
                .shared
                .invoke(
                    ScriptHandler::ResponseHead,
                    &event.request_head,
                    Some(&event.head),
                    None,
                    event.local_response,
                    None,
                )
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    *lock(&self.report) = Some(failure_report(&error));
                    return ResponseHeadAction::Abort(script_failure_abort(
                        &self.shared.manifest,
                        &error,
                    ));
                }
            };
            *lock(&self.report) = report(&result);
            *lock(&self.response_mode) = result.body;
            self.defer_headers
                .store(result.defer_headers, Ordering::Release);
            match result.decision {
                ScriptAction::NeedMore
                | ScriptAction::Forward { .. }
                | ScriptAction::Emit { .. } => {
                    unreachable!("validated response head")
                }
                ScriptAction::Continue => ResponseHeadAction::Continue,
                ScriptAction::Edit { edits } => {
                    let mut head = event.head;
                    apply_header_operations(&mut head.headers, &edits.headers)
                        .expect("validated headers");
                    if let Some(status) = edits.status {
                        head.status = status;
                    }
                    ResponseHeadAction::Replace(head)
                }
                ScriptAction::Respond {
                    response: ScriptResponseSource::Inline(response),
                } => {
                    self.local.store(true, Ordering::Release);
                    ResponseHeadAction::Respond(inline(response, event.head.source_version))
                }
                ScriptAction::Abort { reason } => {
                    ResponseHeadAction::Abort(HookAbort::Policy(reason))
                }
                ScriptAction::Respond { .. } => unreachable!("validated response phase"),
            }
        })
    }

    fn on_request_body(&self, event: RequestBodyEvent) -> BoxHookFuture<'_, RequestBodyAction> {
        let selected = self.selected(ScriptHandler::RequestBody)
            && admits(
                &self.shared.manifest.request_body_filter,
                &event.head.headers,
                None,
                false,
            );
        let action = if selected {
            RequestBodyAction::decoded(self.body_handler(
                ScriptHandler::RequestBody,
                event.head,
                None,
                false,
            ))
        } else {
            RequestBodyAction::pass_through()
        };
        Box::pin(async move { action })
    }

    fn on_response_body(&self, event: ResponseBodyEvent) -> BoxHookFuture<'_, ResponseBodyAction> {
        self.local.store(event.local_response, Ordering::Release);
        let mode = lock(&self.response_mode)
            .unwrap_or_else(|| default_response_mode(&self.shared.manifest));
        let handler = if mode == super::ScriptBodyMode::Stream {
            ScriptHandler::ResponseChunk
        } else {
            ScriptHandler::ResponseBody
        };
        let selected = mode != super::ScriptBodyMode::Skip
            && self.selected(handler)
            && transmog_core::body_semantics(
                &event.request_head.method,
                event.response_head.status,
            ) == transmog_core::BodySemantics::Allowed
            && admits(
                &self.shared.manifest.response_body_filter,
                &event.response_head.headers,
                Some(event.response_head.status),
                event.local_response,
            );
        let action = if selected {
            ResponseBodyAction::decoded(if mode == super::ScriptBodyMode::Stream {
                BodyPlan::Transform(Box::new(response_stream::ScriptResponseStream::new(
                    Arc::clone(&self.shared),
                    event.request_head,
                    event.response_head,
                    event.local_response,
                    self.defer_headers.load(Ordering::Acquire),
                )))
            } else {
                self.body_handler(
                    ScriptHandler::ResponseBody,
                    event.request_head,
                    Some(event.response_head),
                    event.local_response,
                )
            })
        } else {
            ResponseBodyAction::pass_through()
        };
        Box::pin(async move { action })
    }

    fn on_completed(&self, outcome: CompletedExchange) -> BoxHookFuture<'_, ()> {
        Box::pin(self.terminal(
            ScriptHandler::Completed,
            outcome.request_head,
            Some(outcome.response_head),
            None,
        ))
    }

    fn on_failed(&self, failure: ExchangeFailure) -> BoxHookFuture<'_, ()> {
        let request = lock(&self.shared.request).clone();
        Box::pin(async move {
            if let Some(request) = request {
                self.terminal(
                    ScriptHandler::Failed,
                    request,
                    None,
                    Some(failure.kind.category_kind()),
                )
                .await;
            }
            self.shared.terminal.store(true, Ordering::Release);
        })
    }
}

struct ScriptBodyHandler {
    shared: Arc<Shared>,
    handler: ScriptHandler,
    request: RequestHead,
    response: Option<ResponseHead>,
    local: bool,
    report: Option<ScriptReport>,
}

impl BufferedBodyHandler for ScriptBodyHandler {
    fn debug_pause_clock(
        &self,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        self.shared
            .runner
            .debug_pause_clock(&format!("{:032x}", self.shared.metadata.exchange_id.0))
    }
    fn edits_head(&self) -> bool {
        true
    }
    fn script_report(&self) -> Option<ScriptReport> {
        self.report.clone()
    }
    fn on_body(
        &mut self,
        body: BufferedBody,
    ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>> {
        Box::pin(async move {
            let mut head = body.head().cloned().unwrap_or_else(|| {
                self.response.as_ref().map_or_else(
                    || BufferedHead::Request(self.request.clone()),
                    |head| BufferedHead::Response(head.clone()),
                )
            });
            let (request, response) = match &head {
                BufferedHead::Request(request) => (request, None),
                BufferedHead::Response(response) => (&self.request, Some(response)),
            };
            let result = self
                .shared
                .invoke(
                    self.handler,
                    request,
                    response,
                    Some(body.data()),
                    self.local,
                    None,
                )
                .await
                .map_err(|error| {
                    self.report = Some(failure_report(&error));
                    BodyHookError::Abort(script_failure_abort(&self.shared.manifest, &error))
                })?;
            self.report = report(&result);
            let mut data = body.data().clone();
            let mut trailers = body.trailers().cloned();
            match result.decision {
                ScriptAction::NeedMore
                | ScriptAction::Forward { .. }
                | ScriptAction::Emit { .. } => {
                    unreachable!("validated complete body")
                }
                ScriptAction::Continue => {}
                ScriptAction::Edit { edits } => {
                    let mut headers = head.headers().clone();
                    apply_header_operations(&mut headers, &edits.headers)
                        .expect("validated headers");
                    head.set_headers(headers);
                    if let (BufferedHead::Response(response), Some(status)) =
                        (&mut head, edits.status)
                    {
                        response.status = status;
                    }
                    if let Some(bytes) = edits.bytes {
                        data = Bytes::from(bytes);
                        trailers = None;
                    }
                }
                ScriptAction::Abort { reason } => {
                    return Err(BodyHookError::Abort(HookAbort::Policy(reason)));
                }
                ScriptAction::Respond { .. } => unreachable!("validated body phase"),
            }
            if let BufferedHead::Request(request) = &head {
                *lock(&self.shared.request) = Some(request.clone());
            }
            Ok(BufferedBody::try_new(
                // Unchanged input is bounded separately from replacement bytes.
                self.shared
                    .manifest
                    .limits
                    .max_output_bytes
                    .max(self.shared.manifest.limits.max_input_body_bytes),
                data,
                trailers,
            )?
            .with_head(head))
        })
    }
}

pub(super) fn invocation(
    manifest: &ScriptManifest,
    exchange_id: &str,
    handler: ScriptHandler,
    request: &RequestHead,
    response: Option<&ResponseHead>,
    body: Option<&Bytes>,
) -> ScriptInvocation {
    ScriptInvocation {
        bytes: None,
        end: false,
        can_edit_headers: false,
        context: ScriptContext {
            exchange_id: exchange_id.to_owned(),
            now_unix_ms: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            handler,
            http_version: request.source_version,
            ..ScriptContext::default()
        },
        request: script_request(request, manifest.capabilities.read_sensitive_headers),
        response: response.map(|head| ScriptResponse {
            status: head.status,
            headers: script_headers(&head.headers, manifest.capabilities.read_sensitive_headers),
        }),
        body: body.map(|bytes| ScriptBody {
            bytes: bytes.to_vec(),
        }),
        parameters: manifest
            .parameters
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.resolved()))
            .collect(),
        state: BTreeMap::new(),
        local_response: false,
        failure: None,
    }
}

pub(super) fn admits(
    filter: &ScriptBodyFilter,
    headers: &HeaderBlock,
    status: Option<u16>,
    local: bool,
) -> bool {
    if local && !filter.include_local {
        return false;
    }
    if !filter.statuses.is_empty() && !status.is_some_and(|s| filter.statuses.contains(&s)) {
        return false;
    }
    if filter.content_types.is_empty() {
        return true;
    }
    let mut values = headers.values("content-type");
    let Some(value) = values.next().and_then(|v| std::str::from_utf8(v).ok()) else {
        return false;
    };
    values.next().is_none()
        && filter
            .content_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(value.split(';').next().unwrap_or_default().trim()))
}

fn same_origin(left: &transmog_core::Target, right: &transmog_core::Target) -> bool {
    left.scheme.eq_ignore_ascii_case(&right.scheme)
        && left.host.eq_ignore_ascii_case(&right.host)
        && left.port == right.port
}

fn target(value: &str) -> Result<transmog_core::Target, ScriptFailure> {
    let url = url::Url::parse(value)
        .map_err(|_| ScriptFailure::invalid("invalid absolute request URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value.len() > 8192
        || value.chars().any(char::is_control)
    {
        return Err(ScriptFailure::invalid(
            "request URL must be HTTP(S), without user information or fragment",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ScriptFailure::invalid("request URL requires a host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    Ok(transmog_core::Target {
        scheme: url.scheme().to_owned(),
        authority: url[url::Position::BeforeHost..url::Position::AfterPort].to_owned(),
        host,
        port: url.port_or_known_default().expect("HTTP port"),
        path: url.path().to_owned(),
        query: url.query().map(str::to_owned),
    })
}

fn inline(
    response: ScriptInlineResponse,
    version: transmog_core::HttpLegVersion,
) -> CanonicalResponse {
    let mut headers = HeaderBlock::new();
    for (name, value) in response.headers {
        headers.push(HeaderField::try_new(name, value).expect("validated inline header"));
    }
    CanonicalResponse {
        head: ResponseHead {
            status: response.status,
            headers,
            source_version: version,
        },
        body: vec![transmog_core::BodyFrame::Data(Bytes::from(response.bytes))],
    }
}

fn managed(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "content-encoding"
            | "connection"
            | "proxy-connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

fn validate_headers(
    manifest: &ScriptManifest,
    operations: &[ScriptHeaderOperation],
) -> Result<(), ScriptFailure> {
    if operations.len() > 256
        || operations
            .iter()
            .map(|op| match op {
                ScriptHeaderOperation::Set { name, value }
                | ScriptHeaderOperation::Append { name, value } => name.len() + value.len(),
                ScriptHeaderOperation::Remove { name } => name.len(),
            })
            .sum::<usize>()
            > 128 * 1024
    {
        return Err(ScriptFailure::invalid("too many header edits"));
    }
    for operation in operations {
        if managed(operation.name())
            || !manifest
                .capabilities
                .write_headers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(operation.name()))
        {
            return Err(ScriptFailure::invalid("header edit is not authorized"));
        }
    }
    apply_header_operations(&mut HeaderBlock::new(), operations)
        .map_err(|()| ScriptFailure::invalid("invalid header edit"))
}

/// Validates a complete result before any traffic edit, report or state is applied.
///
/// # Errors
/// Rejects unauthorized decisions, phase mismatches and evidence exceeding its bounds.
#[allow(clippy::too_many_lines)] // One exhaustive decision table owns phase and authority validation.
pub fn validate_result(
    manifest: &ScriptManifest,
    handler: ScriptHandler,
    request: &RequestHead,
    result: &ScriptResult,
) -> Result<(), ScriptFailure> {
    let terminal = matches!(handler, ScriptHandler::Completed | ScriptHandler::Failed);
    let body = matches!(
        handler,
        ScriptHandler::RequestBody | ScriptHandler::ResponseBody | ScriptHandler::ResponseChunk
    );
    let response = matches!(
        handler,
        ScriptHandler::ResponseHead | ScriptHandler::ResponseBody | ScriptHandler::ResponseChunk
    );
    let invalid = || ScriptFailure::invalid("decision is not authorized for this phase");
    if manifest.api_revision < 4
        && matches!(
            result.decision,
            ScriptAction::NeedMore | ScriptAction::Forward { .. } | ScriptAction::Emit { .. }
        )
    {
        return Err(invalid());
    }
    if result.body.is_some() || result.defer_headers {
        if manifest.api_revision < 4
            || handler != ScriptHandler::ResponseHead
            || !matches!(
                result.decision,
                ScriptAction::Continue | ScriptAction::Edit { .. } | ScriptAction::Respond { .. }
            )
        {
            return Err(invalid());
        }
        let mode = result
            .body
            .unwrap_or_else(|| default_response_mode(manifest));
        if (mode == super::ScriptBodyMode::Buffer
            && !manifest.handlers.contains(&ScriptHandler::ResponseBody))
            || (mode == super::ScriptBodyMode::Stream
                && !manifest.handlers.contains(&ScriptHandler::ResponseChunk))
            || (result.defer_headers && mode != super::ScriptBodyMode::Stream)
        {
            return Err(ScriptFailure::invalid(
                "body selection requires the corresponding callback",
            ));
        }
    }
    if terminal && !matches!(result.decision, ScriptAction::Continue) {
        return Err(invalid());
    }
    if result.annotations.len() > 16
        || (!result.annotations.is_empty() && !manifest.capabilities.annotate)
        || result.annotations.iter().any(|a| {
            a.note.is_empty()
                || a.note.len() > 2048
                || a.tags.len() > 8
                || a.tags
                    .iter()
                    .any(|t| t.is_empty() || t.len() > 64 || t.chars().any(char::is_control))
        })
    {
        return Err(ScriptFailure::invalid(
            "invalid or unauthorized annotations",
        ));
    }
    if result.diagnostics.len() > 32
        || result.diagnostics.iter().map(String::len).sum::<usize>() > manifest.limits.max_log_bytes
    {
        return Err(ScriptFailure::invalid("diagnostic limit exceeded"));
    }
    if result.state.as_ref().is_some_and(|s| {
        s.len() > 128 || serde_json::to_vec(s).map_or(true, |v| v.len() > 64 * 1024)
    }) {
        return Err(ScriptFailure::invalid("exchange state exceeds 64 KiB"));
    }
    match &result.decision {
        ScriptAction::NeedMore | ScriptAction::Forward { .. } => {
            if handler != ScriptHandler::ResponseChunk {
                return Err(invalid());
            }
        }
        ScriptAction::Emit {
            bytes,
            headers,
            status,
            ..
        } => {
            if handler != ScriptHandler::ResponseChunk
                || !manifest.capabilities.write_body
                || bytes.len() > manifest.limits.max_output_bytes
                || status.is_some_and(|s| {
                    !manifest.capabilities.write_status || !(200..=599).contains(&s)
                })
            {
                return Err(invalid());
            }
            validate_headers(manifest, headers)?;
        }
        ScriptAction::Continue => {}
        ScriptAction::Edit { edits } => {
            validate_headers(manifest, &edits.headers)?;
            if let Some(method) = &edits.method
                && (handler != ScriptHandler::RequestHead
                    || !manifest.capabilities.rewrite_request
                    || method.is_empty()
                    || method.len() > 32
                    || !method
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)))
            {
                return Err(invalid());
            }
            if let Some(url) = &edits.url {
                if handler != ScriptHandler::RequestHead || !manifest.capabilities.rewrite_request {
                    return Err(invalid());
                }
                if !same_origin(&request.target, &target(url)?) && !manifest.capabilities.reroute {
                    return Err(ScriptFailure::invalid(
                        "cross-origin URL edits require rerouting authority",
                    ));
                }
            }
            if edits.status.is_some_and(|status| {
                !response || !manifest.capabilities.write_status || !(200..=599).contains(&status)
            }) {
                return Err(invalid());
            }
            if edits.bytes.as_ref().is_some_and(|bytes| {
                !body
                    || !manifest.capabilities.write_body
                    || bytes.len() > manifest.limits.max_output_bytes
            }) {
                return Err(invalid());
            }
        }
        ScriptAction::Respond {
            response: ScriptResponseSource::Inline(response),
        } => {
            if body
                || !manifest.capabilities.respond
                || !(200..=599).contains(&response.status)
                || response.bytes.len() > manifest.limits.max_output_bytes
                || response.headers.len() > 256
                || response
                    .headers
                    .iter()
                    .map(|(name, value)| name.len() + value.len())
                    .sum::<usize>()
                    > 128 * 1024
            {
                return Err(invalid());
            }
            for (name, value) in &response.headers {
                if managed(name) || HeaderField::try_new(name.as_str(), value.as_str()).is_err() {
                    return Err(ScriptFailure::invalid("invalid inline response header"));
                }
            }
        }
        ScriptAction::Respond {
            response: ScriptResponseSource::Asset { asset_ref },
        } => {
            if handler != ScriptHandler::RequestHead
                || !manifest.capabilities.respond
                || !valid_asset_ref(asset_ref)
            {
                return Err(invalid());
            }
        }
        ScriptAction::Abort { reason } => {
            if !manifest.capabilities.abort
                || reason.is_empty()
                || reason.len() > 256
                || reason.chars().any(char::is_control)
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

pub(crate) fn default_response_mode(manifest: &ScriptManifest) -> super::ScriptBodyMode {
    if manifest.handlers.contains(&ScriptHandler::ResponseBody) {
        super::ScriptBodyMode::Buffer
    } else if manifest.handlers.contains(&ScriptHandler::ResponseChunk) {
        super::ScriptBodyMode::Stream
    } else {
        super::ScriptBodyMode::Skip
    }
}

/// Validates streaming decisions against the exact pending window and header lease.
/// # Errors
/// Rejects impossible consumption, waiting at EOF/at capacity, and late head edits.
pub fn validate_stream_result(
    manifest: &ScriptManifest,
    event: &ScriptInvocation,
    result: &ScriptResult,
) -> Result<(), ScriptFailure> {
    if event.context.handler != ScriptHandler::ResponseChunk {
        return Ok(());
    }
    let len = event.bytes.as_ref().map_or(0, Vec::len);
    validate_stream_window(manifest, result, len, event.end, event.can_edit_headers)
}

pub(super) fn validate_stream_window(
    manifest: &ScriptManifest,
    result: &ScriptResult,
    len: usize,
    end: bool,
    editable: bool,
) -> Result<(), ScriptFailure> {
    let consumed = match &result.decision {
        ScriptAction::Forward { consumed } => Some(*consumed),
        ScriptAction::Emit { consumed, .. } => Some(consumed.unwrap_or(len)),
        _ => None,
    };
    if consumed.is_some_and(|consumed| consumed > len || (consumed == 0 && len != 0)) {
        return Err(ScriptFailure::invalid(
            "streaming consumption must make progress within pending input",
        ));
    }
    let (headers, status) = match &result.decision {
        ScriptAction::Edit { edits } => (&edits.headers, edits.status),
        ScriptAction::Emit {
            headers, status, ..
        } => (headers, *status),
        ScriptAction::NeedMore => {
            if end || len >= manifest.limits.max_input_body_bytes {
                return Err(ScriptFailure::invalid(
                    "cannot wait for more input at EOF or the pending-input limit",
                ));
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    if (!headers.is_empty() || status.is_some()) && !editable {
        return Err(ScriptFailure::invalid(
            "response headers have already been released",
        ));
    }
    Ok(())
}

/// Normalizes sandbox/test inputs and enforces phase, size and read authority.
/// Parameters always come from the immutable manifest.
///
/// # Errors
/// Rejects malformed or oversized input, or an undeclared phase.
#[allow(clippy::too_many_lines)] // Validate the complete event before exposing any field to the runtime.
pub fn prepare_invocation(
    manifest: &ScriptManifest,
    event: &mut ScriptInvocation,
) -> Result<RequestHead, ScriptFailure> {
    let phase = event.context.handler;
    let body_phase = matches!(
        phase,
        ScriptHandler::RequestBody | ScriptHandler::ResponseBody
    );
    let response_phase = matches!(
        phase,
        ScriptHandler::ResponseHead
            | ScriptHandler::ResponseBody
            | ScriptHandler::ResponseChunk
            | ScriptHandler::Completed
    );
    let chunk = phase == ScriptHandler::ResponseChunk;
    if !manifest.handlers.contains(&phase)
        || chunk != event.bytes.is_some()
        || (!chunk && (event.end || event.can_edit_headers))
        || event.bytes.as_ref().is_some_and(|bytes| {
            !manifest.capabilities.read_bodies || bytes.len() > manifest.limits.max_input_body_bytes
        })
        || body_phase != event.body.is_some()
        || response_phase != event.response.is_some()
        || (phase == ScriptHandler::Failed) != event.failure.is_some()
        || event.body.as_ref().is_some_and(|b| {
            !manifest.capabilities.read_bodies
                || b.bytes.len() > manifest.limits.max_input_body_bytes
        })
        || event.context.exchange_id.len() > 128
        || event.state.len() > 128
        || serde_json::to_vec(&event.state).map_or(true, |v| v.len() > 64 * 1024)
    {
        return Err(ScriptFailure::invalid(
            "invalid phase or input limit exceeded",
        ));
    }
    let request = &event.request;
    if request.method.is_empty()
        || request.method.len() > 32
        || !request
            .method
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !matches!(request.scheme.as_str(), "http" | "https")
        || request.port == 0
        || request.host.contains(['/', '@', '?', '#'])
        || !request.path.starts_with('/')
        || request.path.contains(['?', '#'])
    {
        return Err(ScriptFailure::invalid("invalid request fields"));
    }
    let host = if request.host.contains(':') {
        format!(
            "[{}]",
            request.host.trim_start_matches('[').trim_end_matches(']')
        )
    } else {
        request.host.clone()
    };
    let url = format!(
        "{}://{}:{}{}{}",
        request.scheme,
        host,
        request.port,
        request.path,
        request
            .query
            .as_ref()
            .map_or_else(String::new, |q| format!("?{q}"))
    );
    let mut head = RequestHead {
        method: request.method.clone(),
        target: target(&url)?,
        headers: HeaderBlock::new(),
        source_version: event.context.http_version,
    };
    for headers in std::iter::once(&mut event.request.headers)
        .chain(event.response.iter_mut().map(|r| &mut r.headers))
    {
        if headers.len() > 256
            || headers
                .iter()
                .map(|h| h.name.len() + h.value.len())
                .sum::<usize>()
                > 128 * 1024
        {
            return Err(ScriptFailure::invalid("header input limit exceeded"));
        }
        for header in headers {
            HeaderField::try_new(header.name.as_str(), header.value.clone())
                .map_err(|_| ScriptFailure::invalid("invalid header input"))?;
            header.sensitive |= matches!(
                header.name.to_ascii_lowercase().as_str(),
                "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
            );
            if header.sensitive && !manifest.capabilities.read_sensitive_headers {
                header.value.clear();
                header.redacted = true;
            }
        }
    }
    if event
        .response
        .as_ref()
        .is_some_and(|r| !(100..=599).contains(&r.status))
    {
        return Err(ScriptFailure::invalid("invalid response status"));
    }
    for h in &event.request.headers {
        head.headers.push(
            HeaderField::try_new(h.name.as_str(), h.value.clone())
                .map_err(|_| ScriptFailure::invalid("invalid request header"))?,
        );
    }
    event.parameters = manifest
        .parameters
        .iter()
        .map(|(name, p)| (name.clone(), p.resolved()))
        .collect();
    Ok(head)
}
