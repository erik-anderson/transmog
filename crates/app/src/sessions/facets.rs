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
}

/// OR alternatives in one dimension; distinct dimensions are combined with AND.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficFilterGroup {
    /// Metadata identity, independently of visible columns.
    pub dimension: TrafficDimension,
    /// At most 128 alternatives. Empty means unrestricted.
    pub values: Vec<TrafficFilterValue>,
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
) -> Result<Vec<TrafficFilterGroup>, AppError> {
    if groups.len() > 32 || groups.iter().map(|group| group.values.len()).sum::<usize>() > 1024 {
        return Err(invalid(
            "Traffic filters accept at most 32 dimensions and 1024 alternatives",
        ));
    }
    let mut dimensions = BTreeSet::new();
    groups
        .iter()
        .map(|group| {
            if !dimensions.insert(group.dimension) || group.values.len() > 128 {
                return Err(invalid(
                    "Use one group per dimension with at most 128 alternatives",
                ));
            }
            let values = group
                .values
                .iter()
                .map(|value| prepare_value(group.dimension, value))
                .collect::<Result<BTreeSet<_>, _>>()?;
            Ok(TrafficFilterGroup {
                dimension: group.dimension,
                values: values.into_iter().collect(),
            })
        })
        .collect()
}

fn is_numeric(dimension: TrafficDimension) -> bool {
    match dimension {
        TrafficDimension::Column(column) => numeric_column(column),
        TrafficDimension::Facet(_) => false,
    }
}

fn prepare_value(
    dimension: TrafficDimension,
    value: &TrafficFilterValue,
) -> Result<TrafficFilterValue, AppError> {
    match value {
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

impl TrafficFilterGroup {
    pub(super) fn matches(&self, row: &SessionSummary) -> bool {
        let candidate = value_for(row, self.dimension);
        self.values.is_empty()
            || self.values.iter().any(|value| match value {
                TrafficFilterValue::StatusFamily(family) => {
                    row.status.is_some_and(|status| status / 100 == *family)
                }
                TrafficFilterValue::Other => is_other(self.dimension, &candidate),
                value => *value == candidate,
            })
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
            let count = counts.get(&value).copied().unwrap_or(0);
            TrafficFacetOption {
                key,
                label,
                value,
                count,
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

pub(crate) fn facet_counts(
    service: &ApplicationSessionService,
    request: &TrafficFacetInput,
    traces: &crate::traces::TraceRegistry,
    matching: Option<&HashSet<String>>,
) -> Result<TrafficFacetPage, AppError> {
    validate_filter(request.search.as_deref())?;
    // Validate even constraints about to be excluded; malformed requests never succeed silently.
    prepare_groups(&request.query.filter_groups)?;
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
    let (rows, _) = ordered_rows(service, &query, traces, matching)?;
    let mut counts = BTreeMap::<TrafficFilterValue, usize>::new();
    let mut omitted_entries = 0;
    for row in &rows {
        let value = value_for(row, request.dimension);
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
                TrafficFilterValue::Text(text) | TrafficFilterValue::Number(text) => text.clone(),
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
                value,
                count,
            }
        })
        .collect();
    let total_options = options.len();
    Ok(TrafficFacetPage {
        dimension: request.dimension,
        matching_entries: rows.len(),
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
