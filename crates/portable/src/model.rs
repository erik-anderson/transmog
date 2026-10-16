//! Public JSON fields. These do not deserialize private workspace or native rule DTOs.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// The field-level revision-0 wire contract is documented in portable-investigations.md.
// Keep DTO names/fields explicit rather than exposing private persistence types.
#[allow(missing_docs)]
mod dto {
    use super::{Deserialize, Map, Serialize, Value};
    macro_rules! object {
        ($name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
            #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            pub struct $name { $($(#[$attr])* pub $field: $ty),* }
        };
    }
    object!(Producer {
        name: String,
        version: String
    });
    object!(Object {
        sha256: String,
        bytes: String,
        role: String,
        media_type: String
    });
    object!(Header {
        name: String,
        value_base64: String
    });
    object!(Dependency {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] bytes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] expected_sha256: Option<String>,
        representation: String
    });
    object!(EvidenceReference { artifact_sha256: String, source_key: String, exchange_id: Option<String>, occurrence: String, boundary: String });
    object!(Provenance {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] captured_source: Option<EvidenceReference>,
        #[serde(default, skip_serializing_if = "Option::is_none")] acquired_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] source_unavailable_reason: Option<String>
    });
    object!(Asset { id: String, revision: String, status: u16, headers: Vec<Header>, body: Dependency, provenance: Provenance });
    object!(Predicate {
        name: String, kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] value_base64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] pattern: Option<String>
    });
    object!(Example { method: String, url: String, headers: Vec<Header>, expected: String });
    object!(Matcher {
        #[serde(default)] method: Option<String>,
        #[serde(default)] address: Option<Address>,
        #[serde(default, skip_serializing_if = "Option::is_none")] scheme: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")] path_prefix: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")] query: Option<String>,
        #[serde(default)] request_headers: Vec<Predicate>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] response_headers: Vec<Predicate>,
        #[serde(default, skip_serializing_if = "Option::is_none")] response_status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")] response_status_class: Option<u16>,
        #[serde(default)] examples: Vec<Example>
    });
    object!(Parameter {
        name: String,
        value: String
    });
    object!(Pattern {
        address: String,
        query: Query,
        case_sensitive: bool
    });
    object!(Regex { pattern: String, scope: String, whole: bool, case_sensitive: bool, query: Option<Query> });
    #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(
        tag = "kind",
        content = "value",
        rename_all = "kebab-case",
        deny_unknown_fields
    )]
    pub enum Address {
        Exact(String),
        Pattern(Pattern),
        Regex(Regex),
    }
    #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(
        tag = "kind",
        content = "value",
        rename_all = "kebab-case",
        deny_unknown_fields
    )]
    pub enum Query {
        Exact(Option<String>),
        Parameters(Vec<Parameter>),
        Ignore,
    }
    object!(Rule {
        id: String, revision: String, label: String, kind: String, evaluation: String,
        enabled_intent: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")] priority: Option<i32>,
        #[serde(rename = "match")] matcher: Matcher,
        action: Value
    });
    object!(Revision { revision: String, rules: Vec<Rule> });
    object!(Variant { id: String, label: String, #[serde(default)] archived_intent: bool, revisions: Vec<Revision> });
    object!(ActiveSelection {
        variant_id: String,
        revision: String
    });
    object!(Experiment {
        id: String, kind: String, required_features: Vec<String>, label: String,
        paused_intent: bool, #[serde(default)] archived_intent: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")] active_intent: Option<ActiveSelection>,
        variants: Vec<Variant>
    });
    object!(Collection {
        format: String, schema_version: u32, kind: String, id: String, revision: String,
        title: String, created_at: String, producer: Producer, required_features: Vec<String>,
        objects: Vec<Object>, assets: Vec<Asset>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] rules: Vec<Rule>,
        experiments: Vec<Experiment>, extensions: Map<String, Value>
    });
    object!(AutoResponse {
        asset_id: String,
        revision: String
    });
    object!(Operation {
        operation: String, name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")] value_base64: Option<String>
    });
    object!(HeaderEdit { operations: Vec<Operation> });
    object!(Literal {
        find: String, replacement: String, occurrences: String,
        expected_matches: Option<String>, media_type: String, encoding: String, max_bytes: String
    });
}
pub use dto::*;
