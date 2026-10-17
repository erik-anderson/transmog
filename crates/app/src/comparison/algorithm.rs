use super::{
    AppError, Body, ComparisonOptions, ComparisonRow, ComparisonState, Evidence, HashMap,
    HeaderBlock, MAX_ROWS, MAX_TEXT, Operation, VecDeque, error, short,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;

struct Rows {
    items: Vec<ComparisonRow>,
    more: bool,
    tally: Tally,
}
#[derive(Default)]
pub(super) struct Tally {
    pub changed: usize,
    pub unknown: usize,
    pub excluded: usize,
}
impl Rows {
    fn push(
        &mut self,
        section: &str,
        field: &str,
        state: ComparisonState,
        left: impl AsRef<str>,
        right: impl AsRef<str>,
        note: &str,
    ) {
        match state {
            ComparisonState::Different => self.tally.changed += 1,
            ComparisonState::Unavailable
            | ComparisonState::Incomplete
            | ComparisonState::NotCompared => self.tally.unknown += 1,
            ComparisonState::Excluded => self.tally.excluded += 1,
            _ => {}
        }
        let row = ComparisonRow {
            index: self.items.len(),
            section: section.into(),
            field: short(field),
            state,
            left: short(left.as_ref()),
            right: short(right.as_ref()),
            note: short(note),
        };
        if self.items.len() < MAX_ROWS {
            self.items.push(row);
        } else {
            self.more = true;
            if state == ComparisonState::Different
                && !self
                    .items
                    .iter()
                    .any(|r| r.state == ComparisonState::Different)
            {
                let mut row = row;
                row.index = MAX_ROWS - 1;
                self.items[MAX_ROWS - 1] = row;
            }
        }
    }
    fn value(&mut self, section: &str, field: &str, left: Option<&str>, right: Option<&str>) {
        self.push(
            section,
            field,
            match (left, right) {
                (Some(a), Some(b)) if a == b => ComparisonState::Equal,
                (Some(_), Some(_)) => ComparisonState::Different,
                _ => ComparisonState::Unavailable,
            },
            left.unwrap_or("Unavailable"),
            right.unwrap_or("Unavailable"),
            "",
        );
    }
}

#[allow(clippy::too_many_lines)] // Ordered comparison stages share one bounded row collector.
pub(super) async fn compare(
    left: &Evidence,
    right: &Evidence,
    options: &ComparisonOptions,
    op: &Operation,
) -> Result<(Vec<ComparisonRow>, bool, Tally), AppError> {
    let mut rows = Rows {
        items: vec![],
        more: false,
        tally: Tally::default(),
    };
    for (name, a, b) in [
        ("Request", left.request.as_ref(), right.request.as_ref()),
        ("Response", left.response.as_ref(), right.response.as_ref()),
    ] {
        op.check()?;
        if name == "Request" && (!left.exchange || !right.exchange) {
            rows.push(
                name,
                "Boundary",
                ComparisonState::NotApplicable,
                "Response-only comparison",
                "Response-only comparison",
                "An asset, draft or preview response has no request evidence to compare",
            );
            continue;
        }
        let (Some(a), Some(b)) = (a, b) else {
            rows.push(
                name,
                "Boundary",
                if name == "Request" && !left.exchange && !right.exchange {
                    ComparisonState::NotApplicable
                } else {
                    ComparisonState::Unavailable
                },
                a.map_or("Unavailable / not applicable", |m| &m.boundary),
                b.map_or("Unavailable / not applicable", |m| &m.boundary),
                "A missing stage is never replaced by another boundary",
            );
            continue;
        };
        if !a.available || !b.available {
            rows.push(name,if name=="Request"{"Method and URL"}else{"Status"},ComparisonState::Unavailable,&a.first,&b.first,"First-line input is unavailable, over budget or an invalid proposal; shown text is not a generated response");
        } else {
            rows.value(
                name,
                if name == "Request" {
                    "Method and URL"
                } else {
                    "Status"
                },
                a.available.then_some(a.first.as_str()),
                b.available.then_some(b.first.as_str()),
            );
        }
        if left.exchange && right.exchange {
            rows.value(
                name,
                "Protocol",
                a.protocol.as_deref(),
                b.protocol.as_deref(),
            );
        }
        headers(name, &a.headers, &b.headers, options, op, &mut rows)?;
        match (&a.trailers, &b.trailers) {
            (Some(left), Some(right))
                if left.iter().next().is_some() || right.iter().next().is_some() =>
            {
                headers(
                    &format!("{name} trailers"),
                    left,
                    right,
                    options,
                    op,
                    &mut rows,
                )?;
            }
            _ if a
                .trailers
                .as_ref()
                .is_some_and(|h| h.iter().next().is_some())
                || b.trailers
                    .as_ref()
                    .is_some_and(|h| h.iter().next().is_some()) =>
            {
                rows.push(
                    name,
                    "Trailers",
                    ComparisonState::NotCompared,
                    "Trailer evidence incomplete",
                    "Trailer evidence incomplete",
                    "One side lacks comparable trailer observations; open original source evidence",
                );
            }
            _ => {}
        }

        bodies(name, &a.body, &b.body, options, op, &mut rows).await?;
    }
    if left.exchange && right.exchange && (left.outcome.is_some() || right.outcome.is_some()) {
        rows.value(
            "Overview",
            "Local network outcome",
            left.outcome.as_deref(),
            right.outcome.as_deref(),
        );
    }
    let names = left
        .timings
        .iter()
        .chain(&right.timings)
        .map(|(n, _)| n.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let names = if left.exchange && right.exchange {
        names
    } else {
        std::collections::BTreeSet::new()
    };
    if names.is_empty() {
        rows.push(
            "Timings",
            "Measured intervals",
            if !left.exchange || !right.exchange {
                ComparisonState::NotApplicable
            } else {
                ComparisonState::Unavailable
            },
            "Unavailable / not applicable",
            "Unavailable / not applicable",
            "See source timings for imported definitions; no duration inferred from wall clocks",
        );
    }
    for name in names {
        let a = left
            .timings
            .iter()
            .find(|(n, _)| n == &name)
            .map(|(_, n)| *n);
        let b = right
            .timings
            .iter()
            .find(|(n, _)| n == &name)
            .map(|(_, n)| *n);
        rows.push("Timings",&name,match(a,b){(Some(a),Some(b))if a==b=>ComparisonState::Equal,(Some(_),Some(_))=>ComparisonState::Different,_=>ComparisonState::Unavailable},a.map_or_else(||"Unavailable".into(),|n|format!("{n} µs")),b.map_or_else(||"Unavailable".into(),|n|format!("{n} µs")),&match(a,b){(Some(a),Some(b))=>format!("Delta {} µs; intervals may overlap; local queue completion is not remote receipt",i128::from(b)-i128::from(a)),_=>"No comparable measurement".into()});
    }
    stream_metadata(left, right, &mut rows);
    for evidence in [left, right] {
        for note in &evidence.side.notes {
            rows.push(
                "Source/fidelity",
                &evidence.side.label,
                ComparisonState::NotApplicable,
                "",
                "",
                note,
            );
        }
        for note in &evidence.omissions {
            rows.push(
                "Source/fidelity",
                &evidence.side.label,
                ComparisonState::NotCompared,
                "",
                "",
                note,
            );
        }
    }
    // Bound the serialized bridge page independently from source and hunk work.
    while serde_json::to_vec(&rows.items).map_or(usize::MAX, |b| b.len()) > 1024 * 1024 {
        rows.items.pop();
        rows.more = true;
    }
    Ok((rows.items, rows.more, rows.tally))
}

#[allow(clippy::too_many_lines)] // Exact-value alignment, occurrence accounting and summary remain together.
fn headers(
    section: &str,
    left: &HeaderBlock,
    right: &HeaderBlock,
    options: &ComparisonOptions,
    op: &Operation,
    rows: &mut Rows,
) -> Result<(), AppError> {
    let mut groups = BTreeMap::<
        Vec<u8>,
        (
            Vec<(usize, &transmog_core::HeaderField)>,
            Vec<(usize, &transmog_core::HeaderField)>,
        ),
    >::new();
    for (is_left, block) in [(true, left), (false, right)] {
        for (index, field) in block.iter().enumerate() {
            op.check()?;
            let key = field.name().to_ascii_lowercase();
            let pair = groups.entry(key).or_default();
            if is_left {
                pair.0.push((index, field));
            } else {
                pair.1.push((index, field));
            }
        }
    }
    let mut changed = false;
    let mut unknown = false;
    for (name, (a, b)) in groups {
        let name = String::from_utf8_lossy(&name);
        if options
            .excluded_headers
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&name))
        {
            rows.push(
                section,
                &format!("Header {name}"),
                ComparisonState::Excluded,
                format!("{} fields", a.len()),
                format!("{} fields", b.len()),
                "Explicit header exclusion; original evidence remains unchanged",
            );
            continue;
        }
        // Match identical names and values before spelling changes, then pair remaining
        // occurrences in order. Position alone does not make a header different.
        let mut exact = HashMap::<_, VecDeque<usize>>::new();
        for (j, (_, f)) in b.iter().enumerate() {
            if !f.is_redacted() {
                exact.entry((f.name(), f.value())).or_default().push_back(j);
            }
        }
        let mut matched = vec![None; a.len()];
        let mut used = HashSet::new();
        for (i, (_, f)) in a.iter().enumerate() {
            if !f.is_redacted()
                && let Some(j) = exact
                    .get_mut(&(f.name(), f.value()))
                    .and_then(VecDeque::pop_front)
            {
                matched[i] = Some(j);
                used.insert(j);
            }
        }
        let mut values = HashMap::<_, VecDeque<usize>>::new();
        for (j, (_, f)) in b.iter().enumerate() {
            if !used.contains(&j) && !f.is_redacted() {
                values.entry(f.value()).or_default().push_back(j);
            }
        }
        for (i, (_, f)) in a.iter().enumerate() {
            if matched[i].is_none()
                && !f.is_redacted()
                && let Some(j) = values.get_mut(f.value()).and_then(VecDeque::pop_front)
            {
                matched[i] = Some(j);
                used.insert(j);
            }
        }
        let mut remaining = (0..b.len())
            .filter(|j| !used.contains(j))
            .collect::<VecDeque<_>>();
        for counterpart in &mut matched {
            if counterpart.is_none() {
                *counterpart = remaining.pop_front();
            }
        }
        for (i, (index, f)) in a.iter().enumerate() {
            op.check()?;
            let other = matched[i].map(|j| b[j]);
            let (state, note) = match other {
                Some((_, g)) if f.is_redacted() || g.is_redacted() => {
                    unknown = true;
                    (
                        ComparisonState::Unavailable,
                        "Redacted values cannot establish equality; lengths and presence only",
                    )
                }
                Some((_, g)) if f.value() == g.value() => {
                    if f.name() == g.name() {
                        continue;
                    }
                    changed = true;
                    (
                        ComparisonState::Different,
                        "Original field-name spelling changed; value unchanged",
                    )
                }
                _ => {
                    changed = true;
                    (
                        ComparisonState::Different,
                        "Every duplicate occurrence is retained separately",
                    )
                }
            };
            rows.push(
                section,
                &format!(
                    "Header {name} · left field {} / right field {}",
                    index + 1,
                    other.map_or_else(
                        || "absent".into(),
                        |(right_index, _)| (right_index + 1).to_string()
                    )
                ),
                state,
                header_value(f),
                other.map_or_else(|| "Absent".into(), |(_, g)| header_value(g)),
                note,
            );
        }
        let paired = matched.into_iter().flatten().collect::<HashSet<_>>();
        for (j, (index, f)) in b.iter().enumerate() {
            if !paired.contains(&j) {
                changed = true;
                rows.push(
                    section,
                    &format!("Header {name} · right field {}", index + 1),
                    ComparisonState::Different,
                    "Absent",
                    header_value(f),
                    "Added occurrence",
                );
            }
        }
    }
    rows.push(section,"Header summary",if changed{ComparisonState::Different}else if unknown{ComparisonState::Unavailable}else{ComparisonState::Equal},format!("{} fields",left.iter().count()),format!("{} fields",right.iter().count()),"Case-insensitive name grouping; exact values, original spelling and duplicate counts compared; field order ignored; exclusions listed separately");
    Ok(())
}
fn header_value(f: &transmog_core::HeaderField) -> String {
    if f.is_redacted() {
        format!("[redacted; original length {:?}]", f.original_value_bytes())
    } else {
        format!(
            "{}: {}",
            String::from_utf8_lossy(f.name()),
            std::str::from_utf8(f.value()).map_or_else(
                |_| format!(
                    "hex {}",
                    super::hex_text(&f.value()[..f.value().len().min(1024)])
                ),
                str::to_owned
            )
        )
    }
}

#[allow(clippy::too_many_lines)] // Each representation explicitly preserves its own verdict and coverage.
async fn bodies(
    section: &str,
    a: &Body,
    b: &Body,
    options: &ComparisonOptions,
    op: &Operation,
    rows: &mut Rows,
) -> Result<(), AppError> {
    let describe = |body: &Body| {
        format!(
            "{} · content codings {:?} · charset {:?} · {} bytes · SHA-256 {} · {}",
            body.representation,
            body.codings,
            body.charset,
            body.length
                .map_or_else(|| "unknown".into(), |v| v.to_string()),
            body.digest.as_deref().unwrap_or("unavailable"),
            body.reason
        )
    };
    if a.path.is_none() || b.path.is_none() || !a.complete || !b.complete {
        rows.push(section,"Body",if a.complete&&b.complete{ComparisonState::NotCompared}else{ComparisonState::Incomplete},describe(a),describe(b),"Unavailable/incomplete evidence is not empty. Open the source for retained prefix/ranges or Save as.");
        if let (Some(left), Some(right)) = (&a.path, &b.path)
            && a.representation == b.representation
        {
            let length = a
                .length
                .unwrap_or(0)
                .min(b.length.unwrap_or(0))
                .min(64 * 1024);
            if length > 0 {
                let read = |path: &std::path::Path| -> Result<Vec<u8>, AppError> {
                    let file = std::fs::File::open(path)
                        .map_err(|_| error("Frozen prefix unavailable"))?;
                    let mut bytes = Vec::new();
                    std::io::Read::take(file, length)
                        .read_to_end(&mut bytes)
                        .map_err(|_| error("Frozen prefix unreadable"))?;
                    Ok(bytes)
                };
                let left = read(left)?;
                let right = read(right)?;
                rows.push(section,"Retained prefix bytes (partial)",if left==right{ComparisonState::Equal}else{ComparisonState::Different},format!("Bytes 0..{length}"),format!("Bytes 0..{length}"),"Only the common retained prefix is compared. Matching prefixes do not establish complete-body equality; use Original byte ranges below.");
            }
        }
        return Ok(());
    }
    let same_rep = a.representation == b.representation;
    rows.push(
        section,
        "Exact body representation",
        if !same_rep {
            ComparisonState::NotCompared
        } else if a.length == b.length && a.digest == b.digest {
            ComparisonState::Equal
        } else {
            ComparisonState::Different
        },
        describe(a),
        describe(b),
        "Hashes apply only to complete bytes in the same representation; HTTP/TLS framing excluded",
    );
    if a.length.unwrap_or(u64::MAX) > MAX_TEXT as u64
        || b.length.unwrap_or(u64::MAX) > MAX_TEXT as u64
    {
        rows.push(
            section,
            "Decoded/text body",
            ComparisonState::NotCompared,
            "Over 16 MiB decoding input budget",
            "Over 16 MiB decoding input budget",
            "Use paged original byte ranges; full source Hex/Save as remains available",
        );
        return Ok(());
    }
    let decode = async |body: &Body| {
        let bytes = std::fs::read(body.path.as_ref().expect("checked path"))
            .map_err(|_| error("Frozen bytes unreadable"))?;
        crate::inspector::decode_content(&body.codings, bytes).await
    };
    let (Ok(a_bytes), Ok(b_bytes)) = (decode(a).await, decode(b).await) else {
        rows.push(
            section,
            "Decoded body",
            ComparisonState::Unavailable,
            "Decode unavailable",
            "Decode unavailable",
            "Unsupported, invalid or over-budget content coding; original bytes remain available",
        );
        return Ok(());
    };
    op.check()?;
    rows.push(
        section,
        "Decoded bytes",
        if a_bytes == b_bytes {
            ComparisonState::Equal
        } else {
            ComparisonState::Different
        },
        format!("{} bytes · {:x}", a_bytes.len(), Sha256::digest(&a_bytes)),
        format!("{} bytes · {:x}", b_bytes.len(), Sha256::digest(&b_bytes)),
        "Decoded bytes may be equal despite different content coding",
    );
    let (Some(mut left), Some(mut right)) = (
        crate::inspector::decode_unicode(&a_bytes, a.charset.as_deref()),
        crate::inspector::decode_unicode(&b_bytes, b.charset.as_deref()),
    ) else {
        rows.push(
            section,
            "Text body",
            ComparisonState::NotApplicable,
            "Binary / unsupported charset",
            "Binary / unsupported charset",
            "Use original byte ranges; replacement-character decoding cannot prove text equality",
        );
        return Ok(());
    };
    if options.normalize_newlines {
        left = left.replace("\r\n", "\n").replace('\r', "\n");
        right = right.replace("\r\n", "\n").replace('\r', "\n");
    }
    rows.push(
        section,
        "Decoded text",
        if left == right {
            ComparisonState::Equal
        } else {
            ComparisonState::Different
        },
        format!("{} Unicode characters", left.chars().count()),
        format!("{} Unicode characters", right.chars().count()),
        if options.normalize_newlines {
            "Derived view: CRLF/CR normalized to LF; exact decoded bytes above remain authoritative"
        } else {
            "Faithful Unicode decoding; no whitespace, newline or JSON normalization"
        },
    );
    if left != right {
        text_hunks(section, &left, &right, op, rows)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Bounded alignment and context emit one coherent hunk stream.
fn text_hunks(
    section: &str,
    left: &str,
    right: &str,
    op: &Operation,
    rows: &mut Rows,
) -> Result<(), AppError> {
    let left_lines = left.split_inclusive('\n').take(200_001).collect::<Vec<_>>();
    let right_lines = right
        .split_inclusive('\n')
        .take(200_001)
        .collect::<Vec<_>>();
    if left_lines.len() > 200_000 || right_lines.len() > 200_000 {
        rows.push(
            section,
            "Text changes",
            ComparisonState::NotCompared,
            "Over 200,000 line budget",
            "Over 200,000 line budget",
            "Exact equality classification is retained; use paged original bytes",
        );
        return Ok(());
    }
    let (mut left_index, mut right_index, mut hunks) = (0, 0, 0);
    while left_index < left_lines.len() || right_index < right_lines.len() {
        op.check()?;
        if left_index < left_lines.len()
            && right_index < right_lines.len()
            && left_lines[left_index] == right_lines[right_index]
        {
            left_index += 1;
            right_index += 1;
            continue;
        }
        if hunks >= 128 {
            rows.more = true;
            break;
        }
        hunks += 1;
        let mut left_count = usize::from(left_index < left_lines.len());
        let mut right_count = usize::from(right_index < right_lines.len());
        if left_index < left_lines.len() && right_index < right_lines.len() {
            let ahead_b = (1..=32).find(|distance| {
                right_index + distance < right_lines.len()
                    && left_lines[left_index] == right_lines[right_index + distance]
            });
            let ahead_a = (1..=32).find(|distance| {
                left_index + distance < left_lines.len()
                    && left_lines[left_index + distance] == right_lines[right_index]
            });
            match (ahead_a, ahead_b) {
                (Some(distance), None) => {
                    left_count = distance;
                    right_count = 0;
                }
                (None, Some(distance)) => {
                    left_count = 0;
                    right_count = distance;
                }
                (Some(left_distance), Some(right_distance)) if left_distance < right_distance => {
                    left_count = left_distance;
                    right_count = 0;
                }
                (Some(_), Some(right_distance)) => {
                    left_count = 0;
                    right_count = right_distance;
                }
                _ => {}
            }
        }
        let mut left_context = left_lines[left_index..left_index + left_count]
            .iter()
            .take(8)
            .copied()
            .collect::<String>();
        let mut right_context = right_lines[right_index..right_index + right_count]
            .iter()
            .take(8)
            .copied()
            .collect::<String>();
        if left_count == 1 && right_count == 1 {
            let prefix = left_context
                .chars()
                .zip(right_context.chars())
                .take_while(|(left_lines, right_lines)| left_lines == right_lines)
                .count();
            let start = prefix.saturating_sub(80);
            left_context = left_context.chars().skip(start).take(512).collect();
            right_context = right_context.chars().skip(start).take(512).collect();
            rows.push(section,&format!("Body change {hunks} · lines {} / {} · character {}",left_index+1,right_index+1,prefix+1),ComparisonState::Different,&left_context,&right_context,"Bounded context around the first changed character; original bytes remain available");
        } else {
            rows.push(
                section,
                &format!(
                    "Body change {hunks} · lines {} / {}",
                    left_index + 1,
                    right_index + 1
                ),
                ComparisonState::Different,
                left_context,
                right_context,
                "Deterministic line alignment with bounded context and 32-line lookahead",
            );
        }
        left_index += left_count;
        right_index += right_count;
    }
    Ok(())
}

fn stream_metadata(left: &Evidence, right: &Evidence, rows: &mut Rows) {
    if !left.exchange || !right.exchange || (!left.stream_indicated && !right.stream_indicated) {
        return;
    }
    rows.push("Overview","Stream messages/events",ComparisonState::NotCompared,"Payloads not compared","Payloads not compared","HTTP handshakes and available stream metadata are compared; open the source stream inspector for message/event content");
    let (Some(left), Some(right)) = (&left.stream, &right.stream) else {
        rows.push(
            "Overview",
            "Stream metadata",
            ComparisonState::Unavailable,
            "Unavailable on at least one side",
            "Unavailable on at least one side",
            "Missing stream observations do not establish absence",
        );
        return;
    };
    rows.value(
        "Overview",
        "Stream protocol",
        Some(&format!("{:?}", left.state.protocol)),
        Some(&format!("{:?}", right.state.protocol)),
    );
    rows.value(
        "Overview",
        "Stream lifecycle",
        Some(&format!("{:?}", left.state.status)),
        Some(&format!("{:?}", right.state.status)),
    );
    for (field, a, b) in [
        ("Observed stream records", left.observed, right.observed),
        ("Evicted stream records", left.evicted, right.evicted),
        (
            "Missing stream records",
            left.sequence_loss,
            right.sequence_loss,
        ),
        (
            "Decoded client message bytes",
            left.client_bytes,
            right.client_bytes,
        ),
        (
            "Decoded server message bytes",
            left.server_bytes,
            right.server_bytes,
        ),
    ] {
        rows.value(
            "Overview",
            field,
            Some(&a.to_string()),
            Some(&b.to_string()),
        );
    }
    rows.value(
        "Overview",
        "Retained stream records",
        Some(&left.retained.to_string()),
        Some(&right.retained.to_string()),
    );
}
