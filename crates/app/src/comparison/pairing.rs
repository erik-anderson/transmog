use super::{
    AppError, Application, Deserialize, HashMap, HeaderBlock, Operation, Serialize, error, token,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ComparisonScope {
    Selected {
        ids: Vec<String>,
    },
    Filtered {
        query: Box<crate::SessionQueryInput>,
    },
    Source {
        trace_id: Option<String>,
    },
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingOptions {
    #[serde(default)]
    pub excluded_query_names: Vec<String>,
    #[serde(default)]
    pub request_headers: Vec<String>,
    #[serde(default)]
    pub request_body_digest: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRow {
    pub index: usize,
    pub left: Option<String>,
    pub right: Option<String>,
    pub left_label: String,
    pub right_label: String,
    pub reason: String,
    pub state: String,
    pub group: String,
    pub timing_delta: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureComparison {
    pub id: String,
    pub left_count: usize,
    pub left_details: String,
    pub right_details: String,
    pub left_label: String,
    pub right_label: String,
    pub right_count: usize,
    pub changed: usize,
    pub equal_metadata: usize,
    pub unresolved: usize,
    pub left_only: usize,
    pub right_only: usize,
    pub unavailable: usize,
    pub total: usize,
    pub rows: Vec<PairingRow>,
    pub options: PairingOptions,
    pub note: String,
    pub can_undo: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEntryChoice {
    pub id: String,
    pub label: String,
}
#[derive(Clone)]
struct Entry {
    id: String,
    order: u64,
    label: String,
    method: String,
    url: String,
    source: String,
    original: String,
    headers: HeaderBlock,
    response: Option<HeaderBlock>,
    status: Option<u16>,
    redacted: bool,
    body_digest: Option<String>,
    duration: Option<u64>,
}
#[derive(Clone, Default)]
struct Choices {
    pairs: HashMap<String, Option<String>>,
    right_only: HashSet<String>,
}
fn choices_charge(choices: &Choices) -> usize {
    choices
        .pairs
        .iter()
        .map(|(left, right)| left.len() + right.as_ref().map_or(0, String::len) + 96)
        .sum::<usize>()
        + choices
            .right_only
            .iter()
            .map(|id| id.len() + 64)
            .sum::<usize>()
}
fn save_undo(snapshot: &mut CaptureSnapshot) {
    let charge = choices_charge(&snapshot.choices);
    while !snapshot.undo.is_empty()
        && (snapshot.undo.len() >= 20
            || snapshot.undo.iter().map(choices_charge).sum::<usize>() + charge > 4 * 1024 * 1024)
    {
        snapshot.undo.remove(0);
    }
    if charge <= 4 * 1024 * 1024 {
        snapshot.undo.push(snapshot.choices.clone());
    }
}
pub(super) struct CaptureSnapshot {
    id: String,
    left: Vec<Entry>,
    left_label: String,
    left_details: String,
    right_details: String,
    right_label: String,
    right: Vec<Entry>,
    options: PairingOptions,
    choices: Choices,
    undo: Vec<Choices>,
    rows: Vec<PairingRow>,
}

impl Application {
    /// Snapshots explicit scope membership before any virtual-window paging.
    /// # Errors
    /// Returns invalid scope, cancellation, or finite metadata/work limits.
    pub async fn compare_captures(
        &self,
        operation_id: String,
        left: ComparisonScope,
        right: ComparisonScope,
        options: PairingOptions,
    ) -> Result<CaptureComparison, AppError> {
        let app = self.clone();
        let op = self.comparisons.begin(&operation_id)?;
        let _cancel_on_drop = super::CancelOnDrop(op.canceled.clone());
        tokio::task::spawn_blocking(move || {
            validate_options(&options)?;
            let left_label = scope_label(&app, &left);
            let right_label = scope_label(&app, &right);
            let (left, left_details) = scope(&app, left, &op)?;
            let (right, right_details) = scope(&app, right, &op)?;
            let mut snapshot = CaptureSnapshot {
                id: token()?,
                left_label,
                right_label,
                left_details,
                right_details,
                left,
                right,
                options,
                choices: Choices::default(),
                undo: vec![],
                rows: vec![],
            };
            op.check()?;
            snapshot.rows = pair_rows(&snapshot, Some(&op))?;
            let result = view(&snapshot, 0)?;
            let mut registry = app
                .comparisons
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if registry.captures.len() == 4 {
                registry.captures.pop_front();
            }
            registry.captures.push_back(snapshot);
            Ok(result)
        })
        .await
        .map_err(|_| error("Capture comparison worker failed"))?
    }
    /// Pages frozen pairings, changes criteria, corrects correspondence, or undoes a correction.
    /// # Errors
    /// Returns expired scopes or invalid/manual cross-scope pairs.
    #[allow(clippy::too_many_arguments)]
    pub fn capture_comparison_page(
        &self,
        id: &str,
        offset: usize,
        options: Option<PairingOptions>,
        left: Option<String>,
        right: Option<String>,
        action: &str,
    ) -> Result<CaptureComparison, AppError> {
        if !matches!(action, "" | "pair" | "unmatched" | "undo" | "reset")
            || (action == "pair" && (left.is_none() || right.is_none()))
        {
            return Err(error(
                "Select both counterparts for a pair, or a valid pairing action",
            ));
        }
        if let Some(options) = &options {
            validate_options(options)?;
        }
        let mut registry = self
            .comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = registry
            .captures
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| error("Capture comparison expired; select scopes again"))?;
        if matches!(action, "pair" | "unmatched")
            && (left
                .as_ref()
                .is_some_and(|id| !snapshot.left.iter().any(|e| &e.id == id))
                || right
                    .as_ref()
                    .is_some_and(|id| !snapshot.right.iter().any(|e| &e.id == id))
                || (left.is_none() && right.is_none()))
        {
            return Err(error("Select counterparts within this frozen scope"));
        }
        let changed = options.is_some() || !action.is_empty();
        if let Some(options) = options {
            validate_options(&options)?;
            snapshot.options = options;
        }
        match action {
            "pair" | "unmatched" => {
                save_undo(snapshot);
                let previous = &snapshot.rows;
                for row in previous {
                    if (left.is_some() && row.left == left)
                        || (right.is_some() && row.right == right)
                    {
                        if let Some(old_left) = &row.left {
                            snapshot.choices.pairs.insert(old_left.clone(), None);
                        }
                        if let Some(old_right) = &row.right {
                            snapshot.choices.right_only.insert(old_right.clone());
                        }
                    }
                }
                if action == "pair"
                    && let (Some(left), Some(right)) = (left, right)
                {
                    snapshot.choices.right_only.remove(&right);
                    snapshot.choices.pairs.insert(left, Some(right));
                }
            }
            "undo" => {
                if let Some(choices) = snapshot.undo.pop() {
                    snapshot.choices = choices;
                }
            }
            "reset" => {
                save_undo(snapshot);
                snapshot.choices = Choices::default();
            }
            "" => {}
            _ => return Err(error("Invalid pairing action")),
        }
        if changed {
            snapshot.rows = pair_rows(snapshot, None)?;
        }
        view(snapshot, offset)
    }
    /// Lists original frozen scope entries for choosing another counterpart.
    /// # Errors
    /// Returns an expired scope or invalid side.
    pub fn capture_comparison_entries(
        &self,
        id: &str,
        side: &str,
        search: &str,
        offset: usize,
    ) -> Result<Vec<CaptureEntryChoice>, AppError> {
        if search.len() > 1024 {
            return Err(error("Counterpart search is too long"));
        }
        let registry = self
            .comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let s = registry
            .captures
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| error("Capture comparison expired"))?;
        let entries = match side {
            "left" => &s.left,
            "right" => &s.right,
            _ => return Err(error("Invalid scope side")),
        };
        let search = search.to_lowercase();
        Ok(entries
            .iter()
            .filter(|e| e.label.to_lowercase().contains(&search))
            .skip(offset)
            .take(100)
            .map(|e| CaptureEntryChoice {
                id: e.id.clone(),
                label: e.label.clone(),
            })
            .collect())
    }
    /// Releases scope metadata independently of an open pair detail.
    pub fn close_capture_comparison(&self, id: &str) {
        self.comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .captures
            .retain(|s| s.id != id);
    }
}

#[allow(clippy::too_many_lines)] // One metadata-only acquisition pass enforces scope count and byte budgets.
fn scope(
    app: &Application,
    scope: ComparisonScope,
    op: &Operation,
) -> Result<(Vec<Entry>, String), AppError> {
    let ids = match scope {
        ComparisonScope::Selected { ids } => ids,
        ComparisonScope::Filtered { query } => app.matching_traffic_ids(&query)?,
        ComparisonScope::Source { trace_id } => app
            .service
            .catalog()
            .project_retained(|s| {
                let id = format!("{:032x}", s.exchange_id.0);
                (app.traces.source_id(&id) == trace_id).then_some(id)
            })
            .into_iter()
            .flatten()
            .collect(),
    };
    if ids.len() > 50_000 {
        return Err(error(
            "Capture comparison accepts up to 50,000 entries per side; narrow the scope",
        ));
    }
    let mut seen = HashSet::new();
    let mut entries = vec![];
    let mut charge = 0;
    let mut formats = BTreeMap::<&str, usize>::new();
    let mut first = None;
    let mut last = None;
    let mut unknown_starts = 0;
    let mut limited = 0;
    for id in ids {
        op.check()?;
        if !seen.insert(id.clone()) {
            continue;
        }
        let numeric = crate::inspector::parse_session_id(&id)?;
        let Some(snapshot) = app
            .service
            .catalog()
            .get(transmog_core::intercept::ExchangeId(numeric))
        else {
            return Err(error(
                "A selected scope entry expired; choose the scope again",
            ));
        };
        let trace = app.traces.entry(&id);
        let head = snapshot
            .request_heads
            .iter()
            .find(|h| h.boundary == transmog_core::observe::ExchangeBoundary::ClientRequest);
        let response = snapshot
            .response_heads
            .iter()
            .find(|h| h.boundary == transmog_core::observe::ExchangeBoundary::ClientResponse);
        let method = head.map_or_else(String::new, |h| {
            if h.head.method.len() <= 64 * 1024 {
                h.head.method.clone()
            } else {
                "Method exceeds comparison budget".into()
            }
        });
        let url = head
            .filter(|h| super::subjects::request_line_available(&h.head))
            .map_or_else(String::new, |h| crate::request_actions::url(&h.head));
        let headers = head.map_or_else(HeaderBlock::default, |h| h.head.headers.clone());
        let source = trace
            .as_ref()
            .map_or_else(|| "Live traffic".into(), |t| t.trace_id.clone());
        let original = trace
            .as_ref()
            .map_or_else(String::new, |t| t.original_id.clone());
        charge += 512
            + method.len()
            + url.len()
            + headers
                .iter()
                .map(|f| f.name().len() + f.value().len() + 64)
                .sum::<usize>()
            + response.map_or(0, |h| {
                h.head
                    .headers
                    .iter()
                    .map(|f| f.name().len() + f.value().len() + 64)
                    .sum::<usize>()
            });
        if charge > 32 * 1024 * 1024 {
            return Err(error(
                "Scope metadata exceeds 32 MiB per side; narrow the scope",
            ));
        }
        let redacted = headers
            .iter()
            .chain(response.into_iter().flat_map(|r| r.head.headers.iter()))
            .any(transmog_core::HeaderField::is_redacted);
        let body_digest = app.body_store.as_ref().and_then(|s| {
            s.metadata(snapshot.exchange_id)
                .into_iter()
                .find(|m| {
                    m.boundary == "client-request"
                        && m.availability == crate::BodyAvailability::Complete
                        && m.length_known
                })
                .and_then(|m| m.sha256)
        });
        let source_format = trace
            .as_ref()
            .and_then(|t| app.traces.metadata(&t.trace_id))
            .map_or("live", |m| m.format);
        *formats.entry(source_format).or_default() += 1;
        if let Ok(duration) = snapshot
            .metadata
            .started_at
            .duration_since(std::time::UNIX_EPOCH)
            && !duration.is_zero()
        {
            let millis = duration.as_millis();
            first = Some(first.map_or(millis, |old: u128| old.min(millis)));
            last = Some(last.map_or(millis, |old: u128| old.max(millis)));
        } else {
            unknown_starts += 1;
        }
        if redacted
            || snapshot.sequence_loss > 0
            || trace
                .as_ref()
                .is_some_and(|t| !t.target_known || !t.protocol_known || !t.diagnostics.is_empty())
        {
            limited += 1;
        }
        let native_timing = trace.as_ref().is_none_or(|t| {
            !t.timings
                .keys()
                .any(|k| k.starts_with("HAR ") || k.starts_with("NetLog "))
                && app
                    .traces
                    .metadata(&t.trace_id)
                    .is_some_and(|m| m.format == "native")
        });
        let duration = native_timing
            .then(|| snapshot.performance.elapsed_micros())
            .flatten();
        entries.push(Entry {
            order: snapshot.request_number,
            id,
            label: label(&format!(
                "#{} · {} {} · {}",
                snapshot.request_number,
                method,
                url,
                trace
                    .as_ref()
                    .and_then(|entry| app.traces.metadata(&entry.trace_id))
                    .map_or_else(|| "Live traffic".into(), |m| m.name)
            )),
            method,
            url: if trace.is_some_and(|e| !e.target_known) {
                String::new()
            } else {
                url
            },
            source,
            original,
            headers,
            response: response.map(|h| h.head.headers.clone()),
            status: response.map(|h| h.head.status),
            redacted,
            body_digest,
            duration,
        });
    }
    entries.sort_by_key(|entry| entry.order);
    let date = |millis: Option<u128>| {
        millis
            .and_then(|ms| i128::try_from(ms).ok())
            .and_then(|ms| time::OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000).ok())
            .and_then(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .unwrap_or_else(|| "Unavailable".into())
    };
    let formats = formats
        .into_iter()
        .map(|(name, count)| format!("{name}: {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let details = format!(
        "Recorded starts (UTC, source clock): {} to {}; {unknown_starts} unavailable. Stored formats: {formats}. {limited} entries with recorded fidelity limitations; body contents not assessed. Source clocks do not establish cross-capture ordering.",
        date(first),
        date(last)
    );
    Ok((entries, details))
}
fn validate_options(options: &PairingOptions) -> Result<(), AppError> {
    if options.excluded_query_names.len() > 64
        || options.request_headers.len() > 32
        || options
            .excluded_query_names
            .iter()
            .chain(&options.request_headers)
            .any(|s| s.is_empty() || s.len() > 256)
    {
        return Err(error(
            "Pairing criteria exceed 64 query names / 32 request headers",
        ));
    }
    Ok(())
}

fn key(entry: &Entry, options: &PairingOptions) -> Option<String> {
    let url = normalize_url(&entry.url, &options.excluded_query_names)?;
    let mut value = format!("{} {url}", entry.method);
    for name in &options.request_headers {
        write!(&mut value, "\n{}:", name.to_ascii_lowercase()).expect("String write");
        for field in entry
            .headers
            .iter()
            .filter(|f| f.name().eq_ignore_ascii_case(name.as_bytes()))
        {
            if field.is_redacted() {
                return None;
            }
            write!(
                &mut value,
                "{}:{};",
                field.value().len(),
                hex(field.value())
            )
            .expect("String write");
        }
    }
    if options.request_body_digest {
        value.push_str("\nbody:");
        value.push_str(entry.body_digest.as_deref()?);
    }
    Some(value)
}
fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn normalize_url(text: &str, excluded: &[String]) -> Option<String> {
    let parsed = url::Url::parse(text).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let (_, tail) = text.split_once("://")?;
    let path_start = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let raw = &tail[path_start..];
    if raw.contains('#') {
        return None;
    }
    let (path, query) = raw
        .split_once('?')
        .map_or((raw, None), |(p, q)| (p, Some(q)));
    let path = if path.is_empty() { "/" } else { path };
    let host = parsed.host_str()?;
    let authority = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.into()
    };
    let port = parsed.port().map_or_else(String::new, |p| format!(":{p}"));
    let query = query
        .map(|q| {
            let pairs = q
                .split('&')
                .filter(|pair| {
                    let name = pair.split_once('=').map_or(*pair, |(n, _)| n);
                    let decoded = percent_encoding::percent_decode_str(name).decode_utf8_lossy();
                    !excluded.iter().any(|e| e == &decoded)
                })
                .collect::<Vec<_>>()
                .join("&");
            format!("?{pairs}")
        })
        .unwrap_or_default();
    Some(format!(
        "{}://{authority}{port}{path}{query}",
        parsed.scheme()
    ))
}

#[allow(clippy::too_many_lines)] // Manual, provenance and proposed pairs share one occurrence-accounting pass.
fn pair_rows(s: &CaptureSnapshot, op: Option<&Operation>) -> Result<Vec<PairingRow>, AppError> {
    let mut rows = vec![];
    let mut used_l = HashSet::new();
    let mut used_r = s.choices.right_only.clone();
    let lookup_r = s
        .right
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect::<HashMap<_, _>>();
    for entry in &s.left {
        if let Some(op) = op {
            op.check()?;
        }
        if let Some(right) = s.choices.pairs.get(&entry.id) {
            used_l.insert(entry.id.clone());
            let right = right
                .as_ref()
                .and_then(|id| lookup_r.get(id.as_str()).copied());
            if let Some(e) = right {
                used_r.insert(e.id.clone());
            }
            rows.push(row(Some(entry), right, "Explicit user choice", false));
        }
    }
    let mut groups = BTreeMap::<String, (Vec<&Entry>, Vec<&Entry>)>::new();
    for (is_left, entries) in [(true, &s.left), (false, &s.right)] {
        for e in entries {
            if let Some(op) = op {
                op.check()?;
            }
            if if is_left {
                used_l.contains(&e.id)
            } else {
                used_r.contains(&e.id)
            } {
                continue;
            }
            if let Some(key) = key(e, &s.options) {
                let group = groups.entry(key).or_default();
                if is_left {
                    group.0.push(e);
                } else {
                    group.1.push(e);
                }
            } else if is_left {
                used_l.insert(e.id.clone());
                rows.push(row(
                    Some(e),
                    None,
                    "Target or selected request evidence unavailable; manual pairing available",
                    true,
                ));
            } else {
                used_r.insert(e.id.clone());
                rows.push(row(
                    None,
                    Some(e),
                    "Target or selected request evidence unavailable; manual pairing available",
                    true,
                ));
            }
        }
    }
    for (group, (a, b)) in groups {
        if let Some(op) = op {
            op.check()?;
        }
        if a.len() == 1 && b.len() == 1 {
            used_l.insert(a[0].id.clone());
            used_r.insert(b[0].id.clone());
            rows.push(row(
                Some(a[0]),
                Some(b[0]),
                "Exact unique request key",
                false,
            ));
            continue;
        }
        // Shared source identity proves an occurrence only within this loaded namespace.
        let mut anchors = HashMap::<(&str, &str), (usize, usize, Option<&Entry>)>::new();
        for left in &a {
            if !left.original.is_empty() {
                anchors.entry((&left.source, &left.original)).or_default().0 += 1;
            }
        }
        for right in &b {
            if !right.original.is_empty() {
                let anchor = anchors.entry((&right.source, &right.original)).or_default();
                anchor.1 += 1;
                anchor.2 = Some(right);
            }
        }
        for left in &a {
            if let Some((1, 1, Some(right))) =
                anchors.get(&(left.source.as_str(), left.original.as_str()))
            {
                used_l.insert(left.id.clone());
                used_r.insert(right.id.clone());
                rows.push(row(
                    Some(left),
                    Some(right),
                    "Unambiguous shared source provenance",
                    false,
                ));
            }
        }

        let a = a
            .into_iter()
            .filter(|e| !used_l.contains(&e.id))
            .collect::<Vec<_>>();
        let b = b
            .into_iter()
            .filter(|e| !used_r.contains(&e.id))
            .collect::<Vec<_>>();
        for i in 0..a.len().max(b.len()) {
            let left = a.get(i).copied();
            let right = b.get(i).copied();
            if let Some(e) = left {
                used_l.insert(e.id.clone());
            }
            if let Some(e) = right {
                used_r.insert(e.id.clone());
            }
            let mut entry = row(
                left,
                right,
                if a.len() > 1 || b.len() > 1 {
                    "Proposed occurrence order; repeated key may contain retries or a missing middle request"
                } else {
                    "Only observed in the selected scope"
                },
                a.len() > 1 || b.len() > 1,
            );
            entry.group = label(&group);
            rows.push(entry);
        }
    }
    for e in &s.right {
        if s.choices.right_only.contains(&e.id) {
            rows.push(row(None, Some(e), "Explicitly kept unmatched", false));
        }
    }
    for (index, row) in rows.iter_mut().enumerate() {
        row.index = index;
    }
    Ok(rows)
}
fn row(a: Option<&Entry>, b: Option<&Entry>, reason: &str, ambiguous: bool) -> PairingRow {
    let state = if ambiguous {
        "unresolved"
    } else {
        match (a, b) {
            (Some(a), Some(b)) if a.response.is_none() || b.response.is_none() => "unavailable",
            (Some(a), Some(b))
                if a.method != b.method
                    || a.url != b.url
                    || a.status != b.status
                    || a.headers != b.headers
                    || a.response != b.response =>
            {
                "changed"
            }
            (Some(a), Some(b)) if a.redacted || b.redacted => "unavailable",
            (Some(_), Some(_)) => "equal metadata",
            (Some(_), None) => "left only",
            _ => "right only",
        }
    };
    PairingRow {
        index: 0,
        left: a.map(|e| e.id.clone()),
        right: b.map(|e| e.id.clone()),
        left_label: a.map_or_else(|| "Not observed in scope".into(), |e| e.label.clone()),
        right_label: b.map_or_else(|| "Not observed in scope".into(), |e| e.label.clone()),
        reason: reason.into(),
        state: state.into(),
        group: String::new(),
        timing_delta: a
            .and_then(|e| e.duration)
            .zip(b.and_then(|e| e.duration))
            .map(|(left, right)| format!("{:+} µs", i128::from(right) - i128::from(left))),
    }
}
fn view(s: &CaptureSnapshot, offset: usize) -> Result<CaptureComparison, AppError> {
    let rows = &s.rows;
    if offset > rows.len() {
        return Err(error("Invalid pairing page"));
    }
    let count = |state: &str| rows.iter().filter(|r| r.state == state).count();
    Ok(CaptureComparison{id:s.id.clone(),left_label:s.left_label.clone(),left_details:s.left_details.clone(),right_details:s.right_details.clone(),right_label:s.right_label.clone(),left_count:s.left.len(),right_count:s.right.len(),changed:count("changed"),equal_metadata:count("equal metadata"),unresolved:count("unresolved"),left_only:count("left only"),right_only:count("right only"),unavailable:count("unavailable"),total:rows.len(),rows:rows.iter().skip(offset).take(100).cloned().collect(),options:s.options.clone(),can_undo:!s.undo.is_empty(),note:"Bodies not compared; open a pair for content comparison. Timing deltas appear only for matching native request-header-to-terminal measurements; imported timing definitions are otherwise not compared. Presence means observed within these frozen scopes. Repeated-key order is a proposal, not confirmed correspondence. Pairing query/header criteria do not exclude differences from pair detail.".into()})
}

fn label(text: &str) -> String {
    let mut label = text.chars().take(256).collect::<String>();
    if text.chars().count() > 256 {
        label.push('…');
    }
    label
}
fn scope_label(app: &Application, scope: &ComparisonScope) -> String {
    match scope {
        ComparisonScope::Source { trace_id: Some(id) } => app
            .traces
            .metadata(id)
            .map_or_else(|| "Unavailable source".into(), |m| super::short(&m.name)),
        ComparisonScope::Source { trace_id: None } => "Live traffic snapshot".into(),
        ComparisonScope::Selected { .. } => "Selected request snapshot (source order)".into(),
        ComparisonScope::Filtered { query } => super::short(&format!(
            "Filter snapshot · {}",
            serde_json::to_string(query).unwrap_or_else(|_| "Filter unavailable".into())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::Ordering, time::Instant};
    #[test]
    fn large_repeated_key_group_is_bounded_and_never_confirmed_by_response() {
        let entry = |side: &str, index: usize| Entry {
            id: format!("{side}-{index}"),
            order: u64::try_from(index).unwrap(),
            label: format!("{side} #{index}"),
            method: "GET".into(),
            url: "https://repeat.test/api".into(),
            source: side.into(),
            original: index.to_string(),
            headers: HeaderBlock::default(),
            response: Some(HeaderBlock::default()),
            status: Some(if side == "left" { 200 } else { 500 }),
            redacted: false,
            body_digest: None,
            duration: None,
        };
        let mut snapshot = CaptureSnapshot {
            id: "large".into(),
            left_label: "Left fixture".into(),
            right_label: "Right fixture".into(),
            left_details: String::new(),
            right_details: String::new(),
            left: (0..25_000).map(|i| entry("left", i)).collect(),
            right: (0..25_000)
                .filter(|i| *i != 12_000)
                .map(|i| entry("right", i))
                .collect(),
            options: PairingOptions::default(),
            choices: Choices::default(),
            undo: vec![],
            rows: vec![],
        };
        let registry = super::super::ComparisonRegistry::default();
        let op = registry.begin("large").unwrap();
        let began = Instant::now();
        snapshot.rows = pair_rows(&snapshot, Some(&op)).unwrap();
        assert_eq!(snapshot.rows.len(), 25_000);
        assert!(snapshot.rows.iter().all(|r| r.state == "unresolved"));
        assert_eq!(view(&snapshot, 100).unwrap().rows.len(), 100);
        eprintln!(
            "25,000 repeated-key entries per side: {} ms",
            began.elapsed().as_millis()
        );
        op.canceled.store(true, Ordering::Release);
        assert!(pair_rows(&snapshot, Some(&op)).is_err());
    }
}
