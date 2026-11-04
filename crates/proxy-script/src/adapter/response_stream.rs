//! Exchange-local pending bytes for the incremental script callback.
use super::{
    Arc, BodyHookError, BoxBodyFuture, BufferedHead, Bytes, HeaderBlock, HookAbort, RequestHead,
    ResponseHead, ScriptAction, ScriptHandler, ScriptHeaderOperation, ScriptReport, Shared,
    apply_header_operations, failure_report, report, script_failure_abort,
};
use bytes::BytesMut;
use transmog_core::{BodyFrame, intercept::BodyFilter};

pub(super) struct ScriptResponseStream {
    shared: Arc<Shared>,
    request: RequestHead,
    response: ResponseHead,
    local: bool,
    deferred: bool,
    pending: BytesMut,
    trailers: Option<HeaderBlock>,
    head_edit: Option<BufferedHead>,
    reports: Vec<ScriptReport>,
    report_bytes: usize,
    report_count: usize,
    reports_omitted: bool,
}

impl ScriptResponseStream {
    pub(super) fn new(
        shared: Arc<Shared>,
        request: RequestHead,
        response: ResponseHead,
        local: bool,
        deferred: bool,
    ) -> Self {
        Self {
            shared,
            request,
            response,
            local,
            deferred,
            pending: BytesMut::new(),
            trailers: None,
            head_edit: None,
            reports: Vec::new(),
            report_bytes: 0,
            report_count: 0,
            reports_omitted: false,
        }
    }

    fn record_report(&mut self, report: ScriptReport) {
        let bytes = serde_json::to_vec(&report).map_or(usize::MAX, |bytes| bytes.len());
        if self.report_count < 128 && bytes <= (1024 * 1024usize).saturating_sub(self.report_bytes)
        {
            self.report_bytes += bytes;
            self.report_count += 1;
            self.reports.push(report);
        } else if !self.reports_omitted {
            self.reports_omitted = true;
            self.reports.push(ScriptReport {
                details_omitted: true,
                diagnostics: vec!["Additional streaming findings and diagnostics omitted after the exchange budget was reached".into()],
                ..ScriptReport::default()
            });
        }
    }

    fn release_headers(&mut self, headers: &[ScriptHeaderOperation], status: Option<u16>) {
        if !headers.is_empty() || status.is_some() {
            apply_header_operations(&mut self.response.headers, headers)
                .expect("validated head edits");
            if let Some(status) = status {
                self.response.status = status;
            }
            self.head_edit = Some(BufferedHead::Response(self.response.clone()));
        }
        self.deferred = false;
    }

    async fn feed(&mut self, input: Bytes, end: bool) -> Result<Vec<BodyFrame>, BodyHookError> {
        let limit = self.shared.manifest.limits.max_input_body_bytes;
        let mut read = 0;
        let mut output = Vec::new();
        let mut output_bytes = 0usize;
        let mut calls = 0;
        loop {
            let take = (input.len() - read).min(limit - self.pending.len());
            self.pending.extend_from_slice(&input[read..read + take]);
            read += take;
            if self.pending.is_empty() && (!end || calls != 0) {
                break;
            }
            calls += 1;
            if calls > 1024 {
                return Err(BodyHookError::Failed(
                    "streaming callback work limit exceeded".into(),
                ));
            }
            if calls % 16 == 0 {
                tokio::task::yield_now().await;
            }
            let result = match self
                .shared
                .invoke_input(
                    ScriptHandler::ResponseChunk,
                    &self.request,
                    Some(&self.response),
                    None,
                    self.local,
                    None,
                    Some((&self.pending, end && read == input.len(), self.deferred)),
                )
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    self.reports.push(failure_report(&error));
                    return Err(BodyHookError::Abort(script_failure_abort(
                        &self.shared.manifest,
                        &error,
                    )));
                }
            };
            if let Some(report) = report(&result) {
                self.record_report(report);
            }
            let (consumed, bytes, headers, status) = match result.decision {
                ScriptAction::NeedMore => {
                    if read == input.len() {
                        break;
                    }
                    continue;
                }
                ScriptAction::Continue => (self.pending.len(), None, Vec::new(), None),
                ScriptAction::Forward { consumed } => (consumed, None, Vec::new(), None),
                ScriptAction::Edit { edits } => {
                    (self.pending.len(), edits.bytes, edits.headers, edits.status)
                }
                ScriptAction::Emit {
                    bytes,
                    consumed,
                    headers,
                    status,
                } => (
                    consumed.unwrap_or(self.pending.len()),
                    Some(bytes),
                    headers,
                    status,
                ),
                ScriptAction::Abort { reason } => {
                    return Err(BodyHookError::Abort(HookAbort::Policy(reason)));
                }
                ScriptAction::Respond { .. } => unreachable!("validated streaming decision"),
            };
            self.release_headers(&headers, status);
            let original = self.pending.split_to(consumed).freeze();
            let emitted = bytes.map_or(original, Bytes::from);
            output_bytes = output_bytes.saturating_add(emitted.len());
            if output_bytes > 16 * 1024 * 1024 {
                return Err(BodyHookError::Failed(
                    "streaming output batch exceeds 16 MiB".into(),
                ));
            }
            if !emitted.is_empty() {
                output.push(BodyFrame::Data(emitted));
            }
            if self.pending.is_empty() && read == input.len() {
                break;
            }
        }
        Ok(output)
    }
}

impl BodyFilter for ScriptResponseStream {
    fn head_pending(&self) -> bool {
        self.deferred
    }
    fn set_head(&mut self, head: Option<BufferedHead>) {
        if let Some(BufferedHead::Response(response)) = head {
            self.response = response;
        }
    }
    fn take_head_edit(&mut self) -> Option<BufferedHead> {
        self.head_edit.take()
    }
    fn take_script_reports(&mut self) -> Vec<ScriptReport> {
        std::mem::take(&mut self.reports)
    }
    fn debug_pause_clock(
        &self,
    ) -> Option<Arc<transmog_script_diagnostics::supervision::PauseClock>> {
        self.shared
            .runner
            .debug_pause_clock(&format!("{:032x}", self.shared.metadata.exchange_id.0))
    }
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            match frame {
                BodyFrame::Data(bytes) => self.feed(bytes, false).await,
                BodyFrame::Trailers(trailers) => {
                    self.trailers = Some(trailers);
                    Ok(Vec::new())
                }
            }
        })
    }
    fn finish(&mut self) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            let mut output = self.feed(Bytes::new(), true).await?;
            if let Some(trailers) = self.trailers.take() {
                output.push(BodyFrame::Trailers(trailers));
            }
            Ok(output)
        })
    }
}
