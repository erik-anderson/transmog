use crate::{BodyAccess, Direction, Error, HeaderTest, Inputs, MatchRule, Visualizer, invalid};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One retained header occurrence. Redaction never turns into an empty observed value.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    /// Stable occurrence index in the original retained block.
    pub index: u32,
    /// Original field name spelling.
    pub name: String,
    /// Exact retained value; absent when redacted or unavailable.
    pub value: Option<Vec<u8>>,
    /// Whether this occurrence was deliberately redacted.
    pub redacted: bool,
    /// Original measured length, when recorded.
    pub original_bytes: Option<String>,
}

/// Metadata borrowed from retention; constructing a matcher does not read bodies.
#[derive(Clone, Copy, Debug)]
pub struct MessageMetadata<'a> {
    /// Request or response metadata of this message.
    pub direction: Direction,
    /// Ordered retained headers.
    pub headers: &'a [Header],
    /// Request method, including for matching a paired response.
    pub method: Option<&'a str>,
    /// Endpoint path without query.
    pub path: Option<&'a str>,
    /// Observed response status, when present.
    pub status: Option<u16>,
}

/// Paired message metadata for dependency-aware matching.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExchangeMetadata<'a> {
    /// Retained request head, independently available from its body.
    pub request: Option<MessageMetadata<'a>>,
    /// Retained response head, or absent until one arrives.
    pub response: Option<MessageMetadata<'a>>,
}
impl<'a> ExchangeMetadata<'a> {
    /// Metadata of one side, without substituting another boundary.
    pub fn side(&self, side: Direction) -> Option<MessageMetadata<'a>> {
        match side {
            Direction::Request => self.request,
            Direction::Response => self.response,
        }
    }
}

/// Native matching of one conjunction, with redaction-safe header semantics.
pub fn matches(rule: &MatchRule, exchange: ExchangeMetadata<'_>, direction: Direction) -> bool {
    if rule.direction != direction {
        return false;
    }
    let Some(message) = exchange.side(direction) else {
        return false;
    };
    if !rule.content_types.is_empty() {
        let mut content_type: Option<String> = None;
        for field in message
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
        {
            let Some(value) = field
                .value
                .as_ref()
                .filter(|_| !field.redacted)
                .and_then(|v| std::str::from_utf8(v).ok())
            else {
                return false;
            };
            let media = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if content_type.as_ref().is_some_and(|prior| prior != &media) {
                return false;
            }
            content_type = Some(media);
        }
        if !content_type.is_some_and(|t| {
            rule.content_types
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&t))
        }) {
            return false;
        }
    }
    let request = exchange.request;
    if !rule.methods.is_empty()
        && !request
            .and_then(|r| r.method)
            .is_some_and(|m| rule.methods.iter().any(|allowed| allowed == m))
    {
        return false;
    }
    if !rule.path_prefixes.is_empty()
        && !request.and_then(|r| r.path).is_some_and(|p| {
            rule.path_prefixes
                .iter()
                .any(|prefix| p.starts_with(prefix))
        })
    {
        return false;
    }
    if !rule.statuses.is_empty()
        && !exchange
            .response
            .and_then(|r| r.status)
            .is_some_and(|s| rule.statuses.contains(&s))
    {
        return false;
    }
    rule.headers.iter().all(|predicate| {
        let Some(message) = exchange.side(predicate.direction) else {
            return false;
        };
        let fields: Vec<_> = message
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(&predicate.name))
            .collect();
        match &predicate.test {
            HeaderTest::Present => !fields.is_empty(),
            HeaderTest::Equals { value } => fields
                .iter()
                .any(|h| !h.redacted && h.value.as_deref() == Some(value.as_bytes())),
            HeaderTest::NotEquals { value } => {
                !fields.is_empty()
                    && fields.iter().all(|h| {
                        !h.redacted && h.value.as_ref().is_some_and(|v| v != value.as_bytes())
                    })
            }
        }
    })
}

/// Copies only explicitly granted fields before IPC, preserving duplicates and order.
/// # Errors
/// Rejects a header block exceeding the finite input budget.
pub fn project_headers(
    inputs: &Inputs,
    direction: Direction,
    fields: &[Header],
) -> Result<Vec<Header>, Error> {
    let grant = inputs.side(direction);
    let mut bytes = 0_usize;
    let mut result = Vec::new();
    for field in fields.iter().filter(|h| grant.permits_header(&h.name)) {
        bytes = bytes
            .checked_add(field.name.len())
            .and_then(|n| n.checked_add(field.value.as_ref().map_or(0, Vec::len)))
            .ok_or(Error::Limit)?;
        if bytes > 256 * 1024 || result.len() >= 512 {
            return Err(Error::Limit);
        }
        result.push(Header {
            index: field.index,
            name: field.name.clone(),
            value: if field.redacted {
                None
            } else {
                field.value.clone()
            },
            redacted: field.redacted,
            original_bytes: field.original_bytes.clone(),
        });
    }
    Ok(result)
}

/// Checks a body read independently from the header grant.
/// # Errors
/// Returns access denied for undeclared bodies.
pub fn check_body_access(inputs: &Inputs, direction: Direction) -> Result<(), Error> {
    if inputs.side(direction).body == BodyAccess::None {
        return Err(Error::AccessDenied);
    }
    Ok(())
}

/// Exact host-assigned scope; extensions cannot manufacture or widen one.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Scope {
    /// Capture/import namespace identity, never only a source filename.
    pub capture: String,
    /// Host-verified conversation identity, not inferred solely from a host name.
    pub conversation: String,
}

struct Subscription {
    scope: Scope,
    visualizer: Visualizer,
}

/// Native event index containing active opt-in subscriptions only.
#[derive(Default)]
pub struct ActiveSubscriptions {
    active: BTreeMap<String, Subscription>,
    index: BTreeMap<(Scope, Direction, String), BTreeSet<String>>,
}
impl ActiveSubscriptions {
    /// Adds an explicitly activated live instance; all predicates are validated first.
    /// # Errors
    /// Rejects duplicate identities, undeclared live support and capacity exhaustion.
    pub fn subscribe(
        &mut self,
        instance: String,
        scope: Scope,
        visualizer: Visualizer,
    ) -> Result<(), Error> {
        visualizer.validate()?;
        let live = visualizer
            .live_updates
            .as_ref()
            .ok_or(Error::AccessDenied)?;
        if self.active.len() >= 16 {
            return Err(Error::Limit);
        }
        if instance.is_empty() || instance.len() > 128 || self.active.contains_key(&instance) {
            return Err(invalid("Duplicate or invalid visualization instance"));
        }
        let rules = if visualizer.input_matches.is_empty() {
            &visualizer.matches
        } else {
            &visualizer.input_matches
        };
        for rule in rules.iter().filter(|r| live.events.contains(&r.direction)) {
            let keys = if !rule.content_types.is_empty() {
                rule.content_types
                    .iter()
                    .map(|t| format!("type:{}", t.to_ascii_lowercase()))
                    .collect()
            } else if let Some(header) = rule.headers.first() {
                vec![format!(
                    "header:{:?}:{}",
                    header.direction,
                    header.name.to_ascii_lowercase()
                )]
            } else {
                vec![String::new()]
            };
            for key in keys {
                self.index
                    .entry((scope.clone(), rule.direction, key))
                    .or_default()
                    .insert(instance.clone());
            }
        }
        self.active
            .insert(instance, Subscription { scope, visualizer });
        Ok(())
    }

    /// Removes admission before the owner cancels work and terminates its worker.
    pub fn unsubscribe(&mut self, instance: &str) {
        if self.active.remove(instance).is_none() {
            return;
        }
        self.index.retain(|_, ids| {
            ids.remove(instance);
            !ids.is_empty()
        });
    }

    /// Selects active candidates using native metadata before any IPC or body read.
    pub fn admitted(
        &self,
        scope: &Scope,
        exchange: ExchangeMetadata<'_>,
        direction: Direction,
    ) -> Vec<&str> {
        if self.active.is_empty() {
            return vec![];
        }
        let mut keys = BTreeSet::from([String::new()]);
        for side in [Direction::Request, Direction::Response] {
            if let Some(message) = exchange.side(side) {
                for field in message.headers {
                    keys.insert(format!(
                        "header:{side:?}:{}",
                        field.name.to_ascii_lowercase()
                    ));
                    if side == direction
                        && field.name.eq_ignore_ascii_case("content-type")
                        && !field.redacted
                        && let Some(value) = field
                            .value
                            .as_ref()
                            .and_then(|v| std::str::from_utf8(v).ok())
                    {
                        keys.insert(format!(
                            "type:{}",
                            value
                                .split(';')
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_ascii_lowercase()
                        ));
                    }
                }
            }
        }
        let mut candidates = BTreeSet::new();
        for key in keys {
            if let Some(ids) = self.index.get(&(scope.clone(), direction, key)) {
                candidates.extend(ids);
            }
        }
        candidates
            .into_iter()
            .filter_map(|id| {
                let subscription = self.active.get(id)?;
                if &subscription.scope != scope {
                    return None;
                }
                let rules = if subscription.visualizer.input_matches.is_empty() {
                    &subscription.visualizer.matches
                } else {
                    &subscription.visualizer.input_matches
                };
                rules
                    .iter()
                    .any(|r| matches(r, exchange, direction))
                    .then_some(id.as_str())
            })
            .collect()
    }

    /// Number of active opt-in feeds, independent of installed package count.
    pub fn len(&self) -> usize {
        self.active.len()
    }
    /// Whether the dispatch path has no extension work.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}
