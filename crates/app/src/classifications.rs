//! Bounded visible-row classifications from cached declarations and borrowed client headers.
use crate::{AppError, Application, ErrorCategory};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::Arc,
};
use transmog_core::{HeaderField, observe::ExchangeBoundary};
use transmog_extensions::{
    ClassificationColumn, Direction, ExchangeMetadata, HeaderMetadata, MessageMetadata, matches,
};
#[cfg(test)]
mod tests;

/// Exact fixed-label alternatives for one extension column (OR within, AND across columns).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationFilter {
    /// Package-qualified column identity.
    pub column: String,
    /// Text labels or Unavailable for no matching label. Limited evidence never matches either.
    pub values: Vec<crate::TrafficFilterValue>,
}

/// Counts for an extension column, excluding that column's own filter.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationFacetInput {
    /// Complete traffic scope.
    pub query: crate::SessionQueryInput,
    /// Package-qualified enabled column identity.
    pub column: String,
}

/// Bounded declared alternatives and contextual entry counts, without body access.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationFacetPage {
    /// One count per distinct declared label, plus no-match.
    pub options: Vec<crate::TrafficFacetOption>,
    /// Candidate entries with this column unrestricted.
    pub matching_entries: usize,
    /// Entries that could not be classified within metadata limits.
    pub omitted_entries: usize,
}

struct PreparedColumns {
    columns: Vec<(String, ClassificationColumn)>,
    names: [BTreeSet<String>; 2],
}

impl PreparedColumns {
    fn new(columns: Vec<(String, ClassificationColumn)>) -> Self {
        let mut names = [BTreeSet::new(), BTreeSet::new()];
        for (_, column) in &columns {
            for (index, access) in [&column.inputs.request, &column.inputs.response]
                .into_iter()
                .enumerate()
            {
                names[index].extend(access.headers.iter().map(|name| name.to_ascii_lowercase()));
            }
        }
        Self { columns, names }
    }

    fn row(
        &self,
        app: &Application,
        id: &str,
        budget: &mut Budget,
    ) -> Option<TrafficClassification> {
        let exchange = crate::response_assets::parse_exchange_id(id).ok()?;
        app.service.catalog().with_heads(exchange, |heads| {
            let request = heads
                .request_heads
                .iter()
                .find(|head| head.boundary == ExchangeBoundary::ClientRequest)
                .map(|head| head.head.headers.fields());
            let response = heads
                .response_heads
                .iter()
                .rev()
                .find(|head| {
                    head.boundary == ExchangeBoundary::ClientResponse
                        && (head.head.status >= 200 || head.head.status == 101)
                })
                .map(|head| head.head.headers.fields());
            classify(id, request, response, &self.columns, &self.names, budget)
        })
    }
}

fn unavailable_column() -> AppError {
    AppError::new(
        ErrorCategory::Unavailable,
        "An extension column is unavailable. Remove its filter or enable the extension in Settings.",
        false,
    )
}

impl ClassificationFilter {
    fn matches(&self, row: &TrafficClassification) -> bool {
        if row.limited {
            return false;
        }
        let labels = row.values.get(&self.column).map_or(&[][..], Vec::as_slice);
        self.values.is_empty()
            || self.values.iter().any(|value| match value {
                crate::TrafficFilterValue::Text(text) => labels
                    .iter()
                    .any(|label| label.to_lowercase() == text.to_lowercase()),
                crate::TrafficFilterValue::Unavailable => labels.is_empty(),
                _ => false,
            })
    }
}

/// Header-derived labels for one retained row. Values always originate in the manifest.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficClassification {
    /// Retained exchange identity.
    pub id: String,
    /// Package-qualified column identities to fixed matching labels.
    pub values: BTreeMap<String, Vec<String>>,
    /// This row could not be evaluated within the bounded metadata/work budget.
    pub limited: bool,
}
fn invalid() -> AppError {
    AppError::new(
        ErrorCategory::InvalidInput,
        "Choose up to 8 extension columns and between 1 and 512 retained rows",
        false,
    )
}
struct Field<'a> {
    field: &'a HeaderField,
    work: &'a Cell<usize>,
}
impl HeaderMetadata for Field<'_> {
    fn name(&self) -> &[u8] {
        let name = self.field.name();
        if let Some(remaining) = self.work.get().checked_sub(name.len().max(1)) {
            self.work.set(remaining);
            name
        } else {
            self.work.set(0);
            b""
        }
    }
    fn value(&self) -> Option<&[u8]> {
        if self.field.is_redacted() {
            return None;
        }
        let value = self.field.value();
        if let Some(remaining) = self.work.get().checked_sub(value.len().max(1)) {
            self.work.set(remaining);
            Some(value)
        } else {
            self.work.set(0);
            None
        }
    }
    fn redacted(&self) -> bool {
        self.field.is_redacted()
    }
}
struct Budget {
    bytes: usize,
    output: usize,
    work: Cell<usize>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: 4 * 1024 * 1024,
            output: 4 * 1024 * 1024,
            work: Cell::new(262_144),
        }
    }
}
impl Budget {
    // Whole-catalog work is opt-in and bounded independently of viewport batches.
    fn scan() -> Self {
        Self {
            bytes: 64 * 1024 * 1024,
            output: 16 * 1024 * 1024,
            work: Cell::new(8 * 1024 * 1024),
        }
    }
    fn fields<'a>(
        &mut self,
        fields: &'a [HeaderField],
        names: &BTreeSet<String>,
    ) -> Option<Vec<&'a HeaderField>> {
        let mut result = Vec::new();
        if fields.len() > 4096 {
            return None;
        }
        for field in fields {
            self.work.set(self.work.get().checked_sub(1)?);
            self.bytes = self.bytes.checked_sub(field.name().len())?;
            if names.contains(&String::from_utf8_lossy(field.name()).to_ascii_lowercase()) {
                self.bytes = self.bytes.checked_sub(field.value().len())?;
                result.push(field);
            }
        }
        Some(result)
    }
}
impl Application {
    fn prepare_classification_filters(
        &self,
        filters: &[ClassificationFilter],
    ) -> Result<PreparedColumns, AppError> {
        if filters.len() > 8
            || filters
                .iter()
                .map(|filter| &filter.column)
                .collect::<BTreeSet<_>>()
                .len()
                != filters.len()
            || filters.iter().any(|filter| {
                !transmog_extensions::valid_classification_key(&filter.column)
                    || filter.values.len() > 128
                    || filter.values.iter().any(|value| match value {
                        crate::TrafficFilterValue::Text(text) => {
                            text.is_empty()
                                || text.chars().count() > 255
                                || text.chars().any(char::is_control)
                        }
                        crate::TrafficFilterValue::Unavailable => false,
                        _ => true,
                    })
            })
        {
            return Err(AppError::new(
                ErrorCategory::InvalidInput,
                "Choose up to 8 extension column filters with bounded exact labels",
                false,
            ));
        }
        let keys = filters
            .iter()
            .map(|filter| filter.column.clone())
            .collect::<Vec<_>>();
        let columns = self.extensions().classification_columns(&keys);
        if columns.len() != keys.len() {
            return Err(unavailable_column());
        }
        Ok(PreparedColumns::new(columns))
    }

    /// Adds only explicitly requested header predicates to an existing metadata scope.
    pub(crate) fn restrict_classifications(
        &self,
        query: &crate::SessionQueryInput,
        matching: Option<Arc<HashSet<String>>>,
    ) -> Result<Option<Arc<HashSet<String>>>, AppError> {
        if query.extension_filters.is_empty() {
            return Ok(matching);
        }
        let prepared = self.prepare_classification_filters(&query.extension_filters)?;
        let mut base = query.clone();
        base.extension_filters.clear();
        base.cursor = None;
        base.sort = None;
        base.offset = None;
        base.focus_id = None;
        let ids =
            crate::sessions::matching_ids(&self.service, &base, &self.traces, matching.as_deref())?;
        let mut budget = Budget::scan();
        let mut selected = HashSet::new();
        for id in ids {
            let Some(row) = prepared.row(self, &id, &mut budget) else {
                continue;
            };
            if row.limited {
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "Extension filtering exceeded its header metadata limit. Narrow Traffic with other filters first.",
                    false,
                ));
            }
            if query
                .extension_filters
                .iter()
                .all(|filter| filter.matches(&row))
            {
                selected.insert(id);
            }
        }
        Ok(Some(Arc::new(selected)))
    }

    /// Counts fixed labels across retained headers; never opens body sources or extension code.
    /// # Errors
    /// Returns invalid filters, unavailable declarations or expired traffic scope.
    pub fn extension_traffic_facets(
        &self,
        request: &ClassificationFacetInput,
    ) -> Result<ClassificationFacetPage, AppError> {
        let prepared = self.prepare_classification_filters(&[ClassificationFilter {
            column: request.column.clone(),
            values: Vec::new(),
        }])?;
        // Validate constraints even when the edited dimension is excluded.
        self.prepare_classification_filters(&request.query.extension_filters)?;
        let mut query = request.query.clone();
        query
            .extension_filters
            .retain(|filter| filter.column != request.column);
        query.offset = None;
        query.focus_id = None;
        query.sort = None;
        let ids = self.matching_traffic_ids(&query)?;
        let mut counts = BTreeMap::<String, (String, usize)>::new();
        for value in &prepared.columns[0].1.values {
            counts
                .entry(value.value.to_lowercase())
                .or_insert((value.value.clone(), 0));
        }
        let mut unmatched = 0;
        let mut omitted_entries = 0;
        let mut budget = Budget::scan();
        for id in &ids {
            let Some(row) = prepared.row(self, id, &mut budget) else {
                omitted_entries += 1;
                continue;
            };
            if row.limited {
                omitted_entries += 1;
                continue;
            }
            let labels = &row.values[&request.column];
            if labels.is_empty() {
                unmatched += 1;
            }
            for key in labels
                .iter()
                .map(|value| value.to_lowercase())
                .collect::<BTreeSet<_>>()
            {
                if let Some((_, count)) = counts.get_mut(&key) {
                    *count += 1;
                }
            }
        }
        let mut options = counts
            .into_iter()
            .map(|(key, (label, count))| crate::TrafficFacetOption {
                key: format!("text:{key}"),
                label,
                value: crate::TrafficFilterValue::Text(key),
                count,
            })
            .collect::<Vec<_>>();
        options.push(crate::TrafficFacetOption {
            key: "unavailable".into(),
            label: "No match".into(),
            value: crate::TrafficFilterValue::Unavailable,
            count: unmatched,
        });
        Ok(ClassificationFacetPage {
            options,
            matching_entries: ids.len(),
            omitted_entries,
        })
    }
    /// Lists enabled static traffic column headings; never reads a capture or runs code.
    pub fn extension_traffic_columns(
        &self,
    ) -> Vec<transmog_extension_registry::ClassificationChoice> {
        self.extensions().classification_choices()
    }
    /// Evaluates selected columns for a bounded retained metadata window, with no body access.
    /// # Errors
    /// Rejects excessive, duplicate or invalid row/column identities.
    pub fn classify_traffic_rows(
        &self,
        ids: &[String],
        columns: &[String],
    ) -> Result<Vec<TrafficClassification>, AppError> {
        if ids.is_empty()
            || ids.len() > 512
            || columns.len() > 8
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
            || columns.iter().collect::<BTreeSet<_>>().len() != columns.len()
            || columns
                .iter()
                .any(|key| !transmog_extensions::valid_classification_key(key))
        {
            return Err(invalid());
        }
        for id in ids {
            crate::response_assets::parse_exchange_id(id)?;
        }
        let prepared = PreparedColumns::new(self.extensions().classification_columns(columns));
        if prepared.columns.is_empty() {
            return Ok(Vec::new());
        }
        let mut budget = Budget::default();
        Ok(ids
            .iter()
            .filter_map(|id| prepared.row(self, id, &mut budget))
            .collect())
    }
}
fn wrap<'a>(fields: Vec<&'a HeaderField>, work: &'a Cell<usize>) -> Vec<Field<'a>> {
    fields
        .into_iter()
        .map(|field| Field { field, work })
        .collect()
}
fn classify(
    id: &str,
    request: Option<&[HeaderField]>,
    response: Option<&[HeaderField]>,
    columns: &[(String, ClassificationColumn)],
    names: &[BTreeSet<String>; 2],
    budget: &mut Budget,
) -> TrafficClassification {
    let mut result = TrafficClassification {
        id: id.into(),
        values: BTreeMap::new(),
        limited: false,
    };
    let Some(request_fields) = budget.fields(request.unwrap_or_default(), &names[0]) else {
        result.limited = true;
        return result;
    };
    let Some(response_fields) = budget.fields(response.unwrap_or_default(), &names[1]) else {
        result.limited = true;
        return result;
    };
    let requests = wrap(request_fields, &budget.work);
    let responses = wrap(response_fields, &budget.work);
    let message = |direction, headers| MessageMetadata {
        direction,
        headers,
        method: None,
        path: None,
        status: None,
    };
    let metadata = ExchangeMetadata {
        request: request.map(|_| message(Direction::Request, &requests)),
        response: response.map(|_| message(Direction::Response, &responses)),
    };
    for (key, column) in columns {
        let mut values = Vec::new();
        for value in &column.values {
            if value.matches.iter().any(|rule| {
                let remaining = budget.work.get();
                budget.work.set(remaining.saturating_sub(1));
                remaining > 0 && matches(rule, metadata, rule.direction)
            }) {
                if let Some(remaining) = budget.output.checked_sub(value.value.len()) {
                    budget.output = remaining;
                    values.push(value.value.clone());
                } else {
                    result.limited = true;
                }
            }
            if budget.work.get() == 0 {
                result.limited = true;
                break;
            }
        }
        result.values.insert(key.clone(), values);
        if result.limited {
            break;
        }
    }
    if result.limited {
        result.values.clear();
    }
    result
}
