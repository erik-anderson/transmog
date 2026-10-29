//! Permission-aware product operations. The desktop supplies its existing proxy-start policy.
use crate::{
    AccessManager, CallFuture, Error, Grant, Handler, Permission, Scope,
    access::{now, random},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use transmog_app::{
    Application, BodyRangeRequest, CaptureStartRequest, ComposerBodySource, ComposerRequest,
    HeaderRuleChange, HeaderRulePlacement, HeaderRuleTestInput, SessionQueryInput,
    TextTransformTestInput, TrafficColumn, TrafficDimension, TrafficFilterGroup,
    TrafficFilterValue,
};
use transmog_automation::Rule;
mod authoring;
mod context;
mod inspection;
mod rule_tests;
mod sends;

/// Existing desktop proxy-start validation and host setup, injected without UI dependencies.
pub trait ProxyHost: Send + Sync + 'static {
    /// Starts using saved settings; certificate installation remains a separate desktop action.
    fn start(&self) -> CallFuture<'_>;
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Control {
    id: String,
    grant_id: String,
    grant_revision: u64,
    expires_at: u64,
    rules: Vec<String>,
    breakpoints: bool,
    controller_id: Option<String>,
}

#[derive(Default)]
struct Runtime {
    controls: HashMap<String, Control>,
    sends: HashMap<(String, String), sends::SendRecord>,
    recording: Option<(String, u64, PathBuf)>,
    activity: Vec<Value>,
    snapshots: std::collections::VecDeque<inspection::Snapshot>,
    contexts: std::collections::VecDeque<context::Context>,
}

/// Shared operations for one main workspace. Runtime control handles never outlive this instance.
pub struct AgentApplication {
    application: Application,
    access: AccessManager,
    root: PathBuf,
    instance_id: String,
    host: Arc<dyn ProxyHost>,
    operations: tokio::sync::Mutex<()>,
    runtime: Mutex<Runtime>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl AgentApplication {
    /// Creates an adapter and disables temporary agent rules left by an interrupted prior run.
    /// # Errors
    /// Returns entropy or recovery failure without starting a listener.
    pub fn new(
        application: Application,
        access: AccessManager,
        root: PathBuf,
        host: Arc<dyn ProxyHost>,
    ) -> Result<Arc<Self>, Error> {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|_| invalid("A workspace generation could not be generated."))?;
        let (changes, _) = tokio::sync::watch::channel(0);
        let result = Arc::new(Self {
            application,
            access,
            root,
            instance_id: format!("{:032x}", u128::from_le_bytes(nonce)),
            host,
            operations: tokio::sync::Mutex::new(()),
            runtime: Mutex::new(Runtime::default()),
            changes,
        });
        for rule in result.application.automation_status().rules {
            if rule.enabled && rule.id.starts_with("mcp-") {
                result.set_rule_enabled(&rule.id, rule.revision, false)?;
            }
        }
        Ok(result)
    }

    /// Stable identity for this main workspace's current lifetime.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Change hints for desktop rule lists; saved application state remains authoritative.
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Checks control leases periodically, including expiry and permission changes.
    pub fn start_maintenance(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let _operation = service.operations.lock().await;
                if let Err(error) = service.cleanup().await {
                    service.application.record_diagnostic(
                        transmog_app::DiagnosticLevel::Error,
                        "mcp",
                        "control-cleanup-failed",
                        &error.message,
                    );
                }
            }
        })
    }

    /// Immediately ends all temporary control. Called before disabling the listener or closing.
    /// # Errors
    /// Returns a rule persistence failure; unresolved effects stay visible for retry.
    pub async fn end_all(&self) -> Result<(), Error> {
        let _operation = self.operations.lock().await;
        let ids: Vec<_> = self.lock().controls.keys().cloned().collect();
        for id in ids {
            self.end_control(&id).await?;
        }
        self.end_recording().await?;
        self.lock().snapshots.clear();
        Ok(())
    }

    /// Safe current control/activity metadata for the trusted desktop management view.
    pub fn management_status(&self) -> Value {
        let runtime = self.lock();
        json!({"instanceId":self.instance_id,"controls":runtime.controls.values().collect::<Vec<_>>(),"activity":runtime.activity})
    }

    /// Number of temporary leases for the trusted desktop's compact status indicator.
    pub fn active_control_count(&self) -> usize {
        self.lock().controls.len()
    }

    /// Metadata for reviewing temporary changes in the trusted desktop UI.
    pub fn temporary_rules(&self) -> Value {
        json!(self.application.automation_status().rules.into_iter().filter(|rule|rule.id.starts_with("mcp-")).map(|rule|json!({"id":rule.id,"revision":rule.revision,"name":rule.display_name.unwrap_or_else(||"Agent rule".into()),"enabled":rule.enabled})).collect::<Vec<_>>())
    }

    /// Ends leases invalidated by a trusted key-management action.
    /// # Errors
    /// Returns a cleanup failure without hiding remaining temporary effects.
    pub async fn reconcile(&self) -> Result<(), Error> {
        let _operation = self.operations.lock().await;
        self.cleanup().await
    }

    /// Ends a control session from the trusted desktop management panel.
    /// # Errors
    /// Returns a cleanup failure; this method is never exposed through unauthenticated IPC.
    pub async fn end_from_desktop(&self, id: &str) -> Result<(), Error> {
        let _operation = self.operations.lock().await;
        self.end_control(id).await
    }

    /// Converts one temporary rule to ordinary saved configuration after a desktop user action.
    /// # Errors
    /// Rejects stale selections or invalid/persistence-conflicting automation.
    pub async fn keep_rule(&self, id: &str, revision: u64) -> Result<(), Error> {
        let _operation = self.operations.lock().await;
        let status = self.application.automation_status();
        let mut document = transmog_app::AutomationRuleSet {
            generation: status.generation,
            rules: status.rules,
            autoresponses_enabled: status.autoresponses_enabled,
            ..transmog_app::AutomationRuleSet::default()
        };
        let rule = document
            .rules
            .iter_mut()
            .find(|rule| rule.id == id && rule.revision == revision && rule.id.starts_with("mcp-"))
            .ok_or_else(|| {
                Error::new(
                    "conflict",
                    "The selected temporary rule changed. Refresh and review it again.",
                )
            })?;
        rule.id = format!("rule-{}", random(12)?);
        rule.revision = 1;
        let candidate = self
            .application
            .validate_automation(document)
            .map_err(app_error)?;
        self.application
            .activate_automation(&candidate.candidate_id)
            .map_err(app_error)?;
        self.changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One authorization dispatch makes each tool's effects auditable together."
    )]
    async fn handle(&self, grant: Grant, operation: &str, input: Value) -> Result<Value, Error> {
        validate_arguments(operation, &input)?;
        if operation == "transmog_sensitive_access_request" {
            return self
                .access
                .request_sensitive_access(&grant, string(&input, "reason")?);
        }
        if operation == "transmog_sensitive_access_status" {
            return self.access.sensitive_access_status(&grant);
        }
        if operation == "transmog_request_send" {
            return self.send(&grant, &input).await;
        }
        if operation == "transmog_traffic_wait" {
            let result = self.wait_traffic(&grant, &input).await;
            self.activity(&grant, operation, result.is_ok());
            return result;
        }
        if operation == "transmog_traffic_search" {
            let result = self.search_traffic(&grant, &input).await;
            self.activity(&grant, operation, result.is_ok());
            return result;
        }
        let _operation = self.operations.lock().await;
        self.cleanup().await?;
        let result = match operation {
            "transmog_connect" | "transmog_status" => Ok(
                json!({"connected":true,"instanceId":self.instance_id,"grant":{"id":grant.id,"name":grant.name,"permissions":grant.permissions,"scope":grant.scope,"expiresAt":grant.expires_at,"omitSensitiveValues":grant.omit_sensitive_values},"sensitiveAccess":self.access.sensitive_access_status(&grant)?,"proxy":self.application.status(),"recordingActive":matches!(self.application.capture_status(), transmog_app::CaptureReadModel::Active{..}),"limits":{"pageSize":200,"bodyReadBytes":65536,"controlLeaseSeconds":600}}),
            ),
            "transmog_traffic_list" => self.traffic_page(&grant, &input),
            "transmog_exchange_get" => self.exchange(&grant, &input),
            "transmog_exchanges_get" => self.exchanges(&grant, &input),
            "transmog_body_extract" => self.extract_body(&grant, &input).await,
            "transmog_request_prepare" => self.prepare_request(&grant, &input),
            "transmog_stream_list" => self.stream_list(&grant, &input),
            "transmog_stream_read" => self.stream_read(&grant, &input),
            "transmog_headers_read" => self.headers_page(&grant, &input),
            "transmog_context_read" => self.read_context(&grant, &input),
            "transmog_request_status" => self.send_status(&grant, &input),
            "transmog_body_read" => self.body(&grant, &input, "id").await,
            "transmog_exchange_compare" => self.compare(&grant, &input).await,
            "transmog_proxy_start" => {
                grant.require(Permission::Collect)?;
                self.host.start().await
            }
            "transmog_proxy_stop" => {
                grant.require(Permission::Collect)?;
                value(self.application.stop_proxy().await.map_err(app_error)?)
            }
            "transmog_recording_start" => self.start_recording(&grant, &input).await,
            "transmog_recording_stop" => self.stop_recording(&grant).await,
            "transmog_rule_prepare" => self.prepare_rule(&grant, &input),
            "transmog_rules_list" => self.list_rules(&grant, &input),
            "transmog_rule_get" => {
                self.check_instance(&input)?;
                self.read_rule(&grant, string(&input, "id")?)
            }
            "transmog_rule_update" => self.update_shared_rule(&grant, &input),
            "transmog_rule_test" => self.test_rule(&grant, &input).await,
            "transmog_control_begin" => {
                self.check_instance(&input)?;
                grant.require(Permission::Modify)?;
                if self.lock().controls.len() >= 16 {
                    return Err(invalid(
                        "End an existing control session before starting another.",
                    ));
                }
                let control = Control {
                    id: random(18)?,
                    grant_id: grant.id.clone(),
                    grant_revision: grant.revision,
                    expires_at: now() + integer(&input, "seconds", 120, 600)?.max(30),
                    rules: Vec::new(),
                    breakpoints: false,
                    controller_id: None,
                };
                self.lock()
                    .controls
                    .insert(control.id.clone(), control.clone());
                value(control)
            }
            "transmog_control_renew" => {
                let control = self.control(&grant, &input)?;
                let expires = now() + integer(&input, "seconds", 120, 600)?.max(30);
                let mut runtime = self.lock();
                let current = runtime
                    .controls
                    .get_mut(&control.id)
                    .ok_or_else(|| invalid("The control session expired."))?;
                current.expires_at = expires;
                value(current)
            }
            "transmog_control_end" => {
                let control = self.control(&grant, &input)?;
                self.end_control(&control.id).await?;
                Ok(json!({"ended":true}))
            }
            "transmog_rule_activate" => {
                let control = self.control(&grant, &input)?;
                let rule = self.owned_rule(&grant, string(&input, "id")?)?;
                Self::check_rule_scope(&grant, &rule)?;
                let expected = integer(&input, "expectedRevision", 0, u64::MAX)?;
                if self
                    .lock()
                    .controls
                    .values()
                    .any(|owner| owner.id != control.id && owner.rules.contains(&rule.id))
                {
                    return Err(Error::new(
                        "conflict",
                        "This rule already belongs to another control session.",
                    ));
                }
                // Record ownership before publishing a live effect, so cancellation cannot orphan it.
                self.lock()
                    .controls
                    .get_mut(&control.id)
                    .ok_or_else(|| invalid("The control session expired."))?
                    .rules
                    .push(rule.id.clone());
                self.set_rule_enabled(&rule.id, expected, true)
            }
            "transmog_rule_disable" => {
                self.check_instance(&input)?;
                let rule = self.editable_rule(&grant, string(&input, "id")?)?;
                if rule.enabled {
                    grant.require(Permission::Modify)?;
                }
                self.set_rule_enabled(
                    &rule.id,
                    integer(&input, "expectedRevision", 0, u64::MAX)?,
                    false,
                )
            }
            "transmog_breakpoints_set" => self.set_breakpoints(&grant, &input),
            "transmog_paused_list" => {
                let control = self.control(&grant, &input)?;
                if !control.breakpoints {
                    return Err(denied(
                        "This control session does not own the breakpoint controller.",
                    ));
                }
                grant.require(Permission::ReadBodies)?;
                grant.require(Permission::ReadCredentials)?;
                let controller = control.controller_id.as_deref().ok_or_else(|| {
                    denied("The breakpoint controller is no longer owned by this session.")
                })?;
                let status = self
                    .application
                    .paused_exchanges_for(controller)
                    .map_err(app_error)?;
                let total = status.paused.len();
                let offset = index(&input, "offset", 0, 256)?;
                let limit = index(&input, "limit", 1, 4)?.max(1);
                Ok(
                    json!({"controllerId":controller,"paused":status.paused.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"totalPaused":total,"nextOffset":(offset+limit<total).then_some(offset+limit)}),
                )
            }
            "transmog_paused_decide" => {
                let control = self.control(&grant, &input)?;
                if !control.breakpoints {
                    return Err(denied(
                        "This control session does not own the breakpoint controller.",
                    ));
                }
                let decision = parse(input["decision"].clone())?;
                let controller = control.controller_id.as_deref().ok_or_else(|| {
                    denied("The breakpoint controller is no longer owned by this session.")
                })?;
                let status = self
                    .application
                    .decide_breakpoint_for(controller, decision)
                    .map_err(app_error)?;
                Ok(json!({"controllerId":controller,"pendingCount":status.paused.len()}))
            }
            _ => Err(invalid("Unknown Transmog operation.")),
        };
        self.activity(&grant, operation, result.is_ok());
        result
    }

    fn check_instance(&self, input: &Value) -> Result<(), Error> {
        if input.get("instanceId").and_then(Value::as_str) == Some(self.instance_id.as_str()) {
            Ok(())
        } else {
            Err(Error::new(
                "stale_instance",
                "This reference belongs to another Transmog run. Reconnect and find the request again.",
            ))
        }
    }

    fn entry(
        &self,
        grant: &Grant,
        input: &Value,
        field: &str,
    ) -> Result<transmog_app::SessionDetail, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Inspect)?;
        let id = string(input, field)?;
        // Always authorize from complete source evidence, never a truncated display URL.
        let target = self
            .application
            .copy_message_url(id, "client-request")
            .map_err(app_error)?;
        if !grant.scope.allows_entry(&self.instance_id, id, &target) {
            return Err(denied("This request is outside this key's traffic scope."));
        }
        self.application.session_detail(id).map_err(app_error)
    }

    fn detail(&self, grant: &Grant, input: &Value, field: &str) -> Result<Value, Error> {
        let mut detail = value(self.entry(grant, input, field)?)?;
        // These are UI cache fingerprints over full auth sources, not evidence
        // needed by an agent; avoid exposing credential-derived digests.
        if let Some(fields) = detail.as_object_mut() {
            fields.remove("authRevisions");
        }
        mask_headers(&mut detail, grant.allows(Permission::ReadCredentials));
        Ok(detail)
    }

    async fn body(&self, grant: &Grant, input: &Value, id_field: &str) -> Result<Value, Error> {
        self.entry(grant, input, id_field)?;
        grant.require(Permission::ReadBodies)?;
        let id = string(input, id_field)?;
        let boundary = string(input, "boundary")?;
        let offset = input.get("offset").and_then(Value::as_str).unwrap_or("0");
        let offset_number = offset
            .parse::<u64>()
            .map_err(|_| invalid("Choose a nonnegative decimal byte offset."))?;
        let max = index(input, "maxBytes", 16384, 65536)?.max(1);
        if input
            .get("decodeContent")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let body = self
                .application
                .complete_body_context(id, boundary)
                .await
                .map_err(app_error)?;
            let url = self
                .application
                .copy_message_url(id, "client-request")
                .map_err(app_error)?;
            let text = body.text_encoding.is_some();
            let reference = self.retain_context(
                grant,
                context::ContextInput {
                    bytes: body.bytes,
                    required: vec![Permission::Inspect, Permission::ReadBodies],
                    source: json!({"kind":"body","id":id,"boundary":boundary}),
                    metadata: value(body.metadata)?,
                    url,
                    entry_id: Some(id.into()),
                    representation: if text {
                        "decoded-text"
                    } else {
                        "decoded-bytes"
                    },
                    text,
                },
            )?;
            return self.read_context(grant,&json!({"instanceId":self.instance_id,"contextId":reference["contextId"],"offset":offset_number.to_string(),"maxBytes":max}));
        }
        let page = self
            .application
            .read_body_range(BodyRangeRequest {
                session_id: id.into(),
                boundary: boundary.into(),
                offset: offset.into(),
                max_bytes: Some(max),
            })
            .await
            .map_err(app_error)?;
        Ok(
            json!({"metadata":page.metadata,"offset":page.offset.to_string(),"totalBytes":page.total_bytes.to_string(),"encoding":"base64","data":STANDARD.encode(&page.bytes),"nextOffset":(page.offset+(page.bytes.len() as u64)<page.total_bytes).then(||(page.offset+page.bytes.len() as u64).to_string())}),
        )
    }

    async fn compare(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        let mut left = self.detail(grant, input, "leftId")?;
        let mut right = self.detail(grant, input, "rightId")?;
        hide_bodies(&mut left);
        hide_bodies(&mut right);
        let boundary = string(input, "boundary")?;
        let headers = |detail: &Value| -> Value {
            for side in ["requests", "responses"] {
                if let Some(heads) = detail[side].as_array()
                    && let Some(head) = heads.iter().find(|head| head["boundary"] == boundary)
                {
                    return head.clone();
                }
            }
            Value::Null
        };
        let left_head = headers(&left);
        let right_head = headers(&right);
        let normalized = |head: &Value| {
            let mut fields: Vec<String> = head["headers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|header| {
                    format!(
                        "{}:{}",
                        header["name"].as_str().unwrap_or_default().to_lowercase(),
                        header["value"]
                    )
                })
                .collect();
            fields.sort();
            fields
        };
        let state = if left_head.is_null() || right_head.is_null() {
            "unavailable"
        } else if normalized(&left_head) == normalized(&right_head)
            && left_head["status"] == right_head["status"]
            && left_head["method"] == right_head["method"]
            && left_head["target"] == right_head["target"]
        {
            "no-visible-differences"
        } else {
            "different"
        };
        let mut result = json!({"headers":{"state":state,"left":left_head,"right":right_head},"body":{"state":"not-compared","reason":"Read message bodies permission is required."},"coverage":"Bounded retained header views; full byte evidence remains available through explicit body reads."});
        if grant.allows(Permission::ReadBodies) {
            let left_body = self.body(grant, input, "leftId").await;
            let right_body = self.body(grant, input, "rightId").await;
            if let (Ok(left), Ok(right)) = (left_body, right_body) {
                let complete = left["metadata"]["availability"] == "complete"
                    && right["metadata"]["availability"] == "complete"
                    && left["nextOffset"].is_null()
                    && right["nextOffset"].is_null();
                result["body"] = json!({"state":if !complete{"incomplete"}else if left["data"]==right["data"]{"equal"}else{"different"},"left":left,"right":right});
            } else {
                result["body"] = json!({"state":"unavailable"});
            }
        }
        Ok(result)
    }

    async fn start_recording(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        grant.require(Permission::Collect)?;
        grant.require(Permission::ReadBodies)?;
        grant.require(Permission::ReadCredentials)?;
        if grant.scope != Scope::All {
            return Err(denied(
                "Native recording preserves the whole workspace and requires an all-traffic grant.",
            ));
        }
        let label = optional_string(input, "name")?
            .unwrap_or_default()
            .chars()
            .map(|character| {
                if character.is_alphanumeric() || character == '-' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let directory = self.root.join("agent-exports");
        std::fs::create_dir_all(&directory)
            .map_err(|_| invalid("The agent export directory could not be created."))?;
        let name = format!("recording-{label}-{}-{}.tmcap", now(), random(8)?);
        let path = directory.join(&name);
        self.application
            .start_capture(CaptureStartRequest {
                password: None,
                include_network_context: false,
                path: path.clone(),
                max_file_bytes: u64::MAX,
                retain_body_samples: true,
                redact_sensitive_headers: false,
            })
            .await
            .map_err(app_error)?;
        self.lock().recording = Some((grant.id.clone(), grant.revision, path));
        Ok(json!({"recording":true,"artifact":name}))
    }

    async fn stop_recording(&self, grant: &Grant) -> Result<Value, Error> {
        grant.require(Permission::Collect)?;
        let recording = self
            .lock()
            .recording
            .clone()
            .filter(|(owner, _, _)| owner == &grant.id)
            .ok_or_else(|| denied("This key did not start the active recording."))?;
        if !matches!(self.application.capture_status(),transmog_app::CaptureReadModel::Active{path,..} if path==recording.2)
        {
            return Err(invalid("That recording is no longer active."));
        }
        self.application.stop_capture().await.map_err(app_error)?;
        self.lock().recording = None;
        Ok(json!({"sealed":true,"artifact":recording.2.file_name().and_then(|name|name.to_str())}))
    }

    async fn end_recording(&self) -> Result<(), Error> {
        let recording = self.lock().recording.clone();
        if let Some((_, _, owned)) = recording {
            // Do not stop a replacement recording started by the human.
            if matches!(self.application.capture_status(),transmog_app::CaptureReadModel::Active{path,..} if path==owned)
            {
                self.application.stop_capture().await.map_err(app_error)?;
            }
            self.lock().recording = None;
        }
        Ok(())
    }

    fn list_rules(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        let offset = index(input, "offset", 0, 1024)?;
        let limit = index(input, "limit", 100, 100)?.max(1);
        let rows: Vec<_> = self
            .application
            .automation_status()
            .rules
            .into_iter()
            .filter(|rule| self.rule_relevant(grant, rule))
            .map(|rule| rule_summary(&rule))
            .collect();
        Ok(
            json!({"instanceId":self.instance_id,"total":rows.len(),"rules":rows.iter().skip(offset).take(limit).collect::<Vec<_>>(),"nextOffset":(offset+limit<rows.len()).then_some(offset+limit),"coverage":"Saved native rules, including disabled and temporary rules. Scripts and external interceptors can also affect traffic; their source is not exposed by this permission."}),
        )
    }

    fn rule_relevant(&self, grant: &Grant, rule: &Rule) -> bool {
        if matches!(&grant.scope, Scope::Requests { instance_id, .. } if instance_id != &self.instance_id)
        {
            return false;
        }
        let Some(host) = rule.matcher.host.as_deref() else {
            return true;
        };
        match &grant.scope {
            Scope::Requests { instance_id, ids } => {
                instance_id == &self.instance_id
                    && ids.iter().any(|id| {
                        self.application
                            .copy_message_url(id, "client-request")
                            .is_ok_and(|target| {
                                url::Url::parse(&target).is_ok_and(|target| {
                                    target
                                        .host_str()
                                        .is_some_and(|target| target.eq_ignore_ascii_case(host))
                                })
                            })
                    })
            }
            _ => grant.scope.allows_url(&format!("http://{host}/")),
        }
    }

    fn read_rule(&self, grant: &Grant, id: &str) -> Result<Value, Error> {
        let rule = self
            .application
            .automation_status()
            .rules
            .into_iter()
            .find(|rule| rule.id == id)
            .ok_or_else(|| Error::new("not_found", "The rule is no longer available."))?;
        if !self.rule_relevant(grant, &rule) {
            return Err(denied("The rule is outside this key's traffic scope."));
        }
        let mut omitted = Vec::new();
        if rule_has_credentials(&rule) && !grant.allows(Permission::ReadCredentials) {
            omitted.push("Credential-bearing header values require sensitive-header access.");
        }
        if (rule.request.replace_body.is_some()
            || rule.response.replace_body.is_some()
            || rule.response.text_transform.is_some())
            && !grant.allows(Permission::ReadBodies)
        {
            omitted.push("Authored body content requires Read message bodies permission.");
        }
        Ok(
            json!({"summary":rule_summary(&rule),"rule":if omitted.is_empty(){Some(&rule)}else{None},"definition":if omitted.is_empty(){authoring::definition(&rule)}else{None},"editable":omitted.is_empty() && self.editable_rule(grant,id).is_ok() && (!rule.enabled || grant.allows(Permission::Modify)),"omitted":omitted}),
        )
    }

    fn editable_rule(&self, grant: &Grant, id: &str) -> Result<Rule, Error> {
        let rule = if id.starts_with(&format!("mcp-{}-", grant.id)) {
            grant.require(Permission::PrepareChanges)?;
            self.owned_rule(grant, id)?
        } else {
            grant.require(Permission::EditSharedRules)?;
            if id.starts_with("mcp-") {
                return Err(denied(
                    "Only the owning key can edit a temporary rule. A human can keep it as a shared saved rule.",
                ));
            }
            self.application
                .automation_status()
                .rules
                .into_iter()
                .find(|rule| rule.id == id)
                .ok_or_else(|| Error::new("not_found", "The shared rule is no longer available."))?
        };
        Self::check_rule_scope(grant, &rule)?;
        if rule_has_credentials(&rule) && !grant.allows(Permission::ReadCredentials) {
            return Err(denied(
                "This rule contains credential-bearing header configuration. Sensitive header access is required to read or edit it.",
            ));
        }
        Ok(rule)
    }

    fn update_shared_rule(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::EditSharedRules)?;
        let id = string(input, "id")?;
        if id.starts_with("mcp-") {
            return Err(denied(
                "Use Prepare changes for your own temporary drafts. Only the owning key can edit a temporary rule.",
            ));
        }
        let previous = self.editable_rule(grant, id)?;
        let mut rule = Self::author_rule(&input["rule"], Some(&previous))?;
        if let Some(enabled) = input["enabled"].as_bool() {
            rule.enabled = enabled;
        }
        Self::check_rule_scope(grant, &rule)?;
        if previous.enabled || rule.enabled {
            grant.require(Permission::Modify)?;
        }
        if rule_has_credentials(&rule) && !grant.allows(Permission::ReadCredentials) {
            return Err(denied(
                "Sensitive header access is required to edit credential-bearing rules.",
            ));
        }
        if (previous.response.text_transform.is_some() || rule.response.text_transform.is_some())
            && !grant.allows(Permission::ReadBodies)
        {
            return Err(denied(
                "Read message bodies permission is required to edit authored body content.",
            ));
        }
        let expected = integer(input, "expectedRevision", 0, u64::MAX)?;
        if expected != previous.revision {
            return Err(Error::new(
                "conflict",
                "A human or another agent changed this rule. Read the latest revision and reconcile your changes.",
            ));
        }
        rule.revision = previous.revision;
        self.save_rule(rule, Some(expected))
    }

    fn owned_rule(&self, grant: &Grant, id: &str) -> Result<Rule, Error> {
        if !id.starts_with(&format!("mcp-{}-", grant.id)) {
            return Err(denied("This key may change only its own agent rules."));
        }
        self.application
            .automation_status()
            .rules
            .into_iter()
            .find(|rule| rule.id == id)
            .ok_or_else(|| Error::new("not_found", "The agent rule is no longer available."))
    }

    fn check_rule_scope(grant: &Grant, rule: &Rule) -> Result<(), Error> {
        let Some(host) = rule.matcher.host.as_deref() else {
            return Err(invalid("Agent rules require an explicit exact host."));
        };
        if !grant.scope.allows_url(&format!("http://{host}/")) {
            return Err(denied(
                "The rule host is outside this key's scope. Selected-request grants cannot modify future traffic.",
            ));
        }
        if transmog_automation::header_rule_phase(rule).is_none()
            && !transmog_automation::is_text_transform_rule(rule)
        {
            return Err(invalid(
                "Use a standalone header rule or literal response text replacement.",
            ));
        }
        Ok(())
    }

    fn save_rule(&self, rule: Rule, expected: Option<u64>) -> Result<Value, Error> {
        let status = self.application.automation_status();
        let id = rule.id.clone();
        let text = transmog_automation::is_text_transform_rule(&rule);
        let change = HeaderRuleChange::Save {
            rule,
            expected_revision: expected,
            placement: HeaderRulePlacement::Keep,
        };
        let status = if text {
            self.application
                .change_response_transform(status.generation, change)
        } else {
            self.application
                .change_header_rule(status.generation, change)
        }
        .map_err(app_error)?;
        self.changes
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        value(
            status
                .rules
                .into_iter()
                .find(|rule| rule.id == id)
                .ok_or_else(|| invalid("The saved rule could not be found."))?,
        )
    }

    fn prepare_rule(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::PrepareChanges)?;
        let expected = input
            .get("expectedRevision")
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| invalid("expectedRevision must be a nonnegative integer."))
            })
            .transpose()?;
        let previous = if expected.is_some() {
            let previous = self.owned_rule(grant, string(input, "id")?)?;
            if previous.enabled {
                grant.require(Permission::Modify)?;
            }
            Some(previous)
        } else {
            None
        };
        if input.get("id").is_some() && expected.is_none() {
            return Err(invalid(
                "id requires expectedRevision when revising a temporary rule.",
            ));
        }
        let mut rule = Self::author_rule(&input["rule"], previous.as_ref())?;
        if previous.is_none() {
            rule.id = format!("mcp-{}-{}", grant.id, random(9)?);
            rule.revision = 1;
        }
        Self::check_rule_scope(grant, &rule)?;
        rule.enabled = false;
        let prefix = format!("MCP · {} · ", grant.name);
        let name = rule.display_name.as_deref().unwrap_or("Traffic change");
        rule.display_name = Some(format!(
            "{prefix}{}",
            name.strip_prefix(&prefix).unwrap_or(name)
        ));
        self.save_rule(rule, expected)
    }

    fn set_rule_enabled(&self, id: &str, revision: u64, enabled: bool) -> Result<Value, Error> {
        let mut rule = self
            .application
            .automation_status()
            .rules
            .into_iter()
            .find(|rule| rule.id == id && rule.revision == revision)
            .ok_or_else(|| {
                Error::new(
                    "conflict",
                    "This rule changed. Inspect its current revision before applying it.",
                )
            })?;
        rule.enabled = enabled;
        self.save_rule(rule, Some(revision))
    }

    fn control(&self, grant: &Grant, input: &Value) -> Result<Control, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Modify)?;
        self.lock()
            .controls
            .get(string(input, "controlId")?)
            .filter(|control| {
                control.grant_id == grant.id
                    && control.grant_revision == grant.revision
                    && control.expires_at > now()
            })
            .cloned()
            .ok_or_else(|| {
                denied("This key does not own a current control session. Begin a new session.")
            })
    }

    async fn end_control(&self, id: &str) -> Result<(), Error> {
        let Some(control) = self.lock().controls.get(id).cloned() else {
            return Ok(());
        };
        if let Some(controller) = &control.controller_id {
            self.application.disable_breakpoints_for(controller).await;
        }
        for id in &control.rules {
            for _ in 0..3 {
                let Some(rule) = self
                    .application
                    .automation_status()
                    .rules
                    .into_iter()
                    .find(|rule| &rule.id == id && rule.enabled)
                else {
                    break;
                };
                match self.set_rule_enabled(id, rule.revision, false) {
                    Ok(_) => break,
                    Err(error) if error.code == "conflict" => {}
                    Err(error) => return Err(error),
                }
            }
            if self
                .application
                .automation_status()
                .rules
                .iter()
                .any(|rule| &rule.id == id && rule.enabled)
            {
                return Err(Error::new(
                    "conflict",
                    "A temporary rule is still active. Retry ending this control session.",
                ));
            }
        }
        self.lock().controls.remove(id);
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), Error> {
        self.sweep_contexts();
        let access = self.access.status();
        let expired: Vec<_> = self
            .lock()
            .controls
            .values()
            .filter(|control| {
                !access.enabled
                    || (control.breakpoints
                        && !self
                            .access
                            .has_sensitive_access(&control.grant_id, control.grant_revision))
                    || control.expires_at <= now()
                    || !access.grants.iter().any(|grant| {
                        grant.id == control.grant_id
                            && grant.revision == control.grant_revision
                            && !grant.revoked
                            && grant.expires_at.is_none_or(|expiry| expiry > now())
                    })
            })
            .map(|control| control.id.clone())
            .collect();
        for id in expired {
            self.end_control(&id).await?;
        }
        let recording_expired =
            self.lock()
                .recording
                .as_ref()
                .is_some_and(|(owner, revision, _)| {
                    !access.enabled
                        || !self.access.has_sensitive_access(owner, *revision)
                        || !access.grants.iter().any(|grant| {
                            grant.id == *owner
                                && grant.revision == *revision
                                && !grant.revoked
                                && grant.expires_at.is_none_or(|expiry| expiry > now())
                        })
                });
        if recording_expired {
            self.end_recording().await?;
        }
        Ok(())
    }

    fn set_breakpoints(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        let control = self.control(grant, input)?;
        grant.require(Permission::ReadBodies)?;
        grant.require(Permission::ReadCredentials)?;
        let host = string(input, "host")?;
        if !grant.scope.allows_url(&format!("http://{host}/")) {
            return Err(denied("The breakpoint host is outside this key's scope."));
        }
        let mut settings: transmog_app::BreakpointSettings = parse(input["settings"].clone())?;
        if settings.body_limit > 128 * 1024 || settings.max_pending > 8 {
            return Err(invalid(
                "Agent breakpoints require bodyLimit at most 131072 bytes and maxPending at most 8.",
            ));
        }
        settings.hosts = std::collections::BTreeSet::from([host.to_owned()]);
        let status = self
            .application
            .enable_breakpoints(&settings)
            .map_err(app_error)?;
        let controller_id = status.controller_id.clone();
        let mut runtime = self.lock();
        let current = runtime
            .controls
            .get_mut(&control.id)
            .ok_or_else(|| invalid("The control session expired."))?;
        current.breakpoints = true;
        current.controller_id.clone_from(&controller_id);
        Ok(json!({"enabled":status.enabled,"controllerId":controller_id}))
    }

    fn activity(&self, grant: &Grant, operation: &str, success: bool) {
        let mut runtime = self.lock();
        if runtime.activity.len() >= 100 {
            runtime.activity.remove(0);
        }
        runtime.activity.push(json!({"grantId":grant.id,"name":grant.name,"operation":operation,"success":success,"at":now()}));
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Handler for AgentApplication {
    fn call(&self, grant: Grant, operation: String, input: Value) -> CallFuture<'_> {
        Box::pin(async move { self.handle(grant, &operation, input).await })
    }
}

fn value(value: impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|_| invalid("The operation result could not be encoded."))
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value)
        .map_err(|_| invalid("The operation arguments have an invalid shape or value."))
}
fn string<'a>(input: &'a Value, name: &str) -> Result<&'a str, Error> {
    input
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 4096)
        .ok_or_else(|| invalid("A required text argument is missing or invalid."))
}
fn optional_string(input: &Value, name: &str) -> Result<Option<String>, Error> {
    input
        .get(name)
        .map(|value| {
            value
                .as_str()
                .filter(|text| text.len() <= 4096)
                .map(str::to_owned)
                .ok_or_else(|| invalid("A text argument exceeds its limit or is invalid."))
        })
        .transpose()
}
fn integer(input: &Value, name: &str, default: u64, max: u64) -> Result<u64, Error> {
    input.get(name).map_or(Ok(default), |value| {
        value
            .as_u64()
            .filter(|number| *number <= max)
            .ok_or_else(|| invalid("An integer argument exceeds its limit or is invalid."))
    })
}
fn index(input: &Value, name: &str, default: u64, max: u64) -> Result<usize, Error> {
    usize::try_from(integer(input, name, default, max)?)
        .map_err(|_| invalid("The requested position exceeds this platform's limit."))
}
fn invalid(message: &str) -> Error {
    Error::new("invalid_input", message)
}
fn denied(message: &str) -> Error {
    Error::new("permission_denied", message)
}
fn app_error(error: transmog_app::AppError) -> Error {
    Error::new(
        match error.category {
            transmog_app::ErrorCategory::Conflict => "conflict",
            transmog_app::ErrorCategory::InvalidInput => "invalid_input",
            transmog_app::ErrorCategory::Limit => "limit",
            _ => "operation_failed",
        },
        error.message,
    )
}
fn sensitive(name: &str) -> bool {
    // Normalize separators so APIKey, Api-Key and api_key share the same policy.
    // Conservative false positives are preferable to disclosing credential values.
    let normalized: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|value| value.to_ascii_lowercase())
        .collect();
    [
        "auth",
        "ticket",
        "token",
        "secret",
        "credential",
        "cookie",
        "apikey",
        "password",
        "passwd",
        "session",
        "csrf",
        "xsrf",
        "signature",
        "subscriptionkey",
        "privatekey",
        "accesskey",
        "bearer",
        "jwt",
        "saml",
        "dpop",
        "assertion",
        "passphrase",
    ]
    .iter()
    .any(|term| normalized.contains(term))
}

fn rule_has_credentials(rule: &Rule) -> bool {
    use transmog_automation::{HeaderCondition, HeaderOperation};
    rule.request
        .headers
        .iter()
        .chain(&rule.response.headers)
        .any(|operation| match operation {
            HeaderOperation::Set(field)
            | HeaderOperation::OverrideIfPresent(field)
            | HeaderOperation::Append(field) => sensitive(&String::from_utf8_lossy(field.name())),
            HeaderOperation::Remove(_) => false,
        })
        || rule
            .matcher
            .request_headers
            .iter()
            .chain(&rule.matcher.response_headers)
            .any(|predicate| {
                sensitive(&predicate.name)
                    && !matches!(predicate.condition, HeaderCondition::Exists)
            })
}

fn rule_summary(rule: &Rule) -> Value {
    json!({"id":rule.id,"revision":rule.revision,"name":rule.display_name,"enabled":rule.enabled,"host":rule.matcher.host,"method":rule.matcher.method,"pathPrefix":rule.matcher.path_prefix,"priority":rule.priority,"effects":{"requestHeaders":rule.request.headers.len(),"responseHeaders":rule.response.headers.len(),"requestBodyReplacement":rule.request.replace_body.is_some(),"responseBodyReplacement":rule.response.replace_body.is_some(),"responseTextReplacement":rule.response.text_transform.is_some(),"savedResponse":rule.request.response_asset.is_some(),"userAgentOverride":rule.request.user_agent.is_some(),"requestAbort":rule.request.abort_reason.is_some(),"responseAbort":rule.response.abort_reason.is_some(),"discardRequestBody":rule.request.discard_body,"discardResponseBody":rule.response.discard_body},"temporaryAgentRule":rule.id.starts_with("mcp-")})
}
fn mask_headers(value: &mut Value, reveal: bool) {
    match value {
        Value::Object(fields) => {
            if !reveal
                && fields
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(sensitive)
                && fields.contains_key("value")
            {
                fields.insert("value".into(), Value::String("[redacted]".into()));
                fields.insert("sensitive".into(), Value::Bool(true));
            }
            for value in fields.values_mut() {
                mask_headers(value, reveal);
            }
        }
        Value::Array(items) => {
            for value in items {
                mask_headers(value, reveal);
            }
        }
        _ => {}
    }
}
fn hide_bodies(detail: &mut Value) {
    if let Some(bodies) = detail.get_mut("bodies").and_then(Value::as_array_mut) {
        for body in bodies {
            if let Some(fields) = body.as_object_mut() {
                fields.remove("display");
                fields.insert("representation".into(), json!("metadata"));
            }
        }
    }
}
fn validate_arguments(operation: &str, input: &Value) -> Result<(), Error> {
    let spec = crate::tool_catalog()
        .into_iter()
        .find(|tool| tool.name == operation)
        .ok_or_else(|| invalid("Unknown Transmog tool."))?;
    crate::catalog::validation::validate(&spec.schema, input)
}

#[cfg(test)]
mod tests;
