//! On-demand retained context. Extensions never choose an origin, capture or exchange ID.
use super::{
    AppError, Application,
    evidence::{Boundaries, Resources, Source, project_input, source_infos},
    invalid, unavailable,
};
use crate::{BodyStore, traces::TraceRegistry};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{Arc, atomic::Ordering},
};
use transmog_core::{ClientIdentity, HeaderBlock, Target, intercept::ExchangeId};
use transmog_extensions::{
    BodyAccess, ContextExchange, ContextPage, ContextPosition, ContextRequest, ContextStatus,
    Direction, HeaderPredicate, HeaderTest, Visualizer, matches,
};
use transmog_session::{SessionCatalog, SessionHeads, SessionSnapshot};

const SCAN_PAGE: usize = 4096;
// Decimal JSON byte arrays amplify header bytes. Keep context IPC comfortably within the frame ceiling.
const PAGE_HEADER_BYTES: usize = 512 * 1024;
#[cfg(test)]
pub(super) mod tests;

/// Origins are native constraints, independent of extension selectors and correlation fields.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}
impl Origin {
    fn new(target: &Target) -> Option<Self> {
        let scheme = target.scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") || target.port == 0 {
            return None;
        }
        // Target stores an explicit effective port. Never infer an origin from a Host header.
        let host = if let Ok(address) = target.host.parse::<IpAddr>() {
            address.to_string()
        } else {
            url::Host::parse(&target.host)
                .ok()?
                .to_string()
                .to_ascii_lowercase()
        };
        Some(Self {
            scheme,
            host,
            port: target.port,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Scope {
    capture: String,
    original_origin: Origin,
    observed_origin: Origin,
    conversation: [u8; 32],
}
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn correlation<'a>(block: &'a HeaderBlock, predicate: &HeaderPredicate) -> Option<&'a [u8]> {
    let mut occurrences = block
        .iter()
        .filter(|header| header.name_eq(&predicate.name));
    let header = occurrences.next()?;
    if occurrences.next().is_some()
        || header.is_redacted()
        || header.value().is_empty()
        || header.value().len() > 1024
    {
        return None;
    }
    let matches = match &predicate.test {
        HeaderTest::Present => true,
        HeaderTest::Equals { value } => header.value() == value.as_bytes(),
        HeaderTest::NotEquals { value } => header.value() != value.as_bytes(),
    };
    matches.then_some(header.value())
}
fn scope(
    heads: &SessionHeads<'_>,
    selected: Boundaries,
    declaration: &Visualizer,
    capture: String,
    expected: Option<&Scope>,
) -> Option<Scope> {
    if expected.is_some_and(|expected| expected.capture != capture) {
        return None;
    }
    let context = declaration.context.as_ref();
    let request = &heads
        .request_heads
        .iter()
        .find(|head| head.boundary == selected.request)?
        .head;
    let response = heads.response_heads.iter().rev().find(|head| {
        head.boundary == selected.response && (head.head.status >= 200 || head.head.status == 101)
    });
    let original_origin = Origin::new(heads.metadata.original_target.as_target())?;
    let observed_origin = Origin::new(&request.target)?;
    if expected.is_some_and(|expected| {
        expected.original_origin != original_origin || expected.observed_origin != observed_origin
    }) {
        return None;
    }
    let mut hash = Sha256::new();
    field(&mut hash, request.target.path.as_bytes());
    // The query participates in correlation but is never projected into script input.
    field(&mut hash, &[u8::from(request.target.query.is_some())]);
    field(
        &mut hash,
        request.target.query.as_deref().unwrap_or("").as_bytes(),
    );
    let original = heads.metadata.original_target.as_target();
    field(&mut hash, original.path.as_bytes());
    field(&mut hash, &[u8::from(original.query.is_some())]);
    field(
        &mut hash,
        original.query.as_deref().unwrap_or("").as_bytes(),
    );
    field(
        &mut hash,
        heads.metadata.client_addr.ip().to_string().as_bytes(),
    );
    match &heads.metadata.client_identity {
        ClientIdentity::LocalProcess { pid, name } if *pid != 0 => {
            field(&mut hash, b"process");
            field(&mut hash, &pid.to_le_bytes());
            field(&mut hash, &[u8::from(name.is_some())]);
            field(&mut hash, name.as_deref().unwrap_or("").as_bytes());
            if context.is_none() {
                field(
                    &mut hash,
                    &heads.metadata.downstream_connection_id.0.to_le_bytes(),
                );
            }
        }
        identity => {
            field(
                &mut hash,
                if matches!(identity, ClientIdentity::Remote) {
                    b"remote"
                } else {
                    b"unknown"
                },
            );
            // Missing process attribution must not merge independent client connections.
            field(
                &mut hash,
                &heads.metadata.downstream_connection_id.0.to_le_bytes(),
            );
        }
    }
    for predicate in context.into_iter().flat_map(|context| &context.headers) {
        if !declaration
            .inputs
            .side(predicate.direction)
            .permits_header(&predicate.name)
        {
            return None;
        }
        let block: &HeaderBlock = match predicate.direction {
            Direction::Request => &request.headers,
            Direction::Response => &response?.head.headers,
        };
        field(&mut hash, correlation(block, predicate)?);
    }
    Some(Scope {
        capture,
        original_origin,
        observed_origin,
        conversation: hash.finalize().into(),
    })
}

/// Retained native constraints shared by granted sources, without retaining an Application/Registry.
pub(super) struct RelatedGuard {
    catalog: SessionCatalog,
    traces: TraceRegistry,
    live_capture: String,
    anchor: ExchangeId,
    anchor_order: u64,
    selected: Boundaries,
    declaration: Visualizer,
    scope: Scope,
}
impl RelatedGuard {
    pub(super) fn new(
        app: &Application,
        snapshot: &SessionSnapshot,
        selected: Boundaries,
        declaration: &Visualizer,
    ) -> Option<Arc<Self>> {
        let source = app
            .traces
            .context_source_id(&format!("{:032x}", snapshot.exchange_id.0));
        let live_capture = format!("live:{}", app.visualizers.owner);
        let capture = if snapshot.imported {
            source.map(|source| format!("saved:{source}"))
        } else if source.is_none() {
            Some(live_capture.clone())
        } else {
            None
        };
        let scope = scope(&snapshot.into(), selected, declaration, capture?, None)?;
        // Composer evidence has no retained catalog context and cannot enumerate live traffic.
        app.service
            .catalog()
            .with_heads(snapshot.exchange_id, |_| ())?;
        Some(Arc::new(Self {
            catalog: app.service.catalog().clone(),
            traces: app.traces.clone(),
            live_capture,
            anchor: snapshot.exchange_id,
            anchor_order: snapshot.request_number,
            selected,
            declaration: declaration.clone(),
            scope,
        }))
    }
    pub(super) fn index_scope(&self) -> transmog_extensions::Scope {
        index_key(
            &self.scope.capture,
            &self.scope.original_origin,
            &self.scope.observed_origin,
            self.selected,
        )
    }
    fn capture(&self, imported: bool, source: Option<String>) -> Option<String> {
        if imported {
            source.map(|source| format!("saved:{source}"))
        } else if source.is_none() {
            Some(self.live_capture.clone())
        } else {
            None
        }
    }
    pub(super) fn matches(&self, id: ExchangeId) -> bool {
        self.matches_ordered(id, (id == self.anchor).then_some(self.anchor_order))
    }
    fn matches_ordered(&self, id: ExchangeId, order: Option<u64>) -> bool {
        // Resolve trace provenance before taking the catalog lock (import publication lock order).
        let source = self.traces.context_source_id(&format!("{:032x}", id.0));
        self.catalog
            .with_heads(id, |heads| {
                if order.is_some_and(|order| heads.request_number != order) {
                    return false;
                }
                self.capture(heads.imported, source)
                    .and_then(|capture| {
                        scope(
                            &heads,
                            self.selected,
                            &self.declaration,
                            capture,
                            Some(&self.scope),
                        )
                    })
                    .as_ref()
                    == Some(&self.scope)
            })
            .unwrap_or(false)
    }
    pub(super) fn allowed(&self, id: ExchangeId, order: u64) -> bool {
        self.matches(self.anchor) && self.matches_ordered(id, Some(order))
    }
    pub(super) fn snapshot_matches(&self, snapshot: &SessionSnapshot) -> bool {
        let source = self
            .traces
            .context_source_id(&format!("{:032x}", snapshot.exchange_id.0));
        self.capture(snapshot.imported, source)
            .and_then(|capture| {
                scope(
                    &snapshot.into(),
                    self.selected,
                    &self.declaration,
                    capture,
                    Some(&self.scope),
                )
            })
            .as_ref()
            == Some(&self.scope)
    }
    pub(super) fn projected_bytes(&self, id: ExchangeId) -> Option<usize> {
        self.catalog.with_heads(id, |heads| {
            let admission = super::admission::Admission::from_heads(
                heads.request_heads,
                heads.response_heads,
                self.selected,
            );
            let metadata = admission.metadata();
            let mut bytes = 0usize;
            for (side, message) in [
                (Direction::Request, metadata.request),
                (Direction::Response, metadata.response),
            ] {
                if let Some(message) = message {
                    for header in message.headers {
                        if self
                            .declaration
                            .inputs
                            .side(side)
                            .headers
                            .iter()
                            .any(|name| header.0.name_eq(name))
                        {
                            bytes = bytes.saturating_add(header.0.name().len());
                            if !header.0.is_redacted() {
                                bytes = bytes.saturating_add(header.0.value().len());
                            }
                        }
                    }
                }
            }
            bytes
        })
    }
    fn admitted(&self, id: ExchangeId) -> bool {
        if !self.matches(id) {
            return false;
        }
        let rules = if self.declaration.input_matches.is_empty() {
            &self.declaration.matches
        } else {
            &self.declaration.input_matches
        };
        self.catalog
            .with_heads(id, |heads| {
                let admission = super::admission::Admission::from_heads(
                    heads.request_heads,
                    heads.response_heads,
                    self.selected,
                );
                rules
                    .iter()
                    .any(|rule| matches(rule, admission.metadata(), rule.direction))
            })
            .unwrap_or(false)
    }
}

// This coarse index is only a candidate filter. The full guard is mandatory before enqueue and reads.
fn index_key(
    capture: &str,
    original: &Origin,
    observed: &Origin,
    selected: Boundaries,
) -> transmog_extensions::Scope {
    let mut hash = Sha256::new();
    for origin in [original, observed] {
        field(&mut hash, origin.scheme.as_bytes());
        field(&mut hash, origin.host.as_bytes());
        field(&mut hash, &origin.port.to_le_bytes());
    }
    field(&mut hash, format!("{:?}", selected.request).as_bytes());
    transmog_extensions::Scope {
        capture: capture.into(),
        conversation: format!("{:x}", hash.finalize()),
    }
}
pub(super) fn live_index_scope(
    heads: &SessionHeads<'_>,
    selected: Boundaries,
    owner: &str,
) -> Option<transmog_extensions::Scope> {
    if heads.imported {
        return None;
    }
    let request = heads
        .request_heads
        .iter()
        .find(|head| head.boundary == selected.request)?;
    Some(index_key(
        &format!("live:{owner}"),
        &Origin::new(heads.metadata.original_target.as_target())?,
        &Origin::new(&request.head.target)?,
        selected,
    ))
}
struct Cursor {
    position: ContextPosition,
    boundary: u64,
}
pub(super) struct Broker {
    guard: Option<Arc<RelatedGuard>>,
    anchor_order: u64,
    instance: String,
    declaration: Visualizer,
    store: Option<BodyStore>,
    cursors: BTreeMap<String, Cursor>,
    serial: u64,
    delivered: usize,
}
impl Broker {
    pub(super) fn new(
        app: &Application,
        snapshot: &SessionSnapshot,
        selected: Boundaries,
        declaration: &Visualizer,
        instance: &str,
        store: Option<BodyStore>,
    ) -> Self {
        let guard = RelatedGuard::new(app, snapshot, selected, declaration);
        Self {
            guard,
            anchor_order: snapshot.request_number,
            instance: instance.into(),
            declaration: declaration.clone(),
            store,
            cursors: BTreeMap::new(),
            serial: 0,
            delivered: 0,
        }
    }
    fn empty(&self, status: ContextStatus) -> ContextPage {
        ContextPage {
            anchor_order: self.anchor_order.to_string(),
            exchanges: Vec::new(),
            next_cursor: None,
            status,
        }
    }
    fn project(
        &self,
        snapshot: &SessionSnapshot,
        guard: &Arc<RelatedGuard>,
        resources: &Resources,
        remaining: &mut usize,
    ) -> Result<ContextExchange, AppError> {
        guard.project(
            snapshot,
            resources,
            &self.instance,
            self.store.as_ref(),
            remaining,
        )
    }
    fn boundary(&mut self, request: &ContextRequest) -> Result<u64, AppError> {
        let Some(cursor) = &request.cursor else {
            return Ok(self.anchor_order);
        };
        let cursor = self
            .cursors
            .remove(cursor)
            .ok_or_else(|| invalid("The related traffic cursor is unavailable"))?;
        if cursor.position != request.position {
            return Err(invalid(
                "The related traffic cursor has a different direction",
            ));
        }
        Ok(cursor.boundary)
    }
    pub(super) fn page(
        &mut self,
        request: &ContextRequest,
        resources: &Resources,
    ) -> Result<ContextPage, AppError> {
        let context = self
            .declaration
            .context
            .as_ref()
            .ok_or_else(|| invalid("Related traffic access was not declared"))?;
        request
            .validate(context)
            .map_err(|_| invalid("Invalid related traffic request"))?;
        let maximum = context.max_exchanges;
        if resources.stopped.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        let boundary = self.boundary(request)?;
        let Some(guard) = self.guard.as_ref() else {
            return Ok(self.empty(ContextStatus::Unavailable));
        };
        if !guard.matches(guard.anchor) {
            return Ok(self.empty(ContextStatus::Unavailable));
        }
        if self.delivered >= maximum || self.serial >= 512 {
            return Ok(self.empty(ContextStatus::Limited));
        }
        let before = request.position == ContextPosition::Before;
        let ids = guard.catalog.ordered_ids(
            (!before).then_some(boundary),
            before.then_some(boundary),
            before,
            SCAN_PAGE,
        );
        let mut page = self.empty(ContextStatus::Complete);
        let mut remaining = self.declaration.limits.input_bytes.min(PAGE_HEADER_BYTES);
        let mut last = boundary;
        let mut consumed = 0;
        let count = request.limit.min(maximum - self.delivered);
        for (order, id) in &ids {
            if resources.stopped.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            if guard.admitted(*id) {
                let Some(bytes) = guard.projected_bytes(*id) else {
                    continue;
                };
                if bytes > remaining {
                    if page.exchanges.is_empty() {
                        page.status = ContextStatus::Limited;
                    }
                    break;
                }
            } else {
                last = *order;
                consumed += 1;
                continue;
            }
            last = *order;
            consumed += 1;
            let Some(snapshot) = guard.catalog.get(*id) else {
                continue;
            };
            if snapshot.request_number != *order || !guard.snapshot_matches(&snapshot) {
                continue;
            }
            let exchange = self.project(&snapshot, guard, resources, &mut remaining)?;
            self.delivered += 1;
            page.exchanges.push(exchange);
            if page.exchanges.len() == count {
                break;
            }
        }
        if before {
            page.exchanges.reverse();
        }
        if page.status == ContextStatus::Limited || self.delivered >= maximum {
            page.status = ContextStatus::Limited;
        } else if consumed < ids.len() || ids.len() == SCAN_PAGE {
            self.serial += 1;
            let cursor = transmog_extensions::sha256(
                format!("{}:{}", self.instance, self.serial).as_bytes(),
            );
            // One-shot cursors are private to this worker and cannot select another scope or direction.
            self.cursors.clear();
            self.cursors.insert(
                cursor.clone(),
                Cursor {
                    position: request.position,
                    boundary: last,
                },
            );
            page.next_cursor = Some(cursor);
            page.status = ContextStatus::More;
        }
        if resources.stopped.load(Ordering::Acquire) || !guard.matches(guard.anchor) {
            return Err(unavailable());
        }
        Ok(page)
    }
}

impl RelatedGuard {
    pub(super) fn project(
        self: &Arc<Self>,
        snapshot: &SessionSnapshot,
        resources: &Resources,
        instance: &str,
        store: Option<&BodyStore>,
        remaining: &mut usize,
    ) -> Result<ContextExchange, AppError> {
        let mut declaration = self.declaration.clone();
        // Related evidence can be incomplete. Preserve the grant but report absent bodies honestly.
        for access in [
            &mut declaration.inputs.request,
            &mut declaration.inputs.response,
        ] {
            if access.body == BodyAccess::Required {
                access.body = BodyAccess::Optional;
            }
        }
        declaration.limits.input_bytes = *remaining;
        let mut sources = BTreeMap::new();
        let input = project_input(
            snapshot,
            self.selected,
            &declaration,
            store,
            instance,
            &mut sources,
        )?;
        let bytes = [&input.request, &input.response]
            .into_iter()
            .flatten()
            .flat_map(|message| &message.headers)
            .map(|header| header.name.len() + header.value.as_ref().map_or(0, Vec::len))
            .sum::<usize>();
        *remaining = remaining.checked_sub(bytes).ok_or_else(unavailable)?;
        let infos = source_infos(snapshot, self.selected, &input);
        let sources = sources
            .into_iter()
            .map(|(key, source)| {
                (
                    key,
                    Source::Related {
                        source: Box::new(source),
                        guard: Arc::clone(self),
                        exchange: snapshot.exchange_id,
                        order: snapshot.request_number,
                    },
                )
            })
            .collect();
        resources.append(sources, infos, self.declaration.limits.input_bytes)?;
        Ok(ContextExchange {
            order: snapshot.request_number.to_string(),
            input,
        })
    }
}
