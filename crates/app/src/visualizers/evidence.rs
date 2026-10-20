//! Exact retained-message projection and bounded instance-owned source reads.
use super::{VisualizerSourceInfo, invalid, unavailable};
use crate::{AppError, BodyAvailability, BodyStore, ErrorCategory, StoredBodyMetadata, inspector};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use transmog_core::{HeaderBlock, intercept::ExchangeId, observe::ExchangeBoundary};
use transmog_extension_registry::InstanceResources;
use transmog_extensions::{
    BodyAccess, BodyReference, BytePage, Direction, Header, InputAccess, MessageInput, ParseStatus,
    ReadRequest, UpdatePolicy, VisualizationDocument, VisualizationInput, Visualizer, decimal,
    header_source_id, sha256,
};
use transmog_session::SessionSnapshot;
pub(super) enum Source {
    Header(Arc<[u8]>),
    Related {
        source: Box<Source>,
        guard: Arc<super::context::RelatedGuard>,
        exchange: ExchangeId,
        order: u64,
    },
    Body {
        store: BodyStore,
        exchange: ExchangeId,
        boundary: ExchangeBoundary,
        metadata: Box<StoredBodyMetadata>,
        decoding: Option<Result<Vec<String>, transmog_extensions::DecodeFailure>>,
    },
    Decoded {
        bytes: Arc<[u8]>,
        original: Arc<Source>,
    },
}
impl Source {
    pub(super) fn length(&self) -> u64 {
        match self {
            Self::Header(bytes) | Self::Decoded { bytes, .. } => bytes.len() as u64,
            Self::Related { source, .. } => source.length(),
            Self::Body { metadata, .. } => metadata.retained_bytes,
        }
    }
    pub(super) fn authorize(&self, canceled: &AtomicBool) -> Result<(), AppError> {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        match self {
            Self::Header(_) => Ok(()),
            Self::Related {
                source,
                guard,
                exchange,
                order,
            } => {
                if !guard.allowed(*exchange, *order) {
                    return Err(unavailable());
                }
                source.authorize(canceled)
            }
            Self::Decoded { original, .. } => original.authorize(canceled),
            Self::Body {
                store,
                exchange,
                metadata,
                ..
            } => {
                let current = store
                    .metadata(*exchange)
                    .into_iter()
                    .find(|m| m.boundary == metadata.boundary)
                    .ok_or_else(unavailable)?;
                if current.retained_bytes < metadata.retained_bytes
                    || metadata
                        .sha256
                        .as_ref()
                        .is_some_and(|hash| current.sha256.as_ref() != Some(hash))
                {
                    return Err(unavailable());
                }
                Ok(())
            }
        }
    }
    pub(super) fn read(
        &self,
        offset: u64,
        count: usize,
        canceled: &AtomicBool,
    ) -> Result<Vec<u8>, AppError> {
        if count == 0
            || count > 64 * 1024
            || offset
                .checked_add(count as u64)
                .is_none_or(|end| end > self.length())
        {
            return Err(invalid("Visualizer source range is invalid"));
        }
        self.authorize(canceled)?;
        match self {
            Self::Related {
                source,
                guard,
                exchange,
                order,
            } => {
                if !guard.allowed(*exchange, *order) {
                    return Err(unavailable());
                }
                let bytes = source.read(offset, count, canceled)?;
                if !guard.allowed(*exchange, *order) {
                    return Err(unavailable());
                }
                Ok(bytes)
            }
            Self::Header(bytes) => {
                let start =
                    usize::try_from(offset).map_err(|_| invalid("Invalid header source offset"))?;
                Ok(bytes[start..start + count].to_vec())
            }
            Self::Decoded { bytes, .. } => {
                let start = usize::try_from(offset)
                    .map_err(|_| invalid("Invalid decoded source offset"))?;
                let result = bytes[start..start + count].to_vec();
                self.authorize(canceled)?;
                Ok(result)
            }
            Self::Body {
                store,
                exchange,
                boundary,
                ..
            } => {
                // The store holds its eviction lease only during this bounded read.
                // A visualizer cannot pin an arbitrarily large capture body while idle.
                let mut bytes = Vec::with_capacity(count);
                while bytes.len() < count {
                    if canceled.load(Ordering::Acquire) {
                        return Err(unavailable());
                    }
                    let requested = (count - bytes.len()).min(store.range_read_limit());
                    let range = store
                        .read_paged_range(
                            *exchange,
                            *boundary,
                            offset + bytes.len() as u64,
                            requested,
                        )
                        .map_err(|_| unavailable())?;
                    if requested == 0 || range.bytes.len() != requested {
                        return Err(unavailable());
                    }
                    bytes.extend_from_slice(&range.bytes);
                }
                Ok(bytes)
            }
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        match self {
            Self::Header(bytes) | Self::Decoded { bytes, .. } => bytes.len(),
            Self::Related { source, .. } => source.retained_bytes(),
            Self::Body { .. } => 0,
        }
    }
}
pub(super) struct Resources {
    pub(super) live_available: bool,
    pub(super) finalized: AtomicBool,
    pub(super) live: Mutex<Option<Arc<super::live::Feed>>>,
    pub(super) stopped: AtomicBool,
    pub(super) sources: Mutex<BTreeMap<String, Arc<Source>>>,
    pub(super) info: Mutex<Vec<VisualizerSourceInfo>>,
    pub(super) context: Mutex<Option<super::context::Broker>>,
}
impl InstanceResources for Resources {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(feed) = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            feed.stop();
        }
        self.sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.info
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}
impl Resources {
    pub(super) fn finalize_updates(&self) {
        self.finalized.store(true, Ordering::Release);
        if let Some(feed) = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            feed.stop();
        }
    }

    pub(super) fn finish_document(
        &self,
        document: &mut VisualizationDocument,
    ) -> super::VisualizerLiveStatus {
        if document.updates == Some(UpdatePolicy::Final) && !self.finalized.load(Ordering::Acquire)
        {
            self.finalize_updates();
        }
        // A stopped feed retains its lost-hint count until the instance is
        // unloaded. Expansion must not turn incomplete live evidence complete.
        let status = self.live_status();
        if document.status == ParseStatus::Complete && status.missed_hints != "0" {
            document.status = ParseStatus::Partial;
        }
        status
    }

    pub(super) fn live_status(&self) -> super::VisualizerLiveStatus {
        let live = self
            .live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::VisualizerLiveStatus {
            available: self.live_available && !self.finalized.load(Ordering::Acquire),
            enabled: live.as_ref().is_some_and(|feed| feed.enabled()),
            missed_hints: live.as_ref().map_or(0, |feed| feed.missed()).to_string(),
        }
    }
    pub(super) fn append(
        &self,
        added: BTreeMap<String, Source>,
        info: Vec<VisualizerSourceInfo>,
        budget: usize,
    ) -> Result<(), AppError> {
        let mut sources = self.sources.lock().map_err(|_| unavailable())?;
        let mut infos = self.info.lock().map_err(|_| unavailable())?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let new = added.iter().filter(|(id, _)| !sources.contains_key(*id));
        let count = new.clone().count();
        let bytes = sources
            .values()
            .map(|source| source.retained_bytes())
            .chain(new.map(|(_, source)| source.retained_bytes()))
            .fold(0usize, usize::saturating_add);
        if sources.len().saturating_add(count) > 4096 || bytes > budget {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Related source input exceeds the visualizer budget",
                false,
            ));
        }
        for entry in info {
            if !sources.contains_key(&entry.id) {
                infos.push(entry);
            }
        }
        sources.extend(added.into_iter().map(|(id, source)| (id, Arc::new(source))));
        Ok(())
    }
    pub(super) fn context(
        &self,
        request: &transmog_extensions::ContextRequest,
    ) -> Result<transmog_extensions::ContextPage, AppError> {
        self.context
            .lock()
            .map_err(|_| unavailable())?
            .as_mut()
            .ok_or_else(unavailable)?
            .page(request, self)
    }
    pub(super) fn info(&self) -> Result<Vec<VisualizerSourceInfo>, AppError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let info = self.info.lock().map_err(|_| unavailable())?.clone();
        if self.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        Ok(info)
    }
    pub(super) fn read(&self, request: &ReadRequest) -> Result<(u64, BytePage), AppError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let source = self
            .sources
            .lock()
            .map_err(|_| unavailable())?
            .get(&request.source_id)
            .cloned()
            .ok_or_else(unavailable)?;
        let offset = decimal(&request.offset).map_err(|_| invalid("Invalid source offset"))?;
        let bytes = source.read(offset, request.max_bytes, &self.stopped)?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        Ok((
            source.length(),
            BytePage {
                source_id: request.source_id.clone(),
                offset: request.offset.clone(),
                bytes,
            },
        ))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Boundaries {
    pub(super) request: ExchangeBoundary,
    pub(super) response: ExchangeBoundary,
    pub(super) direction: Direction,
}
pub(super) fn boundaries(selected: &str) -> Result<Boundaries, AppError> {
    use ExchangeBoundary::{ClientRequest, ClientResponse, UpstreamRequest, UpstreamResponse};
    let selected = inspector::parse_boundary(selected)?;
    Ok(match selected {
        ClientRequest | ClientResponse => Boundaries {
            request: ClientRequest,
            response: ClientResponse,
            direction: if selected == ClientRequest {
                Direction::Request
            } else {
                Direction::Response
            },
        },
        UpstreamRequest | UpstreamResponse => Boundaries {
            request: UpstreamRequest,
            response: UpstreamResponse,
            direction: if selected == UpstreamRequest {
                Direction::Request
            } else {
                Direction::Response
            },
        },
    })
}
#[allow(clippy::too_many_lines)] // Keep exact boundary, header and body grants in one projection policy.
pub(super) fn project_input(
    snapshot: &SessionSnapshot,
    selected: Boundaries,
    declaration: &Visualizer,
    store: Option<&BodyStore>,
    instance: &str,
    sources: &mut BTreeMap<String, Source>,
) -> Result<VisualizationInput, AppError> {
    let mut remaining = declaration.limits.input_bytes;
    let mut make = |side,
                    boundary,
                    headers: &HeaderBlock,
                    method,
                    path,
                    status|
     -> Result<MessageInput, AppError> {
        let evidence = sha256(
            format!(
                "{instance}:{:?}:{boundary:?}:{}",
                snapshot.exchange_id, snapshot.last_sequence
            )
            .as_bytes(),
        );
        let decoding = super::decoding::grant(headers, declaration.inputs.side(side));
        let mut headers = project_headers(headers, declaration.inputs.side(side), &mut remaining)?;
        for header in &mut headers {
            if let Some(value) = &header.value {
                let id = header_source_id(side, &evidence, header.index);
                header.source_id = Some(id.clone());
                if instance != "discovery" {
                    sources.insert(id, Source::Header(Arc::from(value.as_slice())));
                }
            }
        }
        let mut message = MessageInput {
            evidence_id: evidence.clone(),
            headers,
            body: None,
            method,
            path,
            status,
        };
        if let Some(store) = store {
            let access = declaration.inputs.side(side).body;
            if access != BodyAccess::None {
                if let Some(metadata) = store
                    .metadata(snapshot.exchange_id)
                    .into_iter()
                    .find(|m| m.boundary == boundary_name(boundary))
                {
                    if access == BodyAccess::Required
                        && matches!(
                            metadata.availability,
                            BodyAvailability::Disabled
                                | BodyAvailability::Evicted
                                | BodyAvailability::QuotaOmitted
                        )
                    {
                        return Err(missing_body(boundary));
                    }
                    let id = format!(
                        "body-{}",
                        &sha256(
                            format!(
                                "{evidence}:{}:{:?}:{}",
                                metadata.retained_bytes,
                                metadata.availability,
                                metadata.sha256.as_deref().unwrap_or("")
                            )
                            .as_bytes()
                        )[..48]
                    );
                    let availability = serde_json::to_value(metadata.availability)
                        .map_err(|_| unavailable())?
                        .as_str()
                        .unwrap_or("unavailable")
                        .to_owned();
                    message.body = Some(BodyReference {
                        source_id: id.clone(),
                        length: metadata.retained_bytes.to_string(),
                        availability,
                    });
                    sources.insert(
                        id,
                        Source::Body {
                            store: store.clone(),
                            exchange: snapshot.exchange_id,
                            boundary,
                            metadata: Box::new(metadata),
                            decoding,
                        },
                    );
                } else if access == BodyAccess::Required {
                    return Err(missing_body(boundary));
                }
            }
        } else if instance != "discovery"
            && declaration.inputs.side(side).body == BodyAccess::Required
        {
            return Err(missing_body(boundary));
        }
        Ok(message)
    };
    let request = snapshot
        .request_heads
        .iter()
        .find(|h| h.boundary == selected.request)
        .map(|h| {
            make(
                Direction::Request,
                selected.request,
                &h.head.headers,
                Some(h.head.method.clone()),
                Some(h.head.target.path.clone()),
                None,
            )
        })
        .transpose()?;
    let response = snapshot
        .response_heads
        .iter()
        .rev()
        .find(|h| h.boundary == selected.response && (h.head.status >= 200 || h.head.status == 101))
        .map(|h| {
            make(
                Direction::Response,
                selected.response,
                &h.head.headers,
                None,
                None,
                Some(h.head.status),
            )
        })
        .transpose()?;
    if instance != "discovery"
        && ((request.is_none() && declaration.inputs.request.body == BodyAccess::Required)
            || (response.is_none() && declaration.inputs.response.body == BodyAccess::Required))
    {
        return Err(missing_body(if request.is_none() {
            selected.request
        } else {
            selected.response
        }));
    }
    Ok(VisualizationInput {
        instance_id: instance.into(),
        exchange_id: format!("{:032x}", snapshot.exchange_id.0),
        request_number: snapshot.request_number.to_string(),
        generation: snapshot.last_sequence.to_string(),
        direction: selected.direction,
        request,
        response,
    })
}
pub(super) fn source_infos(
    snapshot: &SessionSnapshot,
    selected: Boundaries,
    input: &VisualizationInput,
) -> Vec<VisualizerSourceInfo> {
    let session_id = format!("{:032x}", snapshot.exchange_id.0);
    let mut info = Vec::new();
    for (boundary, message) in [
        (selected.request, &input.request),
        (selected.response, &input.response),
    ] {
        let Some(message) = message else {
            continue;
        };
        let label = match boundary {
            ExchangeBoundary::ClientRequest => "client request",
            ExchangeBoundary::UpstreamRequest => "request sent to origin",
            ExchangeBoundary::UpstreamResponse => "origin response",
            ExchangeBoundary::ClientResponse => "response sent to client",
        };
        let prefix = format!("Request #{} · {label}", snapshot.request_number);
        for header in &message.headers {
            if let Some(id) = &header.source_id
                && let Some(bytes) = &header.value
            {
                info.push(VisualizerSourceInfo {
                    id: id.clone(),
                    name: format!(
                        "{prefix} · {} (header {})",
                        header.name,
                        u64::from(header.index) + 1
                    ),
                    length: bytes.len().to_string(),
                    availability: "complete".into(),
                    session_id: session_id.clone(),
                    boundary: boundary_name(boundary).into(),
                    http_decoded: None,
                });
            }
        }
        if let Some(body) = &message.body {
            info.push(VisualizerSourceInfo {
                id: body.source_id.clone(),
                name: format!("{prefix} · body"),
                length: body.length.clone(),
                availability: body.availability.clone(),
                session_id: session_id.clone(),
                boundary: boundary_name(boundary).into(),
                http_decoded: None,
            });
        }
    }
    info
}
fn project_headers(
    block: &HeaderBlock,
    access: &InputAccess,
    remaining: &mut usize,
) -> Result<Vec<Header>, AppError> {
    let mut projected = Vec::new();
    let mut header_bytes = 0_usize;
    for (index, field) in block
        .iter()
        .enumerate()
        .filter(|(_, f)| access.headers.iter().any(|name| f.name_eq(name)))
    {
        let bytes = field.name().len().saturating_add(if field.is_redacted() {
            0
        } else {
            field.value().len()
        });
        header_bytes = header_bytes.saturating_add(bytes);
        if projected.len() == 512 || header_bytes > 256 * 1024 {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "Declared header input is too large",
                false,
            ));
        }
        *remaining = remaining.checked_sub(bytes).ok_or_else(|| {
            AppError::new(
                ErrorCategory::Limit,
                "Declared header input exceeds the visualizer budget",
                false,
            )
        })?;
        projected.push(Header {
            source_id: None,
            index: u32::try_from(index).map_err(|_| invalid("Too many retained headers"))?,
            name: String::from_utf8_lossy(field.name()).into_owned(),
            value: (!field.is_redacted()).then(|| field.value().to_vec()),
            redacted: field.is_redacted(),
            original_bytes: field.original_value_bytes().map(|n| n.to_string()),
        });
    }
    Ok(projected)
}
pub(super) fn boundary_name(boundary: ExchangeBoundary) -> &'static str {
    match boundary {
        ExchangeBoundary::ClientRequest => "client-request",
        ExchangeBoundary::UpstreamRequest => "upstream-request",
        ExchangeBoundary::UpstreamResponse => "upstream-response",
        ExchangeBoundary::ClientResponse => "client-response",
    }
}
fn missing_body(boundary: ExchangeBoundary) -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        format!(
            "This visualizer requires a retained body for {}",
            boundary_name(boundary).replace('-', " ")
        ),
        false,
    )
}
