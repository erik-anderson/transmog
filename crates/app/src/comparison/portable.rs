//! Stable case references are rebound explicitly; stored findings are never new equality evidence.
use super::{
    AppError, Application, ComparisonOptions, ComparisonSubject, PairingOptions, Serialize, error,
    pairing,
};
use std::collections::BTreeMap;
use transmog_portable as wire;

pub(super) const ENGINE: &str = "transmog-comparison/1";
pub(super) fn reference(
    links: &BTreeMap<String, wire::EvidenceReference>,
    id: &str,
) -> Result<wire::EvidenceReference, AppError> {
    links.get(id).cloned().ok_or_else(||error("A saved comparison needs evidence outside this export. Include all compared requests and review again."))
}
pub(super) fn settings(
    options: &ComparisonOptions,
    pairing: &PairingOptions,
) -> wire::ComparisonSettings {
    wire::ComparisonSettings {
        excluded_headers: options.excluded_headers.clone(),
        normalize_newlines: options.normalize_newlines,
        excluded_query_names: pairing.excluded_query_names.clone(),
        request_headers: pairing.request_headers.clone(),
        request_body_digest: pairing.request_body_digest,
    }
}
impl Application {
    /// Lists retained comparisons that can be rebound to original captured exchanges.
    pub fn case_comparison_choices(&self) -> Vec<(String, String, Vec<String>)> {
        let state = self
            .comparisons
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut choices = state
            .results
            .iter()
            .filter(|s| {
                s.subjects
                    .iter()
                    .all(|v| matches!(v, ComparisonSubject::Exchange { .. }))
            })
            .map(|s| {
                (
                    s.result.id.clone(),
                    format!("{} / {}", s.result.left.label, s.result.right.label),
                    s.subjects
                        .iter()
                        .filter_map(|v| {
                            if let ComparisonSubject::Exchange { id, .. } = v {
                                Some(id.clone())
                            } else {
                                None
                            }
                        })
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        choices.extend(pairing::case_choices(&state.captures));
        choices
    }
    pub(crate) fn export_case_comparisons(
        &self,
        ids: &[String],
        links: &BTreeMap<String, wire::EvidenceReference>,
    ) -> Result<Vec<wire::CaseComparison>, AppError> {
        if ids.len() > 16 {
            return Err(error("Select up to 16 saved comparisons"));
        }
        let mut output = vec![];
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            if !seen.insert(id) {
                return Err(error("Duplicate selected comparison"));
            }
            // Check after evidence acquisition as well; an old frozen result must not acquire new meaning.
            let result = {
                self.comparisons
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .results
                    .iter()
                    .find(|s| &s.result.id == id)
                    .cloned()
            };
            if let Some(s) = result {
                if self.comparison_stale(id)? {
                    return Err(error(
                        "A comparison source changed. Refresh the comparison before saving its case.",
                    ));
                }
                let mut subjects = vec![];
                let mut request = String::new();
                for subject in &s.subjects {
                    let ComparisonSubject::Exchange {
                        id,
                        request_boundary,
                        response_boundary,
                    } = subject
                    else {
                        return Err(error(
                            "Save recorded-request comparisons; draft, asset and preview comparisons are temporary.",
                        ));
                    };
                    if !request.is_empty() && request != *request_boundary {
                        return Err(error(
                            "Choose the same request boundary on both sides before saving",
                        ));
                    }
                    request.clone_from(request_boundary);
                    let mut r = reference(links, id)?;
                    r.boundary.clone_from(response_boundary);
                    subjects.push(r);
                }
                output.push(wire::CaseComparison{id:format!("comparison-{}",output.len()+1),label:format!("{} / {}",s.result.left.label,s.result.right.label),kind:"exchange".into(),engine:ENGINE.into(),left:vec![subjects.remove(0)],right:subjects,request_boundary:request,settings:settings(&s.result.options,&PairingOptions::default()),corrections:vec![],recorded_summary:format!("Recorded result: {} changes, {} unavailable/not compared, {} excluded. Recompute to assess included evidence.",s.result.changed,s.result.unknown,s.result.excluded)});
            } else {
                let state = self
                    .comparisons
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut c = pairing::export_case(self, &state.captures, id, links)?;
                c.id = format!("comparison-{}", output.len() + 1);
                output.push(c);
            }
        }
        Ok(output)
    }
    /// Rebinds an exact portable exchange identity; filenames and workspace row numbers are never keys.
    /// # Errors
    /// Missing/ambiguous sources are reported before any comparison is started.
    pub fn resolve_case_comparison(
        &self,
        comparison: &wire::CaseComparison,
        links: &[crate::CaseEvidenceLink],
    ) -> Result<CaseComparisonOpen, AppError> {
        if comparison.engine != ENGINE {
            return Err(error(
                "This comparison engine version is unsupported. Recorded findings remain historical; choose new subjects to compare.",
            ));
        }
        let key = |r: &wire::EvidenceReference| {
            (
                r.artifact_sha256.clone(),
                r.source_key.clone(),
                r.exchange_id.clone(),
                r.occurrence.clone(),
            )
        };
        let mut bindings = BTreeMap::new();
        let mut ambiguous = std::collections::HashSet::new();
        for link in links {
            let identity = key(&link.reference);
            if let Some(previous) = bindings.insert(identity.clone(), &link.exchange_id)
                && previous != &link.exchange_id
            {
                ambiguous.insert(identity);
            }
        }
        let resolve = |r: &wire::EvidenceReference| {
            let identity = key(r);
            if ambiguous.contains(&identity) {
                return Err(error(
                    "Saved comparison evidence has multiple open instances. Reopen the case and its sources once.",
                ));
            }
            let id=bindings.get(&identity).ok_or_else(||error("Saved comparison evidence is unavailable. Open every referenced source, then retry."))?;
            if self
                .service
                .catalog()
                .sequence(transmog_core::intercept::ExchangeId(
                    crate::inspector::parse_session_id(id)?,
                ))
                .is_none()
            {
                return Err(error(
                    "Saved comparison evidence was removed from Traffic. Open its source again.",
                ));
            }
            Ok((*id).clone())
        };
        let left = comparison
            .left
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()?;
        let right = comparison
            .right
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()?;
        let options = restored_options(&comparison.settings)?;
        if comparison.kind == "exchange" {
            if left.len() != 1 || right.len() != 1 {
                return Err(error("Saved comparison needs two recorded requests"));
            }
            return Ok(CaseComparisonOpen::Exchange {
                left: ComparisonSubject::Exchange {
                    id: left[0].clone(),
                    request_boundary: comparison.request_boundary.clone(),
                    response_boundary: comparison.left[0].boundary.clone(),
                },
                right: ComparisonSubject::Exchange {
                    id: right[0].clone(),
                    request_boundary: comparison.request_boundary.clone(),
                    response_boundary: comparison.right[0].boundary.clone(),
                },
                options,
            });
        }
        if comparison.kind != "captures" {
            return Err(error("Unsupported saved comparison kind"));
        }
        let corrections = comparison
            .corrections
            .iter()
            .map(|p| {
                Ok((
                    p.left.as_ref().map(resolve).transpose()?,
                    p.right.as_ref().map(resolve).transpose()?,
                ))
            })
            .collect::<Result<_, AppError>>()?;
        Ok(CaseComparisonOpen::Captures {
            left,
            right,
            options: PairingOptions {
                excluded_query_names: comparison.settings.excluded_query_names.clone(),
                request_headers: comparison.settings.request_headers.clone(),
                request_body_digest: comparison.settings.request_body_digest,
            },
            corrections,
        })
    }
}
fn restored_options(settings: &wire::ComparisonSettings) -> Result<ComparisonOptions, AppError> {
    let options = ComparisonOptions {
        excluded_headers: settings.excluded_headers.clone(),
        normalize_newlines: settings.normalize_newlines,
    };
    options.validate()?;
    Ok(options)
}
/// Resolved subjects/options for the existing desktop comparison flow.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CaseComparisonOpen {
    Exchange {
        left: ComparisonSubject,
        right: ComparisonSubject,
        options: ComparisonOptions,
    },
    Captures {
        left: Vec<String>,
        right: Vec<String>,
        options: PairingOptions,
        corrections: Vec<(Option<String>, Option<String>)>,
    },
}
