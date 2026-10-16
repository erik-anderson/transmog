use super::{check_cancel, invalid};
use crate::{AppError, Application};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::atomic::AtomicBool,
};
use transmog_capture::{CaptureLimits, CaptureRecordKind, CaptureWriter};
use transmog_portable as wire;

pub(super) struct Snapshot {
    pub objects: Vec<wire::Object>,
    pub sources: Vec<wire::CaseSource>,
    pub contexts: Vec<wire::CaseContext>,
    pub markers: Vec<wire::CaseMarker>,
    pub files: BTreeMap<String, tempfile::NamedTempFile>,
    pub links: BTreeMap<String, wire::EvidenceReference>,
}
#[allow(clippy::too_many_lines)] // One selected native evidence transaction reuses the production session writer.
pub(super) fn snapshot(
    app: &Application,
    review_id: &str,
    staging: &Path,
    canceled: &AtomicBool,
) -> Result<Snapshot, AppError> {
    let review = app
        .export_scope(Some(review_id))?
        .ok_or_else(|| invalid("Evidence review is unavailable"))?;
    let store = app
        .body_store
        .as_ref()
        .ok_or_else(|| invalid("Body retention is not configured"))?;
    store
        .flush()
        .map_err(|_| invalid("Retained body metadata is unavailable"))?;
    let sessions = app
        .service
        .catalog()
        .project_retained(|s| {
            if !review.ids.contains(&format!("{:032x}", s.exchange_id.0)) {
                return None;
            }
            let mut copy = s.clone();
            for body in &mut copy.bodies {
                body.retained_prefix = bytes::Bytes::new();
            }
            Some(copy)
        })
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if sessions.len() != review.ids.len() {
        return Err(invalid("Evidence changed after review"));
    }
    let mut source_keys = BTreeMap::new();
    let mut source_request_counts = BTreeMap::<String, usize>::new();
    let mut frozen = BTreeMap::new();
    let mut original_occurrences = BTreeMap::new();
    let mut entries = Vec::new();
    for session in &sessions {
        let runtime_id = format!("{:032x}", session.exchange_id.0);
        if session.imported && app.traces.entry(&runtime_id).is_none() {
            return Err(invalid("Original source was removed after review"));
        }
        let mut entry = crate::trace_save::source_entry(app, session);
        original_occurrences.insert(runtime_id.clone(), app.traces.entry_occurrence(&runtime_id));
        if !source_keys.contains_key(&entry.trace_id) {
            let metadata = app.traces.metadata(&entry.trace_id);
            if session.imported && metadata.is_none() {
                return Err(invalid("Original source metadata was removed after review"));
            }
            frozen.insert(
                entry.trace_id.clone(),
                (metadata, app.traces.frozen_origin(&entry.trace_id)),
            );
            source_keys.insert(
                entry.trace_id.clone(),
                crate::rule_interchange::fresh_id("source")?,
            );
        }
        let original_source = entry.trace_id.clone();
        *source_request_counts
            .entry(original_source.clone())
            .or_default() += 1;
        entry.trace_id.clone_from(&source_keys[&original_source]);
        entries.push(entry);
    }
    for (source, _) in frozen.values() {
        if let Some(source) = source {
            serialize_context(source, &mut std::io::sink())?;
        }
    }
    let mut file = tempfile::NamedTempFile::new_in(staging)
        .map_err(|_| invalid("Evidence staging is unavailable"))?;
    let mut writer = CaptureWriter::new(
        CanceledOutput {
            file: file.as_file_mut(),
            canceled,
        },
        CaptureLimits {
            max_file_bytes: u64::MAX,
            max_record_bytes: 8 * 1024 * 1024,
            max_records: usize::MAX,
        },
    )
    .map_err(AppError::from)?;
    let mut emitted_loss = BTreeSet::new();
    for (original, key) in &source_keys {
        let source = frozen[original].0.as_ref();
        if let Some(origin) = &frozen[original].1 {
            let (digest, _) = origin.digest(canceled).or_else(|error| {
                check_cancel(canceled)?;
                Err(error)
            })?;
            if emitted_loss.insert(digest) {
                for loss in origin.loss.iter() {
                    check_cancel(canceled)?;
                    let loss: GlobalLoss = serde_json::from_value(loss.clone())
                        .map_err(|_| invalid("Invalid source capture-wide loss"))?;
                    let record = CaptureRecordKind::Loss {
                        first_missing_sequence: wire::decimal(&loss.first_missing_sequence, true)
                            .map_err(crate::rule_interchange::portable_error)?,
                        count: wire::decimal(&loss.count, true)
                            .map_err(crate::rule_interchange::portable_error)?,
                        reason: loss.reason,
                    };
                    crate::trace_save::append(&mut writer, 0, 0, record)?;
                }
            }
        }
        let label = source.as_ref().map_or("Live traffic", |s| s.name.as_str());
        let format = source.as_ref().map_or("live", |s| s.format);
        crate::trace_save::append(
            &mut writer,
            0,
            0,
            CaptureRecordKind::Unknown {
                kind: "trace-source".into(),
                payload: serde_json::json!({"id":key,"name":label,"format":format,"path":"","sessions":entries.iter().filter(|e|&e.trace_id==key).count(),"importedAt":0,"context":null,"notes":[]}),
            },
        )?;
    }
    let mut incomplete_sources = BTreeSet::new();
    for (session, entry) in sessions.iter().zip(&entries) {
        check_cancel(canceled)?;
        let incomplete = crate::trace_save::write_session(
            &mut writer,
            app,
            store,
            session,
            false,
            Some(entry.clone()),
        )?;
        if incomplete > 0 || session.sequence_loss > 0 {
            incomplete_sources.insert(entry.trace_id.clone());
        }
    }
    writer.seal().map_err(AppError::from)?;
    file.as_file()
        .sync_all()
        .map_err(|_| invalid("Evidence could not be flushed"))?;
    let object = describe(
        file.reopen().map_err(|_| invalid("Evidence unavailable"))?,
        "capture",
        "application/vnd.transmog.tmcap",
        canceled,
    )?;
    let sha = object.sha256.clone();
    let mut result = Snapshot {
        objects: vec![object],
        sources: vec![],
        contexts: vec![],
        markers: vec![],
        files: BTreeMap::from([(sha.clone(), file)]),
        links: BTreeMap::new(),
    };
    for (original, key) in &source_keys {
        let source = frozen[original].0.as_ref();
        let origin = frozen[original]
            .1
            .as_ref()
            .map(|origin| origin.digest(canceled))
            .transpose()
            .or_else(|error| {
                check_cancel(canceled)?;
                Err(error)
            })?;
        let (parents, unavailable_parent_reason) = origin.map_or_else(
            || {
                (
                    vec![],
                    Some("An earlier artifact identity is unavailable for this source".into()),
                )
            },
            |(digest, source_key)| {
                (
                    vec![wire::ParentArtifact {
                        artifact_sha256: digest,
                        source_key: Some(source_key),
                        state: "not-exported".into(),
                        reason: Some(
                            "Original artifact is outside this selected derived copy".into(),
                        ),
                    }],
                    None,
                )
            },
        );
        result.sources.push(wire::CaseSource {
            id: key.clone(),
            artifact_sha256: sha.clone(),
            source_key: key.clone(),
            label: source
                .as_ref()
                .map_or_else(|| "Live traffic".into(), |s| s.name.clone()),
            request_count: Some(source_request_counts[original].to_string()),
            dependency: wire::Availability {
                state: "present".into(),
                reason: None,
                sha256: None,
                bytes: None,
            },
            lineage: wire::Lineage {
                kind: "derived".into(),
                operations: vec!["selected-native-export".into()],
                parents,
                entry_mappings: vec![],
                unavailable_parent_reason,
            },
            evidence_completeness: if !incomplete_sources.contains(key)
                && frozen[original]
                    .1
                    .as_ref()
                    .is_none_or(|origin| origin.loss.is_empty())
            {
                "unknown"
            } else {
                "incomplete"
            }
            .into(),
        });
        if let Some(source) = source {
            let mut context = tempfile::NamedTempFile::new_in(staging)
                .map_err(|_| invalid("Context staging unavailable"))?;
            serialize_context(source, context.as_file_mut())?;
            context
                .as_file()
                .sync_all()
                .map_err(|_| invalid("Context could not be flushed"))?;
            let object = describe(
                context
                    .reopen()
                    .map_err(|_| invalid("Context unavailable"))?,
                "context",
                "application/json",
                canceled,
            )?;
            let context_sha = object.sha256.clone();
            result.contexts.push(wire::CaseContext {
                id: crate::rule_interchange::fresh_id("context")?,
                kind: "capture-metadata".into(),
                origin: wire::ContextOrigin {
                    kind: "captured-source".into(),
                    collected_at: None,
                    collector: wire::Producer {
                        name: "unknown".into(),
                        version: "unknown".into(),
                    },
                    platform: "unknown".into(),
                    purpose: "preserved-source-context".into(),
                    machine_label: None,
                    source: Some(wire::SourceAssociation {
                        artifact_sha256: sha.clone(),
                        source_key: key.clone(),
                    }),
                },
                content: wire::Availability {
                    state: "present".into(),
                    reason: None,
                    sha256: Some(context_sha.clone()),
                    bytes: Some(object.bytes.clone()),
                },
                completeness: "unknown".into(),
                issues: vec![],
            });
            if let Some(markers) = source.context["incidentMarkers"].as_array() {
                for marker in markers {
                    let marker = transmog_capture::support::parse_marker(marker.clone())
                        .map_err(|_| invalid("Invalid source incident marker"))?;
                    result.markers.push(wire::CaseMarker {
                        id: crate::rule_interchange::fresh_id("marker")?,
                        label: marker.note,
                        source: wire::SourceAssociation {
                            artifact_sha256: sha.clone(),
                            source_key: key.clone(),
                        },
                        original_id: marker.id,
                        elapsed_millis: marker.offset_millis.map(|v| v.to_string()),
                    });
                }
            }
            if !result.files.contains_key(&context_sha) {
                result.objects.push(object);
                result.files.insert(context_sha, context);
            }
        }
    }
    let mut occurrences = BTreeMap::new();
    for (session, entry) in sessions.iter().zip(entries) {
        let occurrence = occurrences
            .entry((entry.trace_id.clone(), entry.original_id.clone()))
            .or_insert(0_u64);
        let runtime_id = format!("{:032x}", session.exchange_id.0);
        if let Some(original_occurrence) = original_occurrences[&runtime_id]
            && original_occurrence != *occurrence
        {
            let source = result
                .sources
                .iter_mut()
                .find(|s| s.source_key == entry.trace_id)
                .ok_or_else(|| invalid("Missing derived source mapping"))?;
            if let Some(parent) = source.lineage.parents.first() {
                source.lineage.entry_mappings.push(wire::CaseEntryLineage {
                    exchange_id: Some(entry.original_id.clone()),
                    occurrence: occurrence.to_string(),
                    original: wire::EvidenceReference {
                        artifact_sha256: parent.artifact_sha256.clone(),
                        source_key: parent
                            .source_key
                            .clone()
                            .ok_or_else(|| invalid("Missing parent source identity"))?,
                        exchange_id: Some(entry.original_id.clone()),
                        occurrence: original_occurrence.to_string(),
                        boundary: "client-response".into(),
                    },
                });
            }
        }
        let reference = wire::EvidenceReference {
            artifact_sha256: sha.clone(),
            source_key: entry.trace_id,
            exchange_id: Some(entry.original_id),
            occurrence: occurrence.to_string(),
            boundary: "client-response".into(),
        };
        result.links.insert(runtime_id, reference);
        *occurrence += 1;
    }
    let remaining = app
        .service
        .catalog()
        .project_retained(|session| {
            usize::from(
                review
                    .ids
                    .contains(&format!("{:032x}", session.exchange_id.0)),
            )
        })
        .into_iter()
        .sum::<usize>();
    if remaining != sessions.len() {
        return Err(invalid(
            "Evidence was removed while creating the case; review the selection again",
        ));
    }
    Ok(result)
}
fn describe(
    mut file: File,
    role: &str,
    media: &str,
    canceled: &AtomicBool,
) -> Result<wire::Object, AppError> {
    use sha2::Digest as _;
    let mut hash = sha2::Sha256::new();
    let mut buffer = vec![0; 128 * 1024];
    let mut bytes = 0_u64;
    loop {
        check_cancel(canceled)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| invalid("Case object cannot be read"))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes = bytes
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Object length overflow"))?;
    }
    Ok(wire::Object {
        sha256: format!("{:x}", hash.finalize()),
        bytes: bytes.to_string(),
        role: role.into(),
        media_type: media.into(),
    })
}
struct Limited<'a> {
    file: &'a mut dyn Write,
    remaining: usize,
}

struct CanceledOutput<'a> {
    file: &'a mut File,
    canceled: &'a AtomicBool,
}
impl Write for CanceledOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.canceled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(std::io::Error::other("Case export canceled"));
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
#[derive(serde::Serialize)]
struct SourceContext<'a> {
    context: &'a serde_json::Value,
    notes: &'a [String],
    format: &'a str,
}
fn serialize_context(
    source: &crate::TraceMetadataView,
    output: &mut impl Write,
) -> Result<(), AppError> {
    serde_json::to_writer(
        Limited {
            file: output,
            remaining: 8 * 1024 * 1024,
        },
        &SourceContext {
            context: &source.context,
            notes: &source.notes,
            format: source.format,
        },
    )
    .map_err(|_| invalid("Source context exceeds its object limit or cannot be written"))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GlobalLoss {
    first_missing_sequence: String,
    count: String,
    reason: String,
}
impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("Context limit"));
        }
        let count = self.file.write(bytes)?;
        self.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
