use super::{
    AppError, Application, Body, ComparisonSide, ComparisonSubject, Evidence, HeaderBlock,
    MAX_BODY, MAX_TEXT, Message, Operation, PreviewComparisonEvidence, Read, Seek, SeekFrom, error,
    now, short,
};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::{fs::File, io::Write, path::Path};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};
use transmog_session::SessionTerminal;

#[allow(clippy::too_many_lines)] // Explicit subject variants keep acquisition and lifetime rules visible.
pub(super) async fn acquire(
    app: &Application,
    subject: &ComparisonSubject,
    preview: Option<PreviewComparisonEvidence>,
    path: &Path,
    op: &Operation,
    response_only: bool,
) -> Result<Evidence, AppError> {
    op.check()?;
    std::fs::create_dir(path).map_err(|_| error("Snapshot storage unavailable"))?;
    let side = ComparisonSide {
        label: String::new(),
        source: String::new(),
        entry_id: None,
        acquired_at: now(),
        boundary: String::new(),
        notes: vec![],
    };
    let mut evidence = Evidence {
        side,
        request: None,
        response: None,
        outcome: None,
        timings: vec![],
        versions: vec![],
        omissions: vec![],
        stream: None,
        stream_indicated: false,
        exchange: false,
    };
    match subject {
        ComparisonSubject::Exchange {
            id,
            request_boundary,
            response_boundary,
        } => {
            evidence.exchange = true;
            let numeric = ExchangeId(crate::inspector::parse_session_id(id)?);
            let request_stage = crate::inspector::parse_boundary(request_boundary)?;
            let response_stage = crate::inspector::parse_boundary(response_boundary)?;
            if !matches!(
                request_stage,
                ExchangeBoundary::ClientRequest | ExchangeBoundary::UpstreamRequest
            ) || !matches!(
                response_stage,
                ExchangeBoundary::ClientResponse | ExchangeBoundary::UpstreamResponse
            ) {
                return Err(error("Select a request boundary and a response boundary"));
            }
            if let Some(store) = &app.body_store {
                store
                    .flush()
                    .map_err(|_| error("Body evidence is temporarily unavailable"))?;
            }
            let Some(snapshot) = app.service.catalog().get(numeric) else {
                evidence.side.label = format!("Request {id}");
                evidence
                    .side
                    .notes
                    .push("Source no longer retained; choose another subject".into());
                return Ok(evidence);
            };
            let header_bytes = snapshot
                .request_heads
                .iter()
                .filter(|h| !response_only && h.boundary == request_stage)
                .map(|h| &h.head.headers)
                .chain(
                    snapshot
                        .response_heads
                        .iter()
                        .filter(|h| h.boundary == response_stage)
                        .map(|h| &h.head.headers),
                )
                .flat_map(HeaderBlock::iter)
                .map(|f| f.name().len() + f.value().len())
                .sum::<usize>();
            let header_fields = snapshot
                .request_heads
                .iter()
                .filter(|h| !response_only && h.boundary == request_stage)
                .map(|h| &h.head.headers)
                .chain(
                    snapshot
                        .response_heads
                        .iter()
                        .filter(|h| h.boundary == response_stage)
                        .map(|h| &h.head.headers),
                )
                .map(|h| h.iter().count())
                .sum::<usize>();
            if header_bytes > 4 * 1024 * 1024 || header_fields > 16_384 {
                return Err(error(
                    "Exchange headers exceed the 4 MiB comparison input budget; inspect source headers instead",
                ));
            }
            evidence.side.label = format!("Request #{}", snapshot.request_number);
            evidence.side.entry_id = Some(id.clone());
            evidence.side.notes.push(format!(
                "Recorded capture start (UTC, source clock): {}",
                capture_time(snapshot.metadata.started_at)
            ));
            evidence.side.boundary = if response_only {
                response_boundary.clone()
            } else {
                format!("{request_boundary} / {response_boundary}")
            };
            evidence.side.source = "Live traffic".into();
            let trace = app.traces.entry(id);
            if let Some(entry) = &trace {
                evidence.side.source = app
                    .traces
                    .metadata(&entry.trace_id)
                    .map_or_else(|| entry.trace_id.clone(), |m| m.name);
                evidence.side.notes.push(short(&format!(
                    "Original ID: {} · source {}",
                    entry.original_id, entry.trace_id
                )));
                evidence
                    .side
                    .notes
                    .extend(entry.diagnostics.iter().take(64).map(|s| short(s)));
                if entry.diagnostics.len() > 64 {
                    evidence.omissions.push(
                        "Further source diagnostics omitted; open original source details".into(),
                    );
                }
                if !entry.target_known {
                    evidence
                        .side
                        .notes
                        .push("Request target unavailable in source".into());
                }
            }
            evidence.versions.push((id.clone(), snapshot.last_sequence));
            let family = trace.as_ref().map(|entry| {
                if entry.timings.keys().any(|key| key.starts_with("HAR ")) {
                    "har"
                } else if entry.timings.keys().any(|key| key.starts_with("NetLog ")) {
                    "netlog"
                } else {
                    app.traces
                        .metadata(&entry.trace_id)
                        .map_or("native", |m| m.format)
                }
            });
            for (request, boundary) in [(true, request_stage), (false, response_stage)] {
                if request && response_only {
                    continue;
                }
                let message = if request {
                    snapshot
                        .request_heads
                        .iter()
                        .find(|h| h.boundary == boundary)
                        .map(|h| Message {
                            boundary: request_boundary.clone(),
                            first: if request_line_available(&h.head) {
                                format!(
                                    "{} {}",
                                    h.head.method,
                                    crate::request_actions::url(&h.head)
                                )
                            } else {
                                "Request method/target exceeds the 64 KiB comparison budget".into()
                            },
                            protocol: Some(format!("{:?}", h.head.source_version)),
                            headers: h.head.headers.clone(),
                            body: Body::missing("Body observation unavailable"),
                            available: request_line_available(&h.head),
                            trailers: None,
                        })
                } else {
                    snapshot
                        .response_heads
                        .iter()
                        .find(|h| h.boundary == boundary)
                        .map(|h| Message {
                            boundary: response_boundary.clone(),
                            first: h.head.status.to_string(),
                            protocol: Some(format!("{:?}", h.head.source_version)),
                            headers: h.head.headers.clone(),
                            body: Body::missing("Body observation unavailable"),
                            available: true,
                            trailers: None,
                        })
                };
                if let Some(mut message) = message {
                    message.protocol = snapshot
                        .performance
                        .protocols
                        .iter()
                        .find(|p| p.boundary == message.boundary)
                        .map(|p| p.version.clone());
                    if request && trace.as_ref().is_some_and(|entry| !entry.target_known) {
                        message.first = "Request target unavailable in source".into();
                        message.available = false;
                    }
                    message.trailers = snapshot
                        .bodies
                        .iter()
                        .find(|body| body.boundary == boundary)
                        .map(|body| body.trailers.clone().unwrap_or_default());
                    message.body = body(
                        app,
                        numeric,
                        boundary,
                        &path.join(if request { "request" } else { "response" }),
                        op,
                    )?;
                    if let Some(meta) = app.body_store.as_ref().and_then(|store| {
                        store.metadata(numeric).into_iter().find(|meta| {
                            crate::inspector::parse_boundary(meta.boundary).ok() == Some(boundary)
                        })
                    }) {
                        let source_decoded = !request && matches!(family, Some("har" | "netlog"));
                        let adjusted = family.is_some()
                            && (header_codings(&message.headers) != meta.content_codings
                                || meta
                                    .wire_body_bytes
                                    .is_some_and(|wire| wire != meta.retained_bytes));
                        if source_decoded || adjusted || (request && family == Some("har")) {
                            message.body.representation = if source_decoded {
                                "decoded bytes"
                            } else {
                                "retained source representation"
                            }
                            .into();
                            evidence.omissions.push(format!("{}: original encoded entity bytes are unavailable or source-adjusted; retained/decoded bytes are compared in their labeled representation",message.boundary));
                        }
                    }
                    if !request && message.first == "304" {
                        message.body = Body::missing(
                            "HTTP 304 has no representation body; the original browser cache was not captured",
                        );
                    }
                    if request {
                        evidence.request = Some(message);
                    } else {
                        evidence.response = Some(message);
                    }
                }
            }
            evidence.outcome = snapshot.terminal.as_ref().map(|t| match t {
                SessionTerminal::Completed(_) => "Completed (local observation)".into(),
                SessionTerminal::Failed(f) => format!(
                    "Transport failure {:?} at {:?}; request committed={}, response committed={}",
                    f.kind, f.stage, f.request_committed, f.response_committed
                ),
            });
            if snapshot.sequence_loss > 0 {
                evidence.omissions.push(format!(
                    "{} observer events missing",
                    snapshot.sequence_loss
                ));
            }
            evidence.stream = crate::StreamSummary::from_snapshot(&snapshot);
            evidence.stream_indicated = snapshot
                .request_heads
                .iter()
                .any(|head| head.head.method == "CONNECT")
                || snapshot.stream.is_some()
                || snapshot.websocket.is_some()
                || !snapshot.stream_history.items.is_empty()
                || snapshot.response_heads.iter().any(|head| {
                    head.head.status == 101
                        || head.head.headers.iter().any(|field| {
                            field.name().eq_ignore_ascii_case(b"content-type")
                                && String::from_utf8_lossy(field.value())
                                    .to_ascii_lowercase()
                                    .starts_with("text/event-stream")
                        })
                });
            if family.is_none_or(|format| format == "native") {
                // Compare intervals with the same named, locally measured endpoints only.
                use transmog_core::performance::Milestone;
                for (label, start, end) in [
                    (
                        "Request to response headers",
                        Milestone::RequestHeaders,
                        Milestone::ResponseHeaders,
                    ),
                    (
                        "Client response queue duration",
                        Milestone::ClientResponseBegin,
                        Milestone::ClientResponseQueued,
                    ),
                    (
                        "Exchange duration",
                        Milestone::RequestHeaders,
                        Milestone::ExchangeDone,
                    ),
                ] {
                    let a = snapshot
                        .performance
                        .points
                        .iter()
                        .find(|p| p.milestone == start);
                    let b = snapshot
                        .performance
                        .points
                        .iter()
                        .find(|p| p.milestone == end);
                    if let (Some(a), Some(b)) = (a, b)
                        && let Ok(duration) =
                            u64::try_from(b.offset_micros.saturating_sub(a.offset_micros))
                    {
                        evidence.timings.push((label.into(), duration));
                    }
                }
            } else {
                evidence.side.notes.push("Imported timing definitions remain in source details; native timing deltas are unavailable.".into());
            }
        }
        ComparisonSubject::Asset { reference } => {
            let (asset, file) = match app.response_assets.comparison_source(reference) {
                Ok(source) => source,
                Err(e) => {
                    evidence.side.label = short(&format!("Response {reference}"));
                    evidence.side.source = "Saved response configuration".into();
                    evidence.side.boundary = "configured response".into();
                    evidence.side.notes.push(e.message);
                    return Ok(evidence);
                }
            };
            check_headers(&asset.headers)?;
            let linked = asset.linked_body.is_some();
            let mut body = match file {
                Some(file) => freeze_reader(file, &path.join("response"), op)
                    .unwrap_or_else(|e| Body::missing(e.message)),
                None => Body::missing(
                    "Saved response bytes are unavailable; configuration metadata is retained",
                ),
            };
            if !linked {
                body.representation = "encoded entity bytes".into();
                body.codings = header_codings(&asset.headers);
                if body.complete
                    && (body.digest.as_deref() != Some(asset.sha256.as_str())
                        || body.length != Some(asset.body_bytes))
                {
                    body = Body::missing(
                        "Stored asset bytes no longer match their immutable length/hash",
                    );
                }
            }
            body.charset = charset(&asset.headers);
            evidence.side.label = format!("Response {reference}");
            evidence.side.source = "Saved response configuration".into();
            evidence.side.boundary = if linked {
                "configured response / decoded linked-file input"
            } else {
                "configured response / encoded entity bytes"
            }
            .into();
            evidence.side.notes.push("Configured asset metadata and derived framing/compression are distinct from an observed downstream response.".into());
            if linked {
                evidence.side.notes.push("Current linked-file input acquired now, not a historical run; configured recompression is not materialized by comparison.".into());
            }
            evidence.response = Some(Message {
                boundary: "configured response".into(),
                first: asset.status.to_string(),
                protocol: None,
                headers: asset.headers,
                available: true,
                trailers: None,
                body,
            });
        }
        ComparisonSubject::Draft {
            label,
            status,
            headers,
            decoded_body,
            base,
            body_path,
        } => {
            check_headers(headers)?;
            if label.len() > 512
                || status.len() > 128
                || decoded_body.as_ref().is_some_and(|b| b.len() > MAX_TEXT)
            {
                return Err(error("Draft exceeds comparison input limit"));
            }
            if let Some(base) = base {
                if !matches!(
                    base.as_ref(),
                    ComparisonSubject::Exchange { .. } | ComparisonSubject::Asset { .. }
                ) {
                    return Err(error("A draft base must be an exchange or asset revision"));
                }
                let previous =
                    Box::pin(acquire(app, base, None, &path.join("base"), op, true)).await?;
                if let Some(request) = &previous.request
                    && let Some(file) = &request.body.path
                {
                    let _ = std::fs::remove_file(file);
                }
                evidence.response = previous.response;
                evidence.versions = previous.versions;
            }
            evidence.side.label = label.clone();
            evidence.side.source = "Unsaved editor snapshot".into();
            evidence.side.boundary = "proposed response".into();
            evidence.side.notes.push("Draft only; no save, rule activation or preview. Derived framing/compression repairs are not authored edits.".into());
            let mut body = evidence
                .response
                .take()
                .map_or_else(|| Body::missing("Draft body unavailable"), |m| m.body);
            if (body_path.is_some() || decoded_body.is_some())
                && let Some(file) = body.path.take()
            {
                let _ = std::fs::remove_file(file);
            }
            if let Some(file_path) = body_path {
                body = freeze_file(file_path, &path.join("response"), op)
                    .unwrap_or_else(|e| Body::missing(e.message));
                body.representation = "decoded bytes".into();
            } else if let Some(bytes) = decoded_body {
                let file_path = path.join("response");
                std::fs::write(&file_path, bytes)
                    .map_err(|_| error("Draft snapshot storage unavailable"))?;
                body = Body {
                    path: Some(file_path),
                    length: Some(bytes.len() as u64),
                    digest: Some(format!("{:x}", Sha256::digest(bytes))),
                    complete: true,
                    representation: "decoded bytes".into(),
                    codings: vec![],
                    charset: charset(headers),
                    reason: String::new(),
                };
            }
            body.charset = charset(headers);
            let valid_status = (100..=599).contains(&status.parse::<u16>().unwrap_or(0));
            if !valid_status {
                evidence.side.notes.push("Invalid draft status; displayed as proposed input, not a valid generated response.".into());
            }
            evidence.response = Some(Message {
                boundary: "proposed response".into(),
                first: status.clone(),
                protocol: None,
                headers: headers.clone(),
                body,
                available: valid_status,
                trailers: None,
            });
        }
        ComparisonSubject::Preview { run, observation } => {
            if run.len() > 128 {
                return Err(error("Invalid preview reference"));
            }
            evidence.side.label = format!("Preview {run} · request {observation}");
            evidence.side.source = "Retained preview".into();
            evidence.side.boundary = "supplied browser response".into();
            evidence.omissions.push("Interception evidence only; browser completion, CORS/CSP acceptance and stream contents are not established. Reports retain only the last 256 observations.".into());
            if let Some(preview) = preview {
                evidence.side.label = preview.label.clone();
                evidence.side.source = preview.label;
                evidence.side.entry_id = preview.source_id;
                evidence
                    .side
                    .notes
                    .extend(preview.notes.iter().take(16).map(|n| short(n)));
                let headers = parse_preview_headers(&preview.resource.headers)?;
                let mut body =
                    freeze_file(&preview.resource.body_path, &path.join("response"), op)?;
                body.representation = "decoded bytes".into();
                body.charset = charset(&headers);
                if body.complete && body.digest.as_deref() != Some(preview.resource.sha256.as_str())
                {
                    body = Body::missing(
                        "Frozen preview bytes no longer match their supplied-response identity",
                    );
                }
                evidence.response = Some(Message {
                    boundary: "supplied browser response".into(),
                    first: preview.resource.status.to_string(),
                    protocol: None,
                    headers,
                    body,
                    available: true,
                    trailers: None,
                });
            } else {
                evidence.side.notes.push("Observation response unavailable, expired, or a network failure; no HTTP response substituted.".into());
            }
        }
    }
    op.check()?;
    Ok(evidence)
}

fn body(
    app: &Application,
    id: ExchangeId,
    boundary: ExchangeBoundary,
    path: &Path,
    op: &Operation,
) -> Result<Body, AppError> {
    let Some(store) = &app.body_store else {
        return Ok(Body::missing("Body retention is not configured"));
    };
    let Some(meta) = store
        .metadata(id)
        .into_iter()
        .find(|m| crate::inspector::parse_boundary(m.boundary).ok() == Some(boundary))
    else {
        return Ok(Body::missing("Body observation unavailable"));
    };
    let mut body = Body {
        path: None,
        length: Some(meta.retained_bytes),
        digest: None,
        complete: meta.availability == crate::BodyAvailability::Complete,
        representation: "encoded entity bytes".into(),
        codings: meta.content_codings,
        charset: meta.charset,
        reason: format!(
            "{:?}: {} of {} observed bytes retained. {}",
            meta.availability,
            meta.retained_bytes,
            meta.observed_bytes,
            meta.reason.unwrap_or_default()
        ),
    };
    if !body.complete || meta.retained_bytes > MAX_BODY {
        body.complete = false;
        if meta.retained_bytes > MAX_BODY {
            body.reason.push_str(
                " Body exceeds 32 MiB snapshot budget; source Hex/Save as retains complete access.",
            );
        }
        if meta.retained_bytes > 0
            && let Ok(range) = store.read_paged_range(id, boundary, 0, 64 * 1024)
            && !range.bytes.is_empty()
        {
            std::fs::write(path, &range.bytes)
                .map_err(|_| error("Prefix snapshot storage unavailable"))?;
            body.length = Some(range.bytes.len() as u64);
            body.path = Some(path.into());
            write!(
                &mut body.reason,
                " Frozen retained prefix only: {} bytes; no whole-body equality inferred.",
                range.bytes.len()
            )
            .expect("String write");
        }
        return Ok(body);
    }
    let Ok(lease) = store.open_complete(id, boundary) else {
        body.complete = false;
        body.reason = "Complete bytes expired or unreadable at acquisition".into();
        return Ok(body);
    };
    let (length, digest) = copy(lease, path, op)?;
    if meta.length_known && length != meta.retained_bytes {
        return Err(error("Body source length changed during acquisition"));
    }
    body.length = Some(length);
    body.digest = Some(digest);
    body.path = Some(path.into());
    body.reason.clear();
    Ok(body)
}

fn freeze_file(source: &Path, destination: &Path, op: &Operation) -> Result<Body, AppError> {
    let file = File::open(source).map_err(|_| error("Response file unavailable"))?;
    freeze_reader(file, destination, op)
}
fn freeze_reader(mut file: File, destination: &Path, op: &Operation) -> Result<Body, AppError> {
    let before = file
        .metadata()
        .map_err(|_| error("Response file metadata unavailable"))?;
    if !before.is_file() {
        return Err(error("Response file must be a regular file"));
    }
    let limit = if before.len() > MAX_BODY {
        64 * 1024
    } else {
        before.len()
    };
    let (length, digest) = copy((&mut file).take(limit), destination, op)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error("Response file could not be verified"))?;
    let mut verified = Sha256::new();
    let mut buffer = vec![0; 65536];
    let mut count = 0;
    loop {
        op.check()?;
        let size = (&mut file)
            .take(limit - count)
            .read(&mut buffer)
            .map_err(|_| error("Response file verification failed"))?;
        if size == 0 {
            break;
        }
        count += size as u64;
        if count > MAX_BODY {
            return Err(error("Response file changed during acquisition"));
        }
        verified.update(&buffer[..size]);
    }
    let after = file
        .metadata()
        .map_err(|_| error("Response metadata unavailable"))?;
    if count != length
        || length != limit
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || format!("{:x}", verified.finalize()) != digest
    {
        return Err(error("Response file changed during acquisition; retry"));
    }
    Ok(Body {
        path: Some(destination.into()),
        length: Some(length),
        digest: (before.len() <= MAX_BODY).then_some(digest),
        complete: before.len() <= MAX_BODY,
        representation: "decoded bytes".into(),
        codings: vec![],
        charset: None,
        reason: if before.len() > MAX_BODY {
            format!(
                "Frozen retained prefix only: {length} of {} file bytes; 32 MiB complete-snapshot budget exceeded",
                before.len()
            )
        } else {
            String::new()
        },
    })
}
fn copy(mut reader: impl Read, path: &Path, op: &Operation) -> Result<(u64, String), AppError> {
    let mut target = File::create(path).map_err(|_| error("Snapshot storage unavailable"))?;
    let mut hasher = Sha256::new();
    let mut length = 0;
    let mut buffer = vec![0; 65536];
    loop {
        op.check()?;
        let size = reader
            .read(&mut buffer)
            .map_err(|_| error("Source bytes unreadable"))?;
        if size == 0 {
            break;
        }
        length += size as u64;
        if length > MAX_BODY {
            return Err(error("Source grew beyond comparison snapshot limit"));
        }
        target
            .write_all(&buffer[..size])
            .map_err(|_| error("Snapshot storage full"))?;
        hasher.update(&buffer[..size]);
    }
    Ok((length, format!("{:x}", hasher.finalize())))
}
fn charset(headers: &HeaderBlock) -> Option<String> {
    headers
        .iter()
        .find(|h| h.name().eq_ignore_ascii_case(b"content-type"))
        .and_then(|h| std::str::from_utf8(h.value()).ok())
        .and_then(|s| {
            s.split(';').skip(1).find_map(|p| {
                p.trim()
                    .split_once('=')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("charset"))
                    .map(|(_, v)| v.trim_matches('"').to_string())
            })
        })
}
fn parse_preview_headers(text: &str) -> Result<HeaderBlock, AppError> {
    let mut fields = vec![];
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| error("Invalid frozen preview headers"))?;
        fields.push(
            transmog_core::HeaderField::try_new(name, value.trim_start())
                .map_err(|_| error("Invalid frozen preview field"))?,
        );
    }
    Ok(HeaderBlock::from_fields(fields))
}

fn header_codings(headers: &HeaderBlock) -> Vec<String> {
    headers
        .iter()
        .filter(|field| field.name().eq_ignore_ascii_case(b"content-encoding"))
        .flat_map(|field| {
            String::from_utf8_lossy(field.value())
                .split(',')
                .map(|coding| coding.trim().to_ascii_lowercase())
                .filter(|coding| coding != "identity")
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn request_line_available(head: &transmog_core::RequestHead) -> bool {
    head.method.len()
        + head.target.scheme.len()
        + head.target.authority.len()
        + head.target.path.len()
        + head.target.query.as_ref().map_or(0, String::len)
        <= 64 * 1024
}

fn check_headers(headers: &HeaderBlock) -> Result<(), AppError> {
    if headers.iter().count() > 16_384
        || headers
            .iter()
            .map(|f| f.name().len() + f.value().len())
            .sum::<usize>()
            > 4 * 1024 * 1024
    {
        return Err(error(
            "Response headers exceed the 4 MiB / 16,384 field comparison budget",
        ));
    }
    Ok(())
}

fn capture_time(value: std::time::SystemTime) -> String {
    value
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .filter(|d| !d.is_zero())
        .and_then(|d| i128::try_from(d.as_nanos()).ok())
        .and_then(|n| time::OffsetDateTime::from_unix_timestamp_nanos(n).ok())
        .and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "Unavailable".into())
}
