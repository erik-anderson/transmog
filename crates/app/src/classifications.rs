//! Bounded visible-row classifications from cached declarations and borrowed client headers.
use crate::{AppError, Application, ErrorCategory};
use serde::Serialize;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};
use transmog_core::{HeaderField, observe::ExchangeBoundary};
use transmog_extensions::{
    ClassificationColumn, Direction, ExchangeMetadata, HeaderMetadata, MessageMetadata, matches,
};
#[cfg(test)]
mod tests;

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
        let exchanges = ids
            .iter()
            .map(|id| crate::response_assets::parse_exchange_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        let declarations = self.extensions().classification_columns(columns);
        if declarations.is_empty() {
            return Ok(Vec::new());
        }
        let mut names = [BTreeSet::new(), BTreeSet::new()];
        for (_, column) in &declarations {
            for (index, access) in [&column.inputs.request, &column.inputs.response]
                .into_iter()
                .enumerate()
            {
                names[index].extend(access.headers.iter().map(|name| name.to_ascii_lowercase()));
            }
        }
        let mut budget = Budget::default();
        Ok(ids
            .iter()
            .zip(exchanges)
            .filter_map(|(id, exchange)| {
                self.service.catalog().with_heads(exchange, |heads| {
                    let request = heads
                        .request_heads
                        .iter()
                        .find(|head| head.boundary == ExchangeBoundary::ClientRequest)
                        .map(|head| &head.head.headers);
                    let response = heads
                        .response_heads
                        .iter()
                        .rev()
                        .find(|head| {
                            head.boundary == ExchangeBoundary::ClientResponse
                                && (head.head.status >= 200 || head.head.status == 101)
                        })
                        .map(|head| &head.head.headers);
                    classify(
                        id,
                        request.map(transmog_core::HeaderBlock::fields),
                        response.map(transmog_core::HeaderBlock::fields),
                        &declarations,
                        &names,
                        &mut budget,
                    )
                })
            })
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
