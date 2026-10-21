//! Bounded, ephemeral relationships over captured headers/targets, never a traffic-wide body scan.
use super::{AppError, Application, AuthHandle, Header, Input, Inspection};
use serde::Serialize;
use std::sync::{Arc, atomic::AtomicBool};

/// Retained evidence navigation, without putting correlation values into labels or URLs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedEntry {
    /// Exact retained exchange in this owning application.
    pub session_id: String,
    /// Boundary containing the related auth evidence.
    pub boundary: String,
    /// Human-readable request number.
    pub label: String,
    /// Observed reason; connection context alone is not handshake proof.
    pub reason: String,
}
/// Search scope and limitations accompany every related result.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedAuth {
    /// At most one hundred authorized retained results.
    pub entries: Vec<RelatedEntry>,
    /// Retained exchange metadata entries considered.
    pub searched: usize,
    /// A finite result/scan/byte bound stopped this search.
    pub limited: bool,
    /// Missing metadata and source coverage, never raw correlation values.
    pub detail: String,
}
fn values(inspection: &Inspection, format: &str, key: &str) -> Vec<String> {
    fn collect(
        inspection: &Inspection,
        items: &[transmog_auth::Finding],
        format: &str,
        key: &str,
        result: &mut Vec<String>,
    ) {
        for item in items {
            if item.format.starts_with(format) {
                for field in &item.fields {
                    if (field.name == key || field.name.ends_with(&format!(" @{key}")))
                        && let Some(value) = inspection.value(&field.id)
                    {
                        result.push(value.into());
                    }
                }
            }
            collect(inspection, &item.children, format, key, result);
        }
    }
    let mut result = vec![];
    collect(inspection, &inspection.findings, format, key, &mut result);
    result
}
fn unique_equal(a: &Inspection, b: &Inspection, format: &str, key: &str) -> bool {
    let left = values(a, format, key);
    let right = values(b, format, key);
    left.len() == 1 && right.len() == 1 && !left[0].is_empty() && left == right
}
fn correlation(a: &Inspection, b: &Inspection) -> Option<&'static str> {
    if (unique_equal(a, b, "OAuth", "state") || unique_equal(a, b, "OAuth", "code"))
        && (unique_equal(a, b, "OAuth", "client_id") || unique_equal(a, b, "OAuth", "redirect_uri"))
    {
        return Some(
            "Observed OAuth correlation plus matching client/redirect context; not proof of a completed login.",
        );
    }
    let request = values(a, "SAML", "ID");
    let response = values(b, "SAML", "InResponseTo");
    let reverse_request = values(b, "SAML", "ID");
    let reverse_response = values(a, "SAML", "InResponseTo");
    if ((request.len() == 1 && response.len() == 1 && request == response)
        || (reverse_request.len() == 1
            && reverse_response.len() == 1
            && reverse_request == reverse_response))
        && (unique_equal(a, b, "SAML", "Issuer") || unique_equal(a, b, "SAML", "Destination"))
    {
        return Some(
            "Observed SAML ID/InResponseTo and matching issuer/destination context; signatures not checked.",
        );
    }
    None
}
impl Application {
    /// Find bounded, evidence-backed neighbors in the same capture and authority.
    /// # Errors
    /// Rejects obsolete inspection, changed retention and other-workspace capabilities.
    #[allow(clippy::too_many_lines)] // Keep the finite scan, scope admission and result bound together.
    pub fn related_auth(&self, handle: &AuthHandle) -> Result<RelatedAuth, AppError> {
        let active = self.authorized_auth(handle)?;
        let (anchor, _) = self.visualizer_evidence(&active.request.session_id)?;
        let namespace = self.traces.context_source_id(&active.request.session_id);
        let original = anchor.metadata.original_target.as_target();
        let absolute = |target: &transmog_core::Target| {
            url::Url::parse(&format!(
                "{}://{}{}{}",
                target.scheme,
                target.authority,
                target.path,
                target
                    .query
                    .as_ref()
                    .map_or_else(String::new, |q| format!("?{q}"))
            ))
            .ok()
        };
        let base = absolute(original);
        let redirects: Vec<_> = super::block(&anchor, &active.request.boundary)?
            .0
            .iter()
            .filter(|h| h.name_eq("location") && !h.is_redacted())
            .filter_map(|h| std::str::from_utf8(h.value()).ok())
            .filter_map(|value| base.as_ref()?.join(value).ok())
            .map(|mut url| {
                url.set_fragment(None);
                url
            })
            .collect();
        let anchor_connection = anchor
            .performance
            .transports
            .iter()
            .find(|t| t.leg == "client" && !t.connection_id.is_empty())
            .map(|t| t.connection_id.as_str());
        let connection_available = !anchor.imported || anchor_connection.is_some();
        let mut result = RelatedAuth {
            entries: vec![],
            searched: 0,
            limited: false,
            detail: format!(
                "Searched retained headers and targets only, in this capture and authority; bodies of other messages were not read. {} Connection reuse/multiplexing does not establish a single handshake or successful authentication.",
                if connection_available {
                    "Physical client-leg context is available."
                } else {
                    "Exact connection relationships are unavailable: imported client connection metadata is missing."
                }
            ),
        };
        let ids = self.service.catalog().ordered_ids(None, None, true, 512);
        result.limited = ids.len() == 512;
        let mut bytes_left = 2 * 1024 * 1024usize;
        if anchor.imported && namespace.is_none() {
            result.detail = "Exact related evidence is unavailable: this imported capture lacks a trustworthy source namespace/target. No host/time-only matches were inferred.".into();
            return Ok(result);
        }
        let anchor_proxy = active
            .inspection
            .findings
            .iter()
            .any(|f| f.role.starts_with("proxy"));
        for (_, id) in ids {
            self.auth.active(handle)?;
            result.searched += 1;
            if id == anchor.exchange_id {
                continue;
            }
            let session_id = format!("{:032x}", id.0);
            if self.traces.context_source_id(&session_id) != namespace {
                continue;
            }
            let candidate = self
                .service
                .catalog()
                .with_heads(id, |heads| {
                    let target = heads.metadata.original_target.as_target();
                    let redirect_candidate =
                        absolute(target).is_some_and(|url| redirects.contains(&url));
                    if heads.imported != anchor.imported
                        || !redirect_candidate
                            && (target.scheme != original.scheme
                                || target.host != original.host
                                || target.port != original.port)
                    {
                        return None;
                    }
                    let mut messages = vec![];
                    for (boundary, headers, target) in heads
                        .request_heads
                        .iter()
                        .map(|h| (h.boundary, &h.head.headers, Some(&h.head.target)))
                        .chain(
                            heads
                                .response_heads
                                .iter()
                                .map(|h| (h.boundary, &h.head.headers, None)),
                        )
                    {
                        let mut projected = vec![];
                        let mut has_windows = false;
                        for field in headers.iter() {
                            if field.is_redacted() {
                                continue;
                            }
                            let name = String::from_utf8_lossy(field.name());
                            if [
                                "authorization",
                                "proxy-authorization",
                                "www-authenticate",
                                "proxy-authenticate",
                                "location",
                            ]
                            .iter()
                            .any(|n| name.eq_ignore_ascii_case(n))
                            {
                                if field.value().len() > bytes_left {
                                    bytes_left = 0;
                                    return None;
                                }
                                bytes_left -= field.value().len();
                                let proxy = name.to_ascii_lowercase().starts_with("proxy-");
                                let scheme = field
                                    .value()
                                    .split(u8::is_ascii_whitespace)
                                    .next()
                                    .unwrap_or(&[]);
                                has_windows |= proxy == anchor_proxy
                                    && (scheme.eq_ignore_ascii_case(b"NTLM")
                                        || scheme.eq_ignore_ascii_case(b"Negotiate"));
                                projected.push(Header {
                                    name: name.into_owned(),
                                    value: field.value().into(),
                                    redacted: false,
                                });
                            }
                        }
                        let target = target.map(|t| {
                            format!(
                                "{}{}",
                                t.path,
                                t.query
                                    .as_ref()
                                    .map_or_else(String::new, |q| format!("?{q}"))
                            )
                        });
                        if target.as_ref().is_some_and(|t| t.len() > bytes_left) {
                            return None;
                        }
                        bytes_left =
                            bytes_left.saturating_sub(target.as_ref().map_or(0, String::len));
                        messages.push((boundary, projected, target, has_windows));
                    }
                    Some((
                        heads.request_number,
                        heads.metadata.downstream_connection_id
                            == anchor.metadata.downstream_connection_id,
                        messages,
                        redirect_candidate,
                    ))
                })
                .flatten();
            let Some((number, same_client, messages, redirect_candidate)) = candidate else {
                if bytes_left == 0 {
                    result.limited = true;
                    break;
                }
                continue;
            };
            let same_physical = connection_available
                && same_client
                && (!anchor.imported
                    || self.service.catalog().get(id).is_some_and(|s| {
                        s.performance.transports.iter().any(|t| {
                            t.leg == "client" && Some(t.connection_id.as_str()) == anchor_connection
                        })
                    }));
            for (boundary, headers, target, windows) in messages {
                let reason = if redirect_candidate && target.is_some() {
                    Some(
                        "Captured Location destination matches this observed request; fragments are excluded because they are not transmitted. This is redirect evidence, not proof of authentication.",
                    )
                } else if windows && same_physical {
                    Some(
                        "Auth-bearing message on the same captured client connection and authority, with the same proxy/origin role; connection context only.",
                    )
                } else {
                    let inspected = transmog_auth::inspect(
                        Input {
                            headers,
                            target,
                            body: None,
                            body_status: "skipped".into(),
                            captured_at: None,
                        },
                        Arc::new(AtomicBool::new(false)),
                    );
                    correlation(&active.inspection, &inspected)
                };
                if let Some(reason) = reason {
                    result.entries.push(RelatedEntry {
                        session_id: session_id.clone(),
                        boundary: crate::inspector::boundary(boundary),
                        label: format!("Request {number}"),
                        reason: reason.into(),
                    });
                    if result.entries.len() >= 100 {
                        result.limited = true;
                        break;
                    }
                }
            }
            if result.entries.len() >= 100 {
                break;
            }
        }
        self.authorized_auth(handle)?;
        Ok(result)
    }
}
