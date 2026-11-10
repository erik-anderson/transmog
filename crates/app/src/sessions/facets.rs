//! Typed alternatives and contextual counts share the retained-traffic predicate.
use super::{
    AppError, ApplicationSessionService, ErrorCategory, PreparedFilter, SessionQueryInput,
    SessionSummary, TrafficColumn, numeric_column, numeric_value, ordered_rows, text_value,
    validate_fetch_destinations, validate_filter,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::LazyLock;

// Shared with the desktop picker: presets, aliases and Other must not drift.
#[derive(Deserialize)]
struct Preset {
    value: String,
    #[serde(default)]
    aliases: Vec<String>,
}

static CATALOG: LazyLock<BTreeMap<String, Vec<Preset>>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("filter-catalog.json"))
        .expect("valid traffic filter catalogue")
});

fn presets(dimension: TrafficDimension) -> Option<&'static [Preset]> {
    let key = match dimension {
        TrafficDimension::Column(TrafficColumn::Method) => "method",
        TrafficDimension::Column(TrafficColumn::Protocol) => "protocol",
        TrafficDimension::Column(TrafficColumn::State) => "state",
        TrafficDimension::Facet(TrafficFacet::TrafficType) => "traffic-type",
        TrafficDimension::Facet(TrafficFacet::FetchDestination) => "fetch-destination",
        _ => return None,
    };
    Some(&CATALOG[key])
}

pub(super) fn canonical_protocol(value: &str) -> String {
    // Only recorded HTTP spellings have aliases; internal enums use canonical labels at source.
    let value = value.to_lowercase();
    CATALOG["protocol"]
        .iter()
        .find(|preset| preset.value == value || preset.aliases.contains(&value))
        .map_or(value.clone(), |preset| preset.value.clone())
}

pub(super) fn known_fetch_destination(value: &str) -> bool {
    CATALOG["fetch-destination"]
        .iter()
        .any(|preset| preset.value == value)
}

fn supports_other(dimension: TrafficDimension) -> bool {
    presets(dimension).is_some() || dimension == TrafficDimension::Column(TrafficColumn::Status)
}

fn is_other(dimension: TrafficDimension, value: &TrafficFilterValue) -> bool {
    match value {
        TrafficFilterValue::Text(text) => presets(dimension)
            .is_some_and(|presets| !presets.iter().any(|preset| preset.value == *text)),
        TrafficFilterValue::Number(number)
            if dimension == TrafficDimension::Column(TrafficColumn::Status) =>
        {
            number
                .parse::<u64>()
                .is_ok_and(|status| !(100..600).contains(&status))
        }
        _ => false,
    }
}

/// Metadata dimensions which do not require a visible table column.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficFacet {
    /// HTTP, WebSocket or SSE, independently of HTTP version and lifecycle.
    TrafficType,
    /// Original Sec-Fetch-Dest.
    FetchDestination,
}

/// An existing column identity or a metadata-only facet identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(untagged)]
pub enum TrafficDimension {
    /// Column metadata (visibility is irrelevant).
    Column(TrafficColumn),
    /// Additional metadata.
    Facet(TrafficFacet),
}

/// One exact alternative. Decimal numbers travel as text to preserve u64 precision.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum TrafficFilterValue {
    /// Case-insensitive exact text.
    Text(String),
    /// Nonnegative decimal integer, including measured zero.
    Number(String),
    /// HTTP status family, 1 through 5 (e.g. 4 means 400–499).
    StatusFamily(u16),
    /// Missing evidence, distinct from empty text or measured zero.
    Unavailable,
    /// Present values outside the shared presets (status: outside 100–599).
    Other,
    /// One or more whole DNS labels below this Host suffix, excluding its apex.
    HostSubdomains(String),
}

/// How two independently selected membership lists combine.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficSelectionMode {
    /// Match either populated selection.
    #[default]
    Either,
    /// Match every populated selection.
    Both,
    /// Retain saved checkmarks without applying them.
    IgnoreSaved,
}

/// OR alternatives in one dimension; distinct dimensions are combined with AND.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficFilterGroup {
    /// Metadata identity, independently of visible columns.
    pub dimension: TrafficDimension,
    /// At most 128 alternatives. Empty means unrestricted.
    pub values: Vec<TrafficFilterValue>,
    /// Reusable terms copied into this query; independent of the Favorite library.
    #[serde(default)]
    pub saved_values: Vec<TrafficFilterValue>,
    /// Membership combination, defaulting to either for older payloads.
    #[serde(default)]
    pub selection_mode: TrafficSelectionMode,
}

/// A metadata-only facet request; table pagination never limits its counts.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficFacetInput {
    /// Complete traffic scope, including search and incident/content snapshots.
    pub query: SessionQueryInput,
    /// Exclude all constraints belonging to this dimension before counting.
    pub dimension: TrafficDimension,
    /// Optional case-insensitive option search (does not filter traffic entries).
    pub search: Option<String>,
    /// Explicit fixed/selected values counted independently of option search and paging.
    #[serde(default)]
    pub include_values: Vec<TrafficFilterValue>,
    /// Offset in stable value order. Each request reads a fresh consistent snapshot.
    #[serde(default)]
    pub offset: usize,
    /// 1 through 512; defaults to 100.
    pub limit: Option<usize>,
}

/// A counted metadata option; counts entries, never stream messages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficFacetOption {
    /// Stable typed option key, independent of its count or label.
    pub key: String,
    /// Readable metadata label; custom text remains case-insensitive.
    pub label: String,
    /// Exact typed value accepted by a filter group.
    pub value: TrafficFilterValue,
    /// Entries matching this option and all other dimensions.
    pub count: usize,
    /// Explanation of this option under the edited field's draft.
    pub match_reason: String,
}

/// Successful facet snapshot. Errors remain errors, distinct from zero matches.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficFacetPage {
    /// Edited dimension.
    pub dimension: TrafficDimension,
    /// Bounded options in stable value order, after option search.
    pub options: Vec<TrafficFacetOption>,
    /// Explicit requested alternatives, including measured zero matches.
    pub included_options: Vec<TrafficFacetOption>,
    /// Entries matching the surrounding scope with this dimension excluded.
    pub matching_entries: usize,
    /// Unique requests passing the edited field's complete draft before paging.
    pub draft_entries: usize,
    /// Entries whose exact value exceeds filter bounds or is unsupported for this dimension.
    /// These values are omitted, never truncated or silently relabelled as unavailable.
    pub omitted_entries: usize,
    /// Number of matching distinct selectable options before paging.
    pub total_options: usize,
    /// Continue with this offset and the same query/search; live changes can move options.
    pub next_offset: Option<usize>,
}

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

pub(super) fn prepare_groups(
    groups: &[TrafficFilterGroup],
) -> Result<Vec<PreparedGroup>, AppError> {
    if groups.len() > 32 {
        return Err(invalid(
            "Traffic filters accept at most 32 dimensions and 1024 alternatives",
        ));
    }
    let mut dimensions = BTreeSet::new();
    let prepared: Vec<_> = groups
        .iter()
        .map(|group| {
            if !dimensions.insert(group.dimension) {
                return Err(invalid(
                    "Use one group per dimension with at most 128 alternatives",
                ));
            }
            PreparedGroup::new(group)
        })
        .collect::<Result<_, _>>()?;
    if prepared
        .iter()
        .map(|group| group.picked.len() + group.saved.len())
        .sum::<usize>()
        > 1024
    {
        return Err(invalid("Choose at most 1024 alternatives across fields"));
    }
    Ok(prepared)
}

fn is_numeric(dimension: TrafficDimension) -> bool {
    match dimension {
        TrafficDimension::Column(column) => numeric_column(column),
        TrafficDimension::Facet(_) => false,
    }
}

pub(crate) fn prepare_value(
    dimension: TrafficDimension,
    value: &TrafficFilterValue,
) -> Result<TrafficFilterValue, AppError> {
    match value {
        TrafficFilterValue::HostSubdomains(host)
            if dimension == TrafficDimension::Column(TrafficColumn::Host) =>
        {
            let host = canonical_host(host)?;
            if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
                return Err(invalid(
                    "Host wildcards require a DNS name, not an IP literal",
                ));
            }
            Ok(TrafficFilterValue::HostSubdomains(host))
        }
        TrafficFilterValue::Unavailable => Ok(value.clone()),
        TrafficFilterValue::Other if supports_other(dimension) => Ok(value.clone()),
        TrafficFilterValue::StatusFamily(family)
            if dimension == TrafficDimension::Column(TrafficColumn::Status)
                && (1..=5).contains(family) =>
        {
            Ok(value.clone())
        }
        TrafficFilterValue::Number(number) if is_numeric(dimension) => {
            validate_filter(Some(number))?;
            if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid(
                    "Numeric alternatives require an unsigned decimal integer",
                ));
            }
            let parsed = number
                .parse::<u64>()
                .map_err(|_| invalid("Numeric alternative exceeds u64"))?;
            if dimension == TrafficDimension::Column(TrafficColumn::RequestNumber) && parsed == 0 {
                return Err(invalid("Request order must be positive"));
            }
            Ok(TrafficFilterValue::Number(parsed.to_string()))
        }
        TrafficFilterValue::Text(text) if !is_numeric(dimension) => {
            validate_filter(Some(text))?;
            let text = if dimension == TrafficDimension::Column(TrafficColumn::Protocol) {
                canonical_protocol(text)
            } else if dimension == TrafficDimension::Column(TrafficColumn::Host) {
                canonical_host(text)?
            } else if dimension == TrafficDimension::Column(TrafficColumn::ContentType) {
                text.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase()
            } else {
                text.to_lowercase()
            };
            if dimension == TrafficDimension::Facet(TrafficFacet::TrafficType)
                && !matches!(text.as_str(), "http" | "websocket" | "sse")
            {
                return Err(invalid("Traffic type must be http, websocket or sse"));
            }
            Ok(TrafficFilterValue::Text(text))
        }
        _ => Err(invalid(
            "Filter alternative does not match its metadata dimension",
        )),
    }
}

/// Strict host parsing shared by both sides of Traffic membership comparisons.
pub(crate) fn canonical_host(value: &str) -> Result<String, AppError> {
    if value.contains('*') || value.contains('%') || value.ends_with("..") {
        return Err(invalid("Enter an exact host or a leading *. DNS wildcard"));
    }
    let value = if value.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{value}]")
    } else {
        value.to_owned()
    };
    let host = transmog_capture::support::normalize_host(&value)
        .map_err(|_| invalid("Enter a host without scheme, port, path or credentials"))?;
    if !host.starts_with('[')
        && host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid("Enter a valid DNS hostname or IP literal"));
    }
    Ok(host)
}

fn value_for(row: &SessionSummary, dimension: TrafficDimension) -> TrafficFilterValue {
    let unavailable = match dimension {
        TrafficDimension::Column(TrafficColumn::Method) => row.method == "—",
        TrafficDimension::Column(TrafficColumn::ContentType) => row.content_type.is_none(),
        TrafficDimension::Column(TrafficColumn::Process) => row.caller.process_name.is_none(),
        TrafficDimension::Column(TrafficColumn::Protocol) => row.protocol == "Unavailable",
        TrafficDimension::Column(TrafficColumn::Url) => row.url == "Unavailable",
        _ => false,
    };
    if unavailable {
        return TrafficFilterValue::Unavailable;
    }
    match dimension {
        TrafficDimension::Column(column) if numeric_column(column) => numeric_value(row, column)
            .map_or(TrafficFilterValue::Unavailable, |value| {
                TrafficFilterValue::Number(value.to_string())
            }),
        TrafficDimension::Column(TrafficColumn::Protocol) => {
            TrafficFilterValue::Text(canonical_protocol(&row.protocol))
        }
        TrafficDimension::Column(TrafficColumn::Host) => TrafficFilterValue::Text(
            canonical_host(&row.host).unwrap_or_else(|_| row.host.to_lowercase()),
        ),
        TrafficDimension::Column(column) => {
            TrafficFilterValue::Text(text_value(row, column).to_lowercase())
        }
        TrafficDimension::Facet(TrafficFacet::TrafficType) => TrafficFilterValue::Text(
            match row.stream_protocol {
                Some(transmog_core::stream::StreamProtocol::WebSocket) => "websocket",
                Some(transmog_core::stream::StreamProtocol::Sse) => "sse",
                None => "http",
            }
            .into(),
        ),
        TrafficDimension::Facet(TrafficFacet::FetchDestination) => row
            .fetch_destination
            .as_ref()
            .map_or(TrafficFilterValue::Unavailable, |value| {
                TrafficFilterValue::Text(value.to_lowercase())
            }),
    }
}

pub(super) struct PreparedGroup {
    dimension: TrafficDimension,
    picked: PreparedSelection,
    saved: PreparedSelection,
    mode: TrafficSelectionMode,
}
struct PreparedSelection {
    exact: BTreeSet<TrafficFilterValue>,
    suffixes: Vec<String>,
    families: BTreeSet<u16>,
    other: bool,
    count: usize,
}
impl PreparedSelection {
    fn new(values: BTreeSet<TrafficFilterValue>) -> Self {
        let count = values.len();
        let mut result = Self {
            exact: BTreeSet::new(),
            suffixes: vec![],
            families: BTreeSet::new(),
            other: false,
            count,
        };
        for value in values {
            match value {
                TrafficFilterValue::HostSubdomains(host) => result.suffixes.push(host),
                TrafficFilterValue::StatusFamily(family) => {
                    result.families.insert(family);
                }
                TrafficFilterValue::Other => result.other = true,
                value => {
                    result.exact.insert(value);
                }
            }
        }
        result
    }
    fn len(&self) -> usize {
        self.count
    }
    fn is_empty(&self) -> bool {
        self.count == 0
    }
}
fn below(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix).is_some_and(|prefix| {
        prefix.strip_suffix('.').is_some_and(|labels| {
            labels.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        })
    })
}
impl PreparedGroup {
    fn new(group: &TrafficFilterGroup) -> Result<Self, AppError> {
        let prepare = |values: &[TrafficFilterValue]| {
            values
                .iter()
                .map(|value| prepare_value(group.dimension, value))
                .collect::<Result<BTreeSet<_>, _>>()
        };
        let picked = prepare(&group.values)?;
        let saved = prepare(&group.saved_values)?;
        if picked
            .iter()
            .any(|value| matches!(value, TrafficFilterValue::HostSubdomains(_)))
        {
            return Err(invalid("Observed choices require exact values"));
        }
        if picked.len() + saved.len() > 128 {
            return Err(invalid(
                "Choose at most 128 saved and picked alternatives per field",
            ));
        }
        Ok(Self {
            dimension: group.dimension,
            picked: PreparedSelection::new(picked),
            saved: PreparedSelection::new(saved),
            mode: group.selection_mode,
        })
    }
    fn includes(
        &self,
        values: &PreparedSelection,
        candidate: &TrafficFilterValue,
        row: &SessionSummary,
    ) -> bool {
        values.exact.contains(candidate)
            || row
                .status
                .is_some_and(|status| values.families.contains(&(status / 100)))
            || values.other && is_other(self.dimension, candidate)
            || matches!(candidate, TrafficFilterValue::Text(host) if values.suffixes.iter().any(|suffix| below(host, suffix)))
    }
    pub(super) fn matches(&self, row: &SessionSummary) -> bool {
        let candidate = value_for(row, self.dimension);
        let saved = self.includes(&self.saved, &candidate, row);
        let picked = self.includes(&self.picked, &candidate, row);
        match self.mode {
            TrafficSelectionMode::IgnoreSaved => self.picked.is_empty() || picked,
            TrafficSelectionMode::Both => {
                (self.saved.is_empty() || saved) && (self.picked.is_empty() || picked)
            }
            TrafficSelectionMode::Either => {
                self.saved.is_empty() && self.picked.is_empty() || saved || picked
            }
        }
    }
    fn reason(&self, row: &SessionSummary) -> String {
        let candidate = value_for(row, self.dimension);
        if !self.matches(row) {
            return if self.mode != TrafficSelectionMode::IgnoreSaved
                && !self.saved.is_empty()
                && !self.includes(&self.saved, &candidate, row)
            {
                if self.dimension == TrafficDimension::Column(TrafficColumn::Host) {
                    "Outside Host filter · No saved term matches".into()
                } else {
                    "Outside filter · No saved term matches".into()
                }
            } else {
                "Outside filter · Not picked in observed values".into()
            };
        }
        if self.mode != TrafficSelectionMode::IgnoreSaved {
            for suffix in &self.saved.suffixes {
                if matches!(&candidate, TrafficFilterValue::Text(host) if below(host, suffix)) {
                    return format!("Included by *.{suffix}");
                }
            }
        }
        String::new()
    }
}
#[cfg(test)]
impl TrafficFilterGroup {
    pub(super) fn matches(&self, row: &SessionSummary) -> bool {
        PreparedGroup::new(self).is_ok_and(|group| group.matches(row))
    }
}

fn count_included(
    dimension: TrafficDimension,
    included: BTreeSet<TrafficFilterValue>,
    counts: &BTreeMap<TrafficFilterValue, usize>,
) -> Vec<TrafficFacetOption> {
    included
        .into_iter()
        .map(|value| {
            let (key, label) = option_presentation(dimension, &value);
            let count = if let TrafficFilterValue::HostSubdomains(suffix) = &value {
                counts
                    .iter()
                    .filter_map(|(candidate, count)| match candidate {
                        TrafficFilterValue::Text(host)
                            if host.strip_suffix(suffix).is_some_and(|prefix| {
                                prefix.len() > 1 && prefix.ends_with('.')
                            }) =>
                        {
                            Some(count)
                        }
                        _ => None,
                    })
                    .sum()
            } else {
                counts.get(&value).copied().unwrap_or(0)
            };
            TrafficFacetOption {
                key,
                label,
                value,
                count,
                match_reason: String::new(),
            }
        })
        .collect()
}

fn prepare_included(request: &TrafficFacetInput) -> Result<BTreeSet<TrafficFilterValue>, AppError> {
    if request.include_values.len() > 256 {
        return Err(invalid("Request at most 256 explicit facet values"));
    }
    request
        .include_values
        .iter()
        .map(|value| prepare_value(request.dimension, value))
        .collect::<Result<BTreeSet<_>, _>>()
}

struct Draft {
    group: Option<PreparedGroup>,
    filters: Vec<PreparedFilter>,
    legacy: SessionQueryInput,
}
impl Draft {
    fn new(request: &TrafficFacetInput) -> Result<Self, AppError> {
        let groups = prepare_groups(&request.query.filter_groups)?;
        let group = groups
            .into_iter()
            .find(|group| group.dimension == request.dimension);
        let filters = request
            .query
            .filters
            .iter()
            .filter(|filter| TrafficDimension::Column(filter.column) == request.dimension)
            .map(PreparedFilter::new)
            .collect::<Result<Vec<_>, _>>()?;
        let legacy = SessionQueryInput {
            method: (request.dimension == TrafficDimension::Column(TrafficColumn::Method))
                .then(|| request.query.method.clone())
                .flatten(),
            host: (request.dimension == TrafficDimension::Column(TrafficColumn::Host))
                .then(|| request.query.host.clone())
                .flatten(),
            terminal: (request.dimension == TrafficDimension::Column(TrafficColumn::State))
                .then_some(request.query.terminal)
                .flatten(),
            fetch_destinations: if request.dimension
                == TrafficDimension::Facet(TrafficFacet::FetchDestination)
            {
                request.query.fetch_destinations.clone()
            } else {
                vec![]
            },
            ..Default::default()
        };
        Ok(Self {
            group,
            filters,
            legacy,
        })
    }
    fn matches(&self, row: &SessionSummary) -> bool {
        self.group.as_ref().is_none_or(|group| group.matches(row))
            && super::matches_query_row(row, &self.legacy, None, None, &self.filters)
    }
    fn reason(&self, row: &SessionSummary) -> String {
        self.filters
            .iter()
            .find(|filter| !filter.matches(row))
            .map_or_else(
                || {
                    self.group
                        .as_ref()
                        .map_or_else(String::new, |group| group.reason(row))
                },
                |filter| {
                    format!(
                        "Outside filter · Requires {:?} {}",
                        filter.operator, filter.text
                    )
                },
            )
    }
}

pub(crate) fn facet_counts(
    service: &ApplicationSessionService,
    request: &TrafficFacetInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&HashSet<String>>,
) -> Result<TrafficFacetPage, AppError> {
    validate_filter(request.search.as_deref())?;
    // Validate even constraints about to be excluded; malformed requests never succeed silently.
    let draft = Draft::new(request)?;
    validate_filter(request.query.method.as_deref())?;
    validate_filter(request.query.host.as_deref())?;
    validate_fetch_destinations(&request.query.fetch_destinations)?;
    if request.query.filters.len() > 14 {
        return Err(invalid("At most 14 legacy constraints are supported"));
    }
    for filter in &request.query.filters {
        PreparedFilter::new(filter)?;
    }
    let included = prepare_included(request)?;
    let limit = request.limit.unwrap_or(100);
    if !(1..=512).contains(&limit) {
        return Err(invalid("Request between 1 and 512 facet options"));
    }
    let query = unrestricted_query(request);
    let (rows, _) = ordered_rows(service, &query, traces, matching)?;
    let draft_entries = rows.iter().filter(|row| draft.matches(row)).count();
    let mut counts = BTreeMap::<TrafficFilterValue, usize>::new();
    let mut reasons = BTreeMap::new();
    let mut omitted_entries = 0;
    for row in &rows {
        let value = value_for(row, request.dimension);
        let reason = draft.reason(row);
        reasons.insert(value.clone(), reason);
        if is_other(request.dimension, &value) {
            *counts.entry(TrafficFilterValue::Other).or_default() += 1;
        }
        match prepare_value(request.dimension, &value) {
            Ok(value) => *counts.entry(value).or_default() += 1,
            Err(_) => omitted_entries += 1,
        }
        if request.dimension == TrafficDimension::Column(TrafficColumn::Status)
            && let Some(status) = row.status
            && (100..600).contains(&status)
        {
            *counts
                .entry(TrafficFilterValue::StatusFamily(status / 100))
                .or_default() += 1;
        }
    }
    let included_options = count_included(request.dimension, included, &counts);
    let search = request.search.as_deref().unwrap_or_default().to_lowercase();
    let options: Vec<_> = counts
        .into_iter()
        .filter(|(value, _)| {
            let text = match value {
                TrafficFilterValue::Text(text)
                | TrafficFilterValue::Number(text)
                | TrafficFilterValue::HostSubdomains(text) => text.clone(),
                TrafficFilterValue::StatusFamily(family) => format!("{family}xx"),
                TrafficFilterValue::Unavailable => "unavailable".into(),
                TrafficFilterValue::Other => "other".into(),
            };
            text.contains(&search)
        })
        .map(|(value, count)| {
            let (key, label) = option_presentation(request.dimension, &value);
            TrafficFacetOption {
                key,
                label,
                match_reason: reasons.remove(&value).unwrap_or_default(),
                value,
                count,
            }
        })
        .collect();
    let total_options = options.len();
    Ok(TrafficFacetPage {
        dimension: request.dimension,
        matching_entries: rows.len(),
        draft_entries,
        included_options,
        omitted_entries,
        total_options,
        next_offset: (request.offset.saturating_add(limit) < total_options)
            .then_some(request.offset.saturating_add(limit)),
        options: options
            .into_iter()
            .skip(request.offset)
            .take(limit)
            .collect(),
    })
}

fn option_presentation(
    dimension: TrafficDimension,
    value: &TrafficFilterValue,
) -> (String, String) {
    match value {
        TrafficFilterValue::HostSubdomains(host) => {
            (format!("host-subdomains:{host}"), format!("*.{host}"))
        }
        TrafficFilterValue::Unavailable => ("unavailable".into(), "Unavailable".into()),
        TrafficFilterValue::Other => ("other".into(), "Other".into()),
        TrafficFilterValue::Number(number) => (format!("number:{number}"), number.clone()),
        TrafficFilterValue::StatusFamily(family) => {
            (format!("status-family:{family}"), format!("{family}xx"))
        }
        TrafficFilterValue::Text(text) => {
            let label = match dimension {
                TrafficDimension::Column(TrafficColumn::Method | TrafficColumn::Protocol) => {
                    text.to_uppercase()
                }
                TrafficDimension::Facet(TrafficFacet::TrafficType) => match text.as_str() {
                    "websocket" => "WebSocket".into(),
                    "sse" => "SSE".into(),
                    _ => "HTTP".into(),
                },
                _ => text.clone(),
            };
            (format!("text:{text}"), label)
        }
    }
}

/// Scope for contextual counts, excluding the edited dimension.
pub(crate) fn unrestricted_query(request: &TrafficFacetInput) -> SessionQueryInput {
    let mut query = request.query.clone();
    query
        .filter_groups
        .retain(|group| group.dimension != request.dimension);
    if let TrafficDimension::Column(column) = request.dimension {
        query.filters.retain(|filter| filter.column != column);
        match column {
            TrafficColumn::Method => query.method = None,
            TrafficColumn::Host => query.host = None,
            TrafficColumn::State => query.terminal = None,
            _ => {}
        }
    }
    if request.dimension == TrafficDimension::Facet(TrafficFacet::FetchDestination) {
        query.fetch_destinations.clear();
    }
    // Offset, focus, sorting and latest are presentation details, never count restrictions.
    query.offset = None;
    query.focus_id = None;
    query.sort = None;
    query
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildcard_suffixes_require_whole_valid_labels_in_retained_hosts() {
        for host in ["cdn.domain.example", "eu.cdn.domain.example"] {
            assert!(below(host, "domain.example"));
        }
        for host in [
            "domain.example",
            "otherdomain.example",
            ".domain.example",
            "bad..domain.example",
            "-bad.domain.example",
            "bad_.domain.example",
        ] {
            assert!(!below(host, "domain.example"), "{host}");
        }
    }
    #[test]
    fn hosts_are_canonical_and_wildcards_are_typed_and_bounded_in_both_roles() {
        let host = TrafficDimension::Column(TrafficColumn::Host);
        assert_eq!(
            canonical_host("BÜCHER.example.").unwrap(),
            "xn--bcher-kva.example"
        );
        assert_eq!(canonical_host("2001:db8::1").unwrap(), "[2001:db8::1]");
        assert_eq!(canonical_host("127.0.0.1").unwrap(), "127.0.0.1");
        for value in [
            "a.test:80",
            "https://a.test",
            "a.test/path",
            "a@b.test",
            "a.*.test",
            "*",
            "a..test",
            "-a.test",
            "a.test%2e",
            "[::1]:80",
        ] {
            assert!(canonical_host(value).is_err(), "{value}");
        }
        for value in [
            TrafficFilterValue::HostSubdomains("127.0.0.1".into()),
            TrafficFilterValue::HostSubdomains("[::1]".into()),
        ] {
            assert!(prepare_value(host, &value).is_err());
        }
        let mut group = TrafficFilterGroup {
            dimension: host,
            values: vec![],
            saved_values: vec![TrafficFilterValue::Text("EXAMPLE.TEST.".into()); 129],
            selection_mode: TrafficSelectionMode::IgnoreSaved,
        };
        assert!(prepare_groups(&[group.clone()]).is_ok());
        group
            .saved_values
            .push(TrafficFilterValue::Text("bad:443".into()));
        assert!(
            prepare_groups(&[group.clone()]).is_err(),
            "Paused input is still validated"
        );
        group.saved_values = vec![TrafficFilterValue::HostSubdomains("example.test".into())];
        group.values = group.saved_values.clone();
        assert!(
            prepare_groups(&[group.clone()]).is_err(),
            "Observed alternatives must be exact"
        );
        group.values.clear();
        group.dimension = TrafficDimension::Column(TrafficColumn::Process);
        assert!(prepare_groups(&[group]).is_err());
    }
}
