//! Active-view auth capabilities. Summaries never serialize the parser's private value store.
use crate::{AppError, Application, BodyAvailability, BodyStore, ErrorCategory, inspector};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_auth::{Body, Header, Input, Inspection, MAX_BODY, MAX_TOTAL, Source};
use transmog_core::HeaderBlock;
use transmog_session::SessionSnapshot;
mod related;
pub use related::{RelatedAuth, RelatedEntry};

/// Exact active pane selection, resolved within the invoking application's workspace.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthRequest {
    /// Explicit selected-body interpretation, or auto discovery by media type.
    #[serde(default = "automatic")]
    pub interpretation: String,
    /// Stable pane name, with at most eight panes per application.
    pub slot: String,
    /// Monotonically increasing canonical decimal generation.
    pub generation: String,
    /// Host-issued `WebView` document epoch.
    pub page_id: String,
    /// Exact retained exchange.
    pub session_id: String,
    /// Original/effective request/response boundary.
    pub boundary: String,
    /// Selected visible body scanning; false is metadata-only discovery.
    #[serde(default)]
    pub scan_body: bool,
}
fn automatic() -> String {
    "auto".into()
}
/// Capability binding for explicit value/source/related operations.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthHandle {
    /// Random owning application/workspace namespace.
    pub owner: String,
    /// Owning pane.
    pub slot: String,
    /// Active selection generation.
    pub generation: String,
    /// Host document epoch.
    pub page_id: String,
    /// Hash of selected retained evidence, including availability/redaction.
    pub revision: String,
}
/// Safe masked analysis and exact active-view capability.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthView {
    /// Binding for explicit actions.
    pub handle: AuthHandle,
    /// Masked parser output; private values are skipped by serialization.
    pub inspection: Arc<Inspection>,
}
/// Authorized navigation into the existing source inspector.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSource {
    /// Owning retained exchange.
    pub session_id: String,
    /// Exact message stage.
    pub boundary: String,
    /// Original occurrence and honest transformation provenance.
    pub source: Source,
}
#[derive(Clone)]
struct Active {
    request: AuthRequest,
    handle: AuthHandle,
    inspection: Arc<Inspection>,
}
struct Slot {
    generation: u64,
    canceled: Arc<AtomicBool>,
    active: Option<Active>,
}
pub(crate) struct Service {
    slots: Mutex<BTreeMap<String, Slot>>,
    visible: AtomicBool,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            slots: Mutex::new(BTreeMap::new()),
            visible: AtomicBool::new(true),
        }
    }
}
impl Service {
    pub(crate) fn reset(&self, visible: Option<bool>) {
        if visible == Some(true) {
            self.visible.store(true, Ordering::Release);
            return;
        }
        if let Some(visible) = visible {
            self.visible.store(visible, Ordering::Release);
        }
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for slot in slots.values_mut() {
            slot.canceled.store(true, Ordering::Release);
            slot.active = None;
        }
        if visible.is_none() {
            slots.clear();
        }
    }
    fn begin(&self, request: &AuthRequest) -> Result<Arc<AtomicBool>, AppError> {
        let generation = generation(&request.generation)?;
        if !self.visible.load(Ordering::Acquire)
            || request.slot.is_empty()
            || request.slot.len() > 64
        {
            return Err(unavailable());
        }
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        if slots.len() >= 8 && !slots.contains_key(&request.slot) {
            return Err(unavailable());
        }
        if let Some(previous) = slots.get(&request.slot) {
            if generation <= previous.generation {
                return Err(unavailable());
            }
            previous.canceled.store(true, Ordering::Release);
        }
        let canceled = Arc::new(AtomicBool::new(false));
        slots.insert(
            request.slot.clone(),
            Slot {
                generation,
                canceled: Arc::clone(&canceled),
                active: None,
            },
        );
        Ok(canceled)
    }
    fn publish(&self, active: Active, canceled: &Arc<AtomicBool>) -> Result<(), AppError> {
        let mut slots = self.slots.lock().map_err(|_| unavailable())?;
        let slot = slots
            .get_mut(&active.request.slot)
            .ok_or_else(unavailable)?;
        if canceled.load(Ordering::Acquire) || !Arc::ptr_eq(&slot.canceled, canceled) {
            return Err(unavailable());
        }
        slot.active = Some(active);
        Ok(())
    }
    fn active(&self, handle: &AuthHandle) -> Result<Active, AppError> {
        let slots = self.slots.lock().map_err(|_| unavailable())?;
        let slot = slots.get(&handle.slot).ok_or_else(unavailable)?;
        if slot.canceled.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        slot.active
            .as_ref()
            .filter(|active| active.handle == *handle)
            .cloned()
            .ok_or_else(unavailable)
    }
    fn cancel(&self, slot: &str, generation: &str) {
        let Ok(generation) = self::generation(generation) else {
            return;
        };
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if let Some(current) = slots.get_mut(slot) {
            if generation >= current.generation {
                current.generation = generation;
                current.canceled.store(true, Ordering::Release);
                current.active = None;
            }
        } else if slots.len() < 8 && slot.len() <= 64 {
            slots.insert(
                slot.into(),
                Slot {
                    generation,
                    canceled: Arc::new(AtomicBool::new(true)),
                    active: None,
                },
            );
        }
    }
}
fn generation(value: &str) -> Result<u64, AppError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(unavailable)
}
fn unavailable() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "Auth inspection or retained evidence changed; inspect the current selection again",
        false,
    )
}
fn block<'a>(
    snapshot: &'a SessionSnapshot,
    boundary: &str,
) -> Result<(&'a HeaderBlock, Option<&'a transmog_core::Target>), AppError> {
    let stage = inspector::parse_boundary(boundary)?;
    if let Some(head) = snapshot
        .request_heads
        .iter()
        .rev()
        .find(|h| h.boundary == stage)
    {
        return Ok((&head.head.headers, Some(&head.head.target)));
    }
    if let Some(head) = snapshot
        .response_heads
        .iter()
        .rev()
        .find(|h| h.boundary == stage)
    {
        return Ok((&head.head.headers, None));
    }
    Err(unavailable())
}
pub(crate) fn revision(
    snapshot: &SessionSnapshot,
    store: Option<&BodyStore>,
    boundary: &str,
) -> Result<String, AppError> {
    let (headers, target) = block(snapshot, boundary)?;
    let mut hash = Sha256::new();
    for header in headers.iter() {
        hash.update((header.name().len() as u64).to_le_bytes());
        hash.update(header.name());
        hash.update([u8::from(header.is_redacted())]);
        hash.update((header.value().len() as u64).to_le_bytes());
        hash.update(header.value());
    }
    if let Some(target) = target {
        hash.update((target.path.len() as u64).to_le_bytes());
        hash.update(target.path.as_bytes());
        hash.update([u8::from(target.query.is_some())]);
        if let Some(query) = &target.query {
            hash.update(query.as_bytes());
        }
    }
    if let Some(metadata) = store.and_then(|store| {
        store
            .metadata(snapshot.exchange_id)
            .into_iter()
            .find(|m| m.boundary == boundary)
    }) {
        hash.update(serde_json::to_vec(&metadata).map_err(|_| unavailable())?);
    }
    hash.update(snapshot.request_number.to_le_bytes());
    Ok(hex::encode(hash.finalize()))
}
#[allow(clippy::too_many_lines)] // One bounded selected-message read/HTTP-decoding admission path.
async fn input(
    snapshot: SessionSnapshot,
    store: Option<BodyStore>,
    request: AuthRequest,
    canceled: Arc<AtomicBool>,
) -> Result<Input, AppError> {
    let (headers, target) = block(&snapshot, &request.boundary)?;
    let target = target.map(|target| {
        if target
            .path
            .len()
            .saturating_add(target.query.as_ref().map_or(0, String::len))
            > transmog_auth::MAX_CANDIDATE
        {
            // A bounded over-budget marker reaches core coverage without copying a huge retained target.
            " ".repeat(transmog_auth::MAX_CANDIDATE + 1)
        } else {
            format!(
                "{}{}",
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |query| format!("?{query}"))
            )
        }
    });
    let mut used = target.as_ref().map_or(0, String::len);
    let mut retained = vec![];
    for (ordinal, field) in headers.iter().enumerate().take(100_001) {
        // Preserve original ordinals even for unrelated headers; do not duplicate their large values.
        let oversized_name = field.name().len() > transmog_auth::MAX_CANDIDATE;
        let relevant = !oversized_name
            && std::str::from_utf8(field.name()).is_ok_and(transmog_auth::is_candidate_header);
        let field_bytes = field.name().len().saturating_add(field.value().len());
        let limited = ordinal == 100_000
            || oversized_name
            || relevant
                && (field.value().len() > transmog_auth::MAX_CANDIDATE
                    || field_bytes > MAX_TOTAL.saturating_sub(used));
        let value = if relevant && !limited {
            field.value()
        } else {
            &[]
        };
        let name = if relevant && !limited {
            String::from_utf8_lossy(field.name()).into_owned()
        } else {
            String::new()
        };
        used = used.saturating_add(name.len()).saturating_add(value.len());
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        retained.push(Header {
            name,
            value: value.into(),
            redacted: field.is_redacted(),
            limited,
        });
    }
    let mut input = Input {
        headers: retained,
        target,
        body: None,
        body_status: if request.scan_body {
            "unavailable"
        } else {
            "skipped"
        }
        .into(),
        captured_at: snapshot
            .metadata
            .started_at
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok()),
    };
    if !request.scan_body {
        return Ok(input);
    }
    let Some(store) = store else {
        return Ok(input);
    };
    store.flush().map_err(|_| unavailable())?;
    let Some(metadata) = store
        .metadata(snapshot.exchange_id)
        .into_iter()
        .find(|m| m.boundary == request.boundary)
    else {
        return Ok(input);
    };
    input.body_status = match metadata.availability {
        BodyAvailability::Complete => "complete",
        BodyAvailability::Capturing => "pending",
        BodyAvailability::Truncated | BodyAvailability::Lost => "incomplete",
        BodyAvailability::Evicted => "evicted",
        BodyAvailability::Disabled => "disabled",
        BodyAvailability::QuotaOmitted => "omitted",
    }
    .into();
    if metadata.availability != BodyAvailability::Complete {
        return Ok(input);
    }
    if metadata.retained_bytes > MAX_BODY as u64 {
        input.body_status = "limited".into();
        return Ok(input);
    }
    let mime = metadata.media_type.clone().unwrap_or_default();
    if request.interpretation == "auto"
        && !(mime == "application/x-www-form-urlencoded"
            || mime == "application/json"
            || mime.ends_with("+json")
            || mime.ends_with("/xml")
            || mime.ends_with("+xml")
            || mime == "text/html")
    {
        input.body_status = "unsupported".into();
        return Ok(input);
    }
    if headers
        .iter()
        .any(|h| (h.name_eq("content-type") || h.name_eq("content-encoding")) && h.is_redacted())
    {
        input.body_status = "redacted".into();
        return Ok(input);
    }
    let content_types: Vec<_> = headers.values("content-type").collect();
    if content_types.windows(2).any(|pair| pair[0] != pair[1]) {
        input.body_status = "unsupported".into();
        return Ok(input);
    }
    let stack = transmog_content::ContentCodingStack::from_headers(
        headers,
        transmog_content::ContentLimits::default().max_coding_layers(),
    );
    if stack.as_ref().is_err()
        || stack.as_ref().is_ok_and(|stack| {
            !stack
                .encode_order()
                .map(transmog_content::ContentCoding::as_str)
                .eq(metadata.content_codings.iter().map(String::as_str))
        })
    {
        input.body_status = "unsupported".into();
        return Ok(input);
    }
    let boundary = inspector::parse_boundary(&request.boundary)?;
    let length = usize::try_from(metadata.retained_bytes).map_err(|_| unavailable())?;
    let bytes = tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::with_capacity(length);
        while bytes.len() < length {
            if canceled.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let size = (length - bytes.len()).min(store.range_read_limit());
            let page = store
                .read_paged_range(snapshot.exchange_id, boundary, bytes.len() as u64, size)
                .map_err(|_| unavailable())?;
            if page.bytes.len() != size {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&page.bytes);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| unavailable())??;
    let codings = metadata.content_codings.clone();
    let bytes = match crate::visualizers::decoding::decode(
        &codings,
        bytes,
        MAX_BODY,
        4 * 1024 * 1024,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(reason) => {
            input.body_status = match reason {
                transmog_extensions::DecodeFailure::Limited => "limited",
                transmog_extensions::DecodeFailure::Unsupported => "unsupported",
                _ => "incomplete",
            }
            .into();
            return Ok(input);
        }
    };
    let media_type = metadata
        .charset
        .map_or(mime.clone(), |charset| format!("{mime}; charset={charset}"));
    input.body = Some(Body {
        bytes,
        media_type,
        transforms: codings
            .into_iter()
            .rev()
            .map(|coding| format!("HTTP content decoding: {coding}"))
            .collect(),
    });
    Ok(input)
}
impl Application {
    /// Analyze exact retained sources in a visible pane, off the UI thread.
    /// # Errors
    /// Rejects stale generations, other-window identifiers, unavailable evidence and finite limits.
    pub async fn inspect_auth(&self, request: AuthRequest) -> Result<AuthView, AppError> {
        if !["auto", "jose", "saml", "ntlm", "negotiate"].contains(&request.interpretation.as_str())
        {
            return Err(unavailable());
        }
        if request.page_id != self.visualizer_page() {
            return Err(unavailable());
        }
        let canceled = self.auth.begin(&request)?;
        let (snapshot, store) = self.visualizer_evidence(&request.session_id)?;
        if let Some(store) = &store {
            store.flush().map_err(|_| unavailable())?;
        }
        let before = revision(&snapshot, store.as_ref(), &request.boundary)?;
        let input = input(snapshot, store, request.clone(), Arc::clone(&canceled)).await?;
        let worker = Arc::clone(&canceled);
        let interpretation = request.interpretation.clone();
        let inspection = Arc::new(
            tokio::task::spawn_blocking(move || {
                transmog_auth::inspect_as(input, worker, &interpretation)
            })
            .await
            .map_err(|_| unavailable())?,
        );
        let (snapshot, store) = self.visualizer_evidence(&request.session_id)?;
        if request.page_id != self.visualizer_page()
            || canceled.load(Ordering::Acquire)
            || revision(&snapshot, store.as_ref(), &request.boundary)? != before
        {
            return Err(unavailable());
        }
        let handle = AuthHandle {
            owner: self.preview_capture.clone(),
            slot: request.slot.clone(),
            generation: request.generation.clone(),
            page_id: request.page_id.clone(),
            revision: before,
        };
        self.auth.publish(
            Active {
                request,
                handle: handle.clone(),
                inspection: Arc::clone(&inspection),
            },
            &canceled,
        )?;
        Ok(AuthView { handle, inspection })
    }
    fn authorized_auth(&self, handle: &AuthHandle) -> Result<Active, AppError> {
        if handle.owner != self.preview_capture || handle.page_id != self.visualizer_page() {
            return Err(unavailable());
        }
        let active = self.auth.active(handle)?;
        let (snapshot, store) = self.visualizer_evidence(&active.request.session_id)?;
        if let Some(store) = &store {
            store.flush().map_err(|_| unavailable())?;
        }
        if revision(&snapshot, store.as_ref(), &active.request.boundary)? != handle.revision {
            self.auth.cancel(&handle.slot, &handle.generation);
            return Err(unavailable());
        }
        Ok(active)
    }
    /// Retrieve only one explicitly requested value, after retention and redaction reauthorization.
    /// # Errors
    /// Rejects changed evidence, expired capabilities and unknown value occurrences.
    pub fn auth_value(&self, handle: &AuthHandle, field: &str) -> Result<String, AppError> {
        let active = self.authorized_auth(handle)?;
        let value = active
            .inspection
            .value(field)
            .ok_or_else(unavailable)?
            .to_owned();
        self.authorized_auth(handle)?;
        Ok(value)
    }
    /// Copy an artifact's exact retained token into independent converter scratch input.
    /// # Errors
    /// Rejects stale generations, foreign documents, changed retention and unsupported artifacts.
    pub fn auth_converter_input(
        &self,
        handle: &AuthHandle,
        finding: &str,
    ) -> Result<crate::format_converter::ConverterText, AppError> {
        let active = self.authorized_auth(handle)?;
        let (text, operation) = active
            .inspection
            .converter_token(finding)
            .ok_or_else(unavailable)?;
        let result = crate::format_converter::ConverterText {
            text: text.into(),
            operation: Some(operation.into()),
        };
        self.authorized_auth(handle)?;
        Ok(result)
    }
    /// Read an explicitly requested bounded group for an Auth subtree copy.
    /// # Errors
    /// Rejects stale evidence, unknown fields and aggregate output exhaustion.
    pub fn auth_values(
        &self,
        handle: &AuthHandle,
        fields: &[String],
    ) -> Result<Vec<String>, AppError> {
        if fields.len() > 2048 {
            return Err(unavailable());
        }
        let active = self.authorized_auth(handle)?;
        let mut size = 0;
        let values = fields
            .iter()
            .map(|field| {
                let value = active.inspection.value(field).ok_or_else(unavailable)?;
                size += value.len();
                if size > MAX_TOTAL {
                    return Err(unavailable());
                }
                Ok(value.to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.authorized_auth(handle)?;
        Ok(values)
    }
    /// Resolve a finding into an original retained source navigation action.
    /// # Errors
    /// Rejects stale or unknown findings and cross-workspace capabilities.
    pub fn auth_source(&self, handle: &AuthHandle, finding: &str) -> Result<AuthSource, AppError> {
        fn find<'a>(items: &'a [transmog_auth::Finding], id: &str) -> Option<&'a Source> {
            for item in items {
                if item.id == id {
                    return Some(&item.source);
                }
                if let Some(source) = find(&item.children, id) {
                    return Some(source);
                }
            }
            None
        }
        let active = self.authorized_auth(handle)?;
        let source = find(&active.inspection.findings, finding)
            .cloned()
            .ok_or_else(unavailable)?;
        Ok(AuthSource {
            session_id: active.request.session_id,
            boundary: active.request.boundary,
            source,
        })
    }
    /// Release the pane and prevent older in-flight results from restoring it.
    pub fn cancel_auth(&self, slot: &str, generation: &str, page_id: &str) {
        if page_id == self.visualizer_page() {
            self.auth.cancel(slot, generation);
        }
    }
}

#[cfg(test)]
mod tests;
