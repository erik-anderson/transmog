//! Validation and per-exchange execution of the script contract.

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
        Ok(Arc::new(ScriptInterceptor {
            shared: Arc::new(Shared {
                manifest: Arc::clone(&self.manifest),
                runner: Arc::clone(&self.runner),
                metadata: metadata.clone(),
                state: tokio::sync::Mutex::new(BTreeMap::new()),
                started: std::time::Instant::now(),
                request: Mutex::new(None),
            }),
            admitted: AtomicBool::new(false),
            local: AtomicBool::new(false),
            assets: self.assets.clone(),
            report: Mutex::new(None),
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
        if let Some(next) = &result.state {
            state.clone_from(next);
        }
        Ok(result)
    }
}

struct ScriptInterceptor {
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
    (!result.annotations.is_empty() || !result.diagnostics.is_empty()).then(|| ScriptReport {
        annotations: result.annotations.clone(),
        diagnostics: result.diagnostics.clone(),
    })
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
                        annotations: Vec::new(),
                        diagnostics: vec![format!(
                            "Terminal script callback failed: {:?}",
                            error.category
                        )],
                    });
                }
            }
        }
        self.shared.state.lock().await.clear();
    }
}

impl ExchangeInterceptor for ScriptInterceptor {
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
                    return RequestHeadAction::Abort(script_failure_abort(
                        &self.shared.manifest,
                        &error,
                    ));
                }
            };
            *lock(&self.report) = report(&result);
            match result.decision {
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
                    return ResponseHeadAction::Abort(script_failure_abort(
                        &self.shared.manifest,
                        &error,
                    ));
                }
            };
            *lock(&self.report) = report(&result);
            match result.decision {
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
        let selected = self.selected(ScriptHandler::ResponseBody)
            && admits(
                &self.shared.manifest.response_body_filter,
                &event.response_head.headers,
                Some(event.response_head.status),
                event.local_response,
            );
        let action = if selected {
            ResponseBodyAction::decoded(self.body_handler(
                ScriptHandler::ResponseBody,
                event.request_head,
                Some(event.response_head),
                event.local_response,
            ))
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
                    BodyHookError::Abort(script_failure_abort(&self.shared.manifest, &error))
                })?;
            self.report = report(&result);
            let mut data = body.data().clone();
            let mut trailers = body.trailers().cloned();
            match result.decision {
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
            Ok(
                BufferedBody::try_new(
                    self.shared.manifest.limits.max_output_bytes,
                    data,
                    trailers,
                )?
                .with_head(head),
            )
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
        ScriptHandler::RequestBody | ScriptHandler::ResponseBody
    );
    let response = matches!(
        handler,
        ScriptHandler::ResponseHead | ScriptHandler::ResponseBody
    );
    let invalid = || ScriptFailure::invalid("decision is not authorized for this phase");
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
        ScriptHandler::ResponseHead | ScriptHandler::ResponseBody | ScriptHandler::Completed
    );
    if !manifest.handlers.contains(&phase)
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
