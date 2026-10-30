//! Script results, parameters and body admission.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ScriptAction, ScriptHeaderOperation};

/// A complete decision and bounded evidence produced by one invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptResult {
    /// The single traffic decision. Evidence and state do not imply a traffic change.
    pub decision: ScriptAction,
    /// Operator-visible findings saved with the exchange.
    #[serde(default)]
    pub annotations: Vec<ScriptAnnotation>,
    /// Ephemeral diagnostic messages, visible in script tests and live evidence.
    #[serde(default)]
    pub diagnostics: Vec<String>,
    /// Replace this script's exchange-local JSON state; omitted means retain it.
    #[serde(default)]
    pub state: Option<BTreeMap<String, Value>>,
}

impl From<ScriptAction> for ScriptResult {
    fn from(decision: ScriptAction) -> Self {
        Self {
            decision,
            annotations: Vec::new(),
            diagnostics: Vec::new(),
            state: None,
        }
    }
}

pub use transmog_core::intercept::{ScriptAnnotation, ScriptSeverity};

/// Related message edits validated as a unit before application.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptEdits {
    /// Ordered operations on the current message's headers.
    #[serde(default)]
    pub headers: Vec<ScriptHeaderOperation>,
    /// Request method; request-head phase only.
    pub method: Option<String>,
    /// Absolute request URL; request-head phase only, with explicit rerouting authority.
    pub url: Option<String>,
    /// Response status; response phases only.
    pub status: Option<u16>,
    /// Complete decoded replacement bytes; body phases only.
    pub bytes: Option<Vec<u8>>,
}

/// Inline local response, constructed without external I/O.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptInlineResponse {
    /// Final response status.
    pub status: u16,
    /// Ordered fields. Managed framing headers are supplied by the engine.
    pub headers: Vec<(String, String)>,
    /// Complete identity body.
    pub bytes: Vec<u8>,
}

/// A declared script parameter and its saved value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ScriptParameter {
    /// Text or a finite selection of text values.
    String {
        /// Human-readable purpose.
        label: String,
        /// Default authoring value.
        default: String,
        /// Saved override, omitted to use the default.
        value: Option<String>,
        /// Allowed choices; empty permits arbitrary bounded text.
        #[serde(default)]
        choices: Vec<String>,
    },
    /// A boolean switch.
    Boolean {
        /// Human-readable purpose.
        label: String,
        /// Default authoring value.
        default: bool,
        /// Saved override.
        value: Option<bool>,
    },
    /// A bounded integer.
    Integer {
        /// Human-readable purpose.
        label: String,
        /// Default authoring value.
        default: i64,
        /// Saved override.
        value: Option<i64>,
        /// Inclusive minimum.
        min: i64,
        /// Inclusive maximum.
        max: i64,
    },
}

impl ScriptParameter {
    pub(crate) fn resolved(&self) -> Value {
        match self {
            Self::String { default, value, .. } => {
                Value::String(value.as_ref().unwrap_or(default).clone())
            }
            Self::Boolean { default, value, .. } => Value::Bool(value.unwrap_or(*default)),
            Self::Integer { default, value, .. } => Value::from(value.unwrap_or(*default)),
        }
    }

    pub(crate) fn valid(&self) -> bool {
        let label = match self {
            Self::String {
                label,
                default,
                value,
                choices,
            } => {
                if choices.len() > 64
                    || choices.iter().any(|v| v.len() > 4096)
                    || [Some(default), value.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|v| v.len() > 4096 || (!choices.is_empty() && !choices.contains(v)))
                {
                    return false;
                }
                label
            }
            Self::Boolean { label, .. } => label,
            Self::Integer {
                label,
                default,
                value,
                min,
                max,
            } => {
                if min > max
                    || *min < -9_007_199_254_740_991
                    || *max > 9_007_199_254_740_991
                    || !(min..=max).contains(&default)
                    || value.is_some_and(|v| v < *min || v > *max)
                {
                    return false;
                }
                label
            }
        };
        !label.is_empty() && label.len() <= 128 && !label.chars().any(char::is_control)
    }
}

/// Native admission predicates evaluated before a body is decoded or buffered.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptBodyFilter {
    /// Exact media types without parameters, compared case-insensitively.
    #[serde(default)]
    pub content_types: Vec<String>,
    /// Exact response statuses; empty permits all statuses.
    #[serde(default)]
    pub statuses: Vec<u16>,
    /// Include locally generated responses.
    #[serde(default)]
    pub include_local: bool,
}
