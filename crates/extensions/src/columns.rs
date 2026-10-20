//! Static, header-only traffic classifications. No module or body capability is involved.
use crate::{BodyAccess, Error, Inputs, MatchRule, invalid, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One literal label and its alternative native header conjunctions.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationValue {
    /// Fixed display text; captured header values cannot be interpolated.
    pub value: String,
    /// OR rules with AND conditions; request/response dependencies may be combined.
    pub matches: Vec<MatchRule>,
}

/// An optional package-qualified traffic column, evaluated only from retained headers.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationColumn {
    /// Stable identity within the extension, independent of visualizer identities.
    pub id: String,
    /// Inert column heading.
    pub name: String,
    /// Exact request and response header names; both body grants must be none.
    pub inputs: Inputs,
    /// Fixed labels, deduplicated in declaration order when several match.
    pub values: Vec<ClassificationValue>,
}
impl ClassificationColumn {
    /// Validates a bounded header-only declaration with no executable entry.
    /// # Errors
    /// Rejects body access, non-header predicates, duplicate labels and invalid grants.
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_id(&self.id)
            || !crate::manifest::label(&self.name)
            || self.values.is_empty()
            || self.values.len() > 32
            || [&self.inputs.request, &self.inputs.response]
                .iter()
                .any(|side| side.body != BodyAccess::None || side.decode_http_content)
        {
            return Err(invalid(
                "Declare bounded header-only classification columns",
            ));
        }
        let mut labels = BTreeSet::new();
        self.inputs.validate()?;
        for value in &self.values {
            if !crate::manifest::label(&value.value)
                || !labels.insert(&value.value)
                || value.matches.is_empty()
                || value.matches.len() > 32
                || value.matches.iter().any(|rule| {
                    !rule.methods.is_empty()
                        || !rule.path_prefixes.is_empty()
                        || !rule.statuses.is_empty()
                })
            {
                return Err(invalid(
                    "Classification values must be unique literals matched only by headers",
                ));
            }
            for rule in &value.matches {
                crate::manifest::validate_rule(&self.inputs, rule)?;
            }
        }
        Ok(())
    }
}

/// Stable identity for a column in settings and table preferences.
pub fn classification_key(extension: &str, column: &str) -> String {
    format!("{extension}:{column}")
}

/// Whether an opaque column identity consists of two valid, bounded identifiers.
pub fn valid_classification_key(key: &str) -> bool {
    key.split_once(':')
        .is_some_and(|(extension, column)| valid_id(extension) && valid_id(column))
}
