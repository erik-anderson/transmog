//! Snapshot comparisons over original evidence. Presentation limits never decide equality.
#![allow(missing_docs)]

mod algorithm;
mod pairing;
mod subjects;
#[cfg(test)]
mod tests;

use crate::{AppError, Application, CapturedResource, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use transmog_core::HeaderBlock;

pub use pairing::{
    CaptureComparison, CaptureEntryChoice, ComparisonScope, PairingOptions, PairingRow,
};

pub(super) const MAX_BODY: u64 = 32 * 1024 * 1024;
const MAX_RESULTS: usize = 4;
const MAX_ROWS: usize = 512;
const PAGE_ROWS: usize = 64;
const MAX_TEXT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ComparisonSubject {
    Exchange {
        id: String,
        #[serde(default = "request_boundary")]
        request_boundary: String,
        #[serde(default = "response_boundary")]
        response_boundary: String,
    },
    Asset {
        reference: String,
    },
    Draft {
        label: String,
        status: String,
        headers: HeaderBlock,
        decoded_body: Option<Vec<u8>>,
        base: Option<Box<ComparisonSubject>>,
        body_path: Option<std::path::PathBuf>,
    },
    Preview {
        run: String,
        observation: u64,
    },
}
fn request_boundary() -> String {
    "client-request".into()
}
fn response_boundary() -> String {
    "client-response".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonOptions {
    #[serde(default)]
    pub excluded_headers: Vec<String>,
    #[serde(default)]
    pub normalize_newlines: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonRequest {
    pub operation_id: String,
    pub left: ComparisonSubject,
    pub right: ComparisonSubject,
    #[serde(default)]
    pub options: ComparisonOptions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComparisonState {
    Equal,
    Different,
    Unavailable,
    Incomplete,
    NotApplicable,
    Excluded,
    NotCompared,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonRow {
    pub index: usize,
    pub section: String,
    pub field: String,
    pub state: ComparisonState,
    pub left: String,
    pub right: String,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonSide {
    pub label: String,
    pub source: String,
    pub entry_id: Option<String>,
    pub acquired_at: u64,
    pub boundary: String,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonResult {
    pub id: String,
    pub left: ComparisonSide,
    pub right: ComparisonSide,
    pub different: bool,
    pub complete: bool,
    pub changed: usize,
    pub unknown: usize,
    pub excluded: usize,
    pub total_rows: usize,
    pub more_changes: bool,
    pub rows: Vec<ComparisonRow>,
    pub body_sections: Vec<String>,
    pub options: ComparisonOptions,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonPage {
    pub rows: Vec<ComparisonRow>,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonBytes {
    pub left: String,
    pub right: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub note: String,
}

/// Supplied by the owner adapter while the immutable scene is retained.
#[derive(Clone)]
pub struct PreviewComparisonEvidence {
    pub resource: CapturedResource,
    pub notes: Vec<String>,
    pub source_id: Option<String>,
    pub label: String,
}

#[derive(Clone, Default)]
pub(crate) struct ComparisonRegistry(Arc<Mutex<Registry>>);
#[derive(Default)]
struct Registry {
    results: VecDeque<Arc<Snapshot>>,
    operations: HashMap<String, Arc<AtomicBool>>,
    captures: VecDeque<pairing::CaptureSnapshot>,
}

struct Operation {
    registry: ComparisonRegistry,
    id: String,
    canceled: Arc<AtomicBool>,
    began: Instant,
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Operation {
    fn check(&self) -> Result<(), AppError> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(error("Comparison canceled"));
        }
        if self.began.elapsed().as_secs() >= 30 {
            return Err(error(
                "Comparison reached its 30 second work limit; narrow the scope",
            ));
        }
        Ok(())
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.registry
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operations
            .remove(&self.id);
    }
}

struct Snapshot {
    result: ComparisonResult,
    rows: Vec<ComparisonRow>,
    left: Evidence,
    right: Evidence,
    _root: tempfile::TempDir,
}
struct Evidence {
    side: ComparisonSide,
    request: Option<Message>,
    response: Option<Message>,
    outcome: Option<String>,
    timings: Vec<(String, u64)>,
    versions: Vec<(String, u64)>,
    omissions: Vec<String>,
    stream: Option<crate::StreamSummary>,
    stream_indicated: bool,
    exchange: bool,
}
struct Message {
    trailers: Option<HeaderBlock>,
    boundary: String,
    first: String,
    protocol: Option<String>,
    headers: HeaderBlock,
    body: Body,
    available: bool,
}
struct Body {
    path: Option<std::path::PathBuf>,
    length: Option<u64>,
    digest: Option<String>,
    complete: bool,
    representation: String,
    codings: Vec<String>,
    charset: Option<String>,
    reason: String,
}
impl Body {
    fn missing(reason: impl Into<String>) -> Self {
        Self {
            path: None,
            length: None,
            digest: None,
            complete: false,
            representation: "encoded entity bytes".into(),
            codings: vec![],
            charset: None,
            reason: reason.into(),
        }
    }
}

impl ComparisonRegistry {
    fn begin(&self, id: &str) -> Result<Operation, AppError> {
        if id.is_empty() || id.len() > 128 {
            return Err(error("Invalid comparison operation ID"));
        }
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.operations.len() >= 2 || state.operations.contains_key(id) {
            return Err(error(
                "Two comparisons are already running, or this operation ID is in use",
            ));
        }
        let canceled = Arc::new(AtomicBool::new(false));
        state.operations.insert(id.into(), canceled.clone());
        Ok(Operation {
            registry: self.clone(),
            id: id.into(),
            canceled,
            began: Instant::now(),
        })
    }
    fn get(&self, id: &str) -> Result<Arc<Snapshot>, AppError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .results
            .iter()
            .find(|s| s.result.id == id)
            .cloned()
            .ok_or_else(|| error("Comparison snapshot expired; refresh comparison"))
    }
}

impl Application {
    /// Compares immutable evidence without executing traffic, hooks or rules.
    /// # Errors
    /// Returns invalid subject, cancellation, acquisition or bounded-work errors.
    pub async fn compare_evidence(
        &self,
        input: ComparisonRequest,
        previews: [Option<PreviewComparisonEvidence>; 2],
    ) -> Result<ComparisonResult, AppError> {
        let app = self.clone();
        let operation = self.comparisons.begin(&input.operation_id)?;
        let _cancel_on_drop = CancelOnDrop(operation.canceled.clone());
        tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Handle::current();
            let root = tempfile::tempdir()
                .map_err(|_| error("Comparison snapshot storage unavailable"))?;
            input.options.validate()?;
            let response_only = !matches!(&input.left, ComparisonSubject::Exchange { .. })
                || !matches!(&input.right, ComparisonSubject::Exchange { .. });
            let left = runtime.block_on(subjects::acquire(
                &app,
                &input.left,
                previews[0].clone(),
                &root.path().join("left"),
                &operation,
                response_only,
            ))?;
            operation.check()?;
            let right = runtime.block_on(subjects::acquire(
                &app,
                &input.right,
                previews[1].clone(),
                &root.path().join("right"),
                &operation,
                response_only,
            ))?;
            let (rows, more_changes, tally) = runtime.block_on(algorithm::compare(
                &left,
                &right,
                &input.options,
                &operation,
            ))?;
            operation.check()?;
            let changed = tally.changed;
            let unknown = tally.unknown;
            let excluded = tally.excluded;
            let result = ComparisonResult {
                id: token()?,
                left: left.side.clone(),
                right: right.side.clone(),
                different: changed > 0,
                complete: unknown == 0 && excluded == 0 && !more_changes,
                changed,
                unknown,
                excluded,
                total_rows: rows.len(),
                more_changes,
                rows: rows.iter().take(PAGE_ROWS).cloned().collect(),
                body_sections: [
                    ("Request", left.request.as_ref(), right.request.as_ref()),
                    ("Response", left.response.as_ref(), right.response.as_ref()),
                ]
                .into_iter()
                .filter(|(_, a, b)| {
                    a.is_some_and(|m| m.body.path.is_some())
                        || b.is_some_and(|m| m.body.path.is_some())
                })
                .map(|(n, _, _)| n.into())
                .collect(),
                options: input.options,
            };
            let snapshot = Arc::new(Snapshot {
                result: result.clone(),
                rows,
                left,
                right,
                _root: root,
            });
            let mut state = app
                .comparisons
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.results.len() == MAX_RESULTS {
                state.results.pop_front();
            }
            state.results.push_back(snapshot);
            Ok(result)
        })
        .await
        .map_err(|_| error("Comparison worker failed"))?
    }

    /// Releases retained comparison bytes and metadata.
    pub fn close_comparison(&self, id: &str) {
        self.comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .results
            .retain(|s| s.result.id != id);
    }
    /// Requests cancellation without discarding an earlier completed result.
    pub fn cancel_comparison(&self, id: &str) {
        if let Some(c) = self
            .comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .operations
            .get(id)
        {
            c.store(true, Ordering::Release);
        }
    }
    /// Reads a bounded page of a frozen result.
    /// # Errors
    /// Returns an expired snapshot or invalid offset.
    pub fn comparison_page(&self, id: &str, offset: usize) -> Result<ComparisonPage, AppError> {
        let s = self.comparisons.get(id)?;
        if offset > s.rows.len() {
            return Err(error("Invalid change offset"));
        }
        Ok(ComparisonPage {
            rows: s
                .rows
                .iter()
                .skip(offset)
                .take(PAGE_ROWS)
                .cloned()
                .collect(),
            next_offset: (offset + PAGE_ROWS < s.rows.len()).then_some(offset + PAGE_ROWS),
        })
    }
    /// Reports changed/removed source metadata without refreshing the frozen result.
    /// # Errors
    /// Returns an expired comparison.
    pub fn comparison_stale(&self, id: &str) -> Result<bool, AppError> {
        let s = self.comparisons.get(id)?;
        Ok(s.left
            .versions
            .iter()
            .chain(&s.right.versions)
            .any(|(id, sequence)| {
                crate::inspector::parse_session_id(id)
                    .ok()
                    .and_then(|id| {
                        self.service
                            .catalog()
                            .sequence(transmog_core::intercept::ExchangeId(id))
                    })
                    .is_none_or(|current| current != *sequence)
            }))
    }
    /// Reads a 256-byte original range from each side of a frozen body.
    /// # Errors
    /// Returns expired or unavailable body evidence.
    pub fn comparison_bytes(
        &self,
        id: &str,
        section: &str,
        offset: u64,
    ) -> Result<ComparisonBytes, AppError> {
        let s = self.comparisons.get(id)?;
        let bodies = match section {
            "Request" => [s.left.request.as_ref(), s.right.request.as_ref()],
            "Response" => [s.left.response.as_ref(), s.right.response.as_ref()],
            _ => return Err(error("Invalid body section")),
        };
        let mut values = Vec::new();
        let mut length = 0;
        for message in bodies {
            let Some(body) = message.map(|m| &m.body) else {
                values.push("Not applicable".into());
                continue;
            };
            length = length.max(body.length.unwrap_or(0));
            values.push(if let Some(path) = &body.path {
                let mut file =
                    std::fs::File::open(path).map_err(|_| error("Frozen bytes unavailable"))?;
                file.seek(SeekFrom::Start(offset))
                    .map_err(|_| error("Invalid byte offset"))?;
                let mut bytes = Vec::new();
                file.take(256)
                    .read_to_end(&mut bytes)
                    .map_err(|_| error("Frozen bytes unreadable"))?;
                crate::inspector::render_body(
                    crate::BodyRepresentation::Bytes,
                    &bytes,
                    None,
                    None,
                    offset,
                )
                .1
            } else {
                body.reason.clone()
            });
        }
        Ok(ComparisonBytes{left:values.remove(0),right:values.remove(0),offset,next_offset:offset.checked_add(256).filter(|next|*next<length),note:"Original frozen representation; content coding is shown in the body summary. Text-diff limits do not change these bytes.".into()})
    }
}

impl ComparisonOptions {
    fn validate(&self) -> Result<(), AppError> {
        if self.excluded_headers.len() > 64
            || self
                .excluded_headers
                .iter()
                .any(|s| s.is_empty() || s.len() > 256 || !s.is_ascii())
        {
            return Err(error("Choose at most 64 explicit ASCII header names"));
        }
        Ok(())
    }
}
fn token() -> Result<String, AppError> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| error("Snapshot identity unavailable"))?;
    Ok(hex_text(&b))
}
fn error(s: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, s, false)
}
fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
fn short(s: &str) -> String {
    let mut value = s.chars().take(1024).collect::<String>();
    if s.chars().count() > 1024 {
        value.push_str("… [display shortened; comparison used full evidence]");
    }
    value
}

fn hex_text(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            write!(&mut text, "{byte:02x}").expect("writing into String");
            text
        })
}
