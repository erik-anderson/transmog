//! Native comparison over frozen snapshots, using the ordinary isolated format readers.
use super::{AppError, Application, CancelOnDrop, Operation, error, token};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use transmog_extension_registry::Registry;
use transmog_extensions::{
    BodyReference, BytePage, Diagnostic, Direction, Invocation, Limits, MessageInput, Node,
    ParseStatus, Presentation, Value, VisualizationDocument, VisualizationInput, VisualizationStep,
};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_FINDINGS: usize = 512;

/// A native visualization; no extension identity, live worker or executable UI is exposed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredComparison {
    /// Identity of this frozen result, independent of later comparison choices.
    pub id: String,
    /// Uses exactly the same inert presentation contract as extension results.
    pub document: VisualizationDocument,
}

struct Worker {
    registry: Registry,
    owner: String,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    finished: bool,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !self.finished {
            self.canceled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.registry.deactivate_owner(&self.owner);
    }
}

impl Application {
    /// Compares fields from two frozen bodies through an explicitly selected enabled reader.
    /// # Errors
    /// Returns expired snapshots, unavailable codecs/readers, cancellation or bounded-work errors.
    pub async fn comparison_fields(
        &self,
        id: &str,
        section: &str,
        format: &str,
        operation_id: &str,
        page_id: &str,
    ) -> Result<StructuredComparison, AppError> {
        if !["protobuf", "bond", "msgpack", "cbor", "bson"].contains(&format) {
            return Err(error("Choose a supported binary format"));
        }
        let snapshot = self.comparisons.get(id)?;
        let messages = match section {
            "Request" => [
                snapshot.left.request.as_ref(),
                snapshot.right.request.as_ref(),
            ],
            "Response" => [
                snapshot.left.response.as_ref(),
                snapshot.right.response.as_ref(),
            ],
            _ => return Err(error("Choose Request or Response")),
        };
        let op = self.comparisons.begin(&format!("fields:{operation_id}"))?;
        let _cancel = CancelOnDrop(op.canceled.clone());
        if !self.visualizer_snapshot_current(page_id) {
            return Err(error("The comparison page is hidden, closed or expired"));
        }
        let mut parsed = Vec::new();
        for message in messages {
            op.check()?;
            let message =
                message.ok_or_else(|| error("The selected body boundary is unavailable"))?;
            let body = &message.body;
            if !body.complete || body.length.is_none_or(|size| size > MAX_BYTES as u64) {
                return Err(error(
                    "Field comparison needs complete bodies up to 2 MiB per side; original evidence remains available",
                ));
            }
            let path = body
                .path
                .clone()
                .ok_or_else(|| error("Frozen body bytes unavailable"))?;
            let encoded = tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let file =
                    std::fs::File::open(path).map_err(|_| error("Frozen body unavailable"))?;
                let mut bytes = Vec::new();
                file.take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| error("Frozen body unreadable"))?;
                Ok::<_, AppError>(bytes)
            })
            .await
            .map_err(|_| error("Frozen body read failed"))??;
            if encoded.len() > MAX_BYTES || Some(encoded.len() as u64) != body.length {
                return Err(error(
                    "Frozen body length changed or exceeded the field comparison limit",
                ));
            }
            if body.representation == "encoded entity bytes" {
                // Validate the original occurrences before normalized codec metadata is used.
                if message.headers.iter().any(|field| {
                    field.name().eq_ignore_ascii_case(b"content-encoding") && field.is_redacted()
                }) {
                    return Err(error("Content-coding metadata is redacted"));
                }
                transmog_content::ContentCodingStack::from_headers(
                    &message.headers,
                    transmog_content::ContentLimits::default().max_coding_layers(),
                )
                .map_err(|_| error("Content-coding metadata is invalid or unsupported"))?;
            }
            let bytes = crate::visualizers::decoding::decode(&body.codings, encoded, MAX_BYTES, MAX_BYTES)
                .await.map_err(|_| error("HTTP decoding failed or exceeded the field comparison limit; encoded bytes were not interpreted"))?;
            op.check()?;
            let document = parse(self.extensions(), format, &bytes, &message.headers, &op).await?;
            parsed.push((document, bytes));
        }
        let (left, left_bytes) = &parsed[0];
        let (right, right_bytes) = &parsed[1];
        let document = compare(left, left_bytes, right, right_bytes, section, format)?;
        op.check()?;
        Ok(StructuredComparison {
            id: token()?,
            document,
        })
    }
}

async fn parse(
    registry: Registry,
    format: &str,
    bytes: &[u8],
    headers: &transmog_core::HeaderBlock,
    op: &Operation,
) -> Result<VisualizationDocument, AppError> {
    let owner = format!("comparison-{}", token()?);
    let mut worker = Worker {
        registry: registry.clone(),
        owner: owner.clone(),
        canceled: op.canceled.clone(),
        finished: false,
    };
    let contribution = format!("{format}-response");
    let startup = registry.clone();
    let startup_owner = owner.clone();
    let canceled = op.canceled.clone();
    let instance = tokio::task::spawn_blocking(move || {
        startup.activate_cancellable(&startup_owner, "net.transmog.binary", &contribution, || {
            canceled.load(std::sync::atomic::Ordering::Acquire)
        })
    })
    .await
    .map_err(|_| error("Format reader startup failed"))?
    .map_err(|failure| error(&failure.message))?;
    let runner = worker
        .registry
        .runner(&worker.owner, &instance.id)
        .map_err(|_| error("Format reader unavailable"))?;
    let input = reader_input(
        instance.id,
        bytes.len(),
        content_types(
            headers,
            instance
                .declaration
                .inputs
                .response
                .permits_header("content-type"),
        )?,
    );
    let mut invocation = Invocation::Visualize {
        input: Box::new(input),
    };
    for _ in 0..64 {
        op.check()?;
        let step = tokio::select! {
            result = runner.invoke(invocation) => result.map_err(|failure| error(&failure.message))?,
            failure = wait_canceled(op) => return Err(failure),
        };
        op.check()?;
        match step {
            VisualizationStep::Document { document } => {
                if !comparable(&document) {
                    return Err(error(
                        "Field comparison requires materialized fields over the frozen body; derived or deferred evidence remains available in its source viewer",
                    ));
                }
                worker.finished = true;
                return Ok(document);
            }
            VisualizationStep::NeedInput { reads, token } => {
                let mut pages = Vec::new();
                for read in reads {
                    let start = read
                        .offset
                        .parse::<usize>()
                        .map_err(|_| error("Invalid reader offset"))?;
                    let end = start
                        .checked_add(read.max_bytes)
                        .ok_or_else(|| error("Invalid reader range"))?;
                    if read.source_id != "body" || read.max_bytes > 65_536 || end > bytes.len() {
                        return Err(error("Format reader requested unavailable evidence"));
                    }
                    pages.push(BytePage {
                        source_id: read.source_id,
                        offset: read.offset,
                        bytes: bytes[start..end].to_vec(),
                    });
                }
                invocation = Invocation::Resume { token, pages };
            }
            _ => {
                return Err(error(
                    "This format reader requested evidence outside the frozen body",
                ));
            }
        }
    }
    Err(error("Format reader exceeded the continuation limit"))
}

fn reader_input(
    instance_id: String,
    byte_length: usize,
    headers: Vec<transmog_extensions::Header>,
) -> VisualizationInput {
    VisualizationInput {
        instance_id,
        exchange_id: "comparison".into(),
        request_number: "1".into(),
        generation: "1".into(),
        direction: Direction::Response,
        request: None,
        response: Some(MessageInput {
            evidence_id: "frozen".into(),
            headers,
            body: Some(BodyReference {
                source_id: "body".into(),
                length: byte_length.to_string(),
                availability: "complete".into(),
            }),
            method: None,
            path: None,
            status: None,
        }),
    }
}

struct Field<'a> {
    name: String,
    node: &'a Node,
    wire: String,
}

fn content_types(
    block: &transmog_core::HeaderBlock,
    permitted: bool,
) -> Result<Vec<transmog_extensions::Header>, AppError> {
    if !permitted {
        return Ok(vec![]);
    }
    let mut headers = Vec::new();
    let mut bytes = 0usize;
    for (index, field) in block
        .iter()
        .enumerate()
        .filter(|(_, field)| field.name().eq_ignore_ascii_case(b"content-type"))
    {
        bytes = bytes.saturating_add(field.value().len());
        if headers.len() >= 16 || bytes > 65_536 {
            return Err(error(
                "Content-type metadata exceeds the field comparison limit",
            ));
        }
        headers.push(transmog_extensions::Header {
            source_id: None,
            index: u32::try_from(index)
                .map_err(|_| error("Header index exceeds the field comparison limit"))?,
            name: String::from_utf8_lossy(field.name()).into_owned(),
            value: (!field.is_redacted()).then(|| field.value().to_vec()),
            redacted: field.is_redacted(),
            original_bytes: field
                .original_value_bytes()
                .map(|length| length.to_string()),
        });
    }
    Ok(headers)
}

fn comparable(document: &VisualizationDocument) -> bool {
    if !document.sources.is_empty() {
        return false;
    }
    let mut pending = document.roots.iter().collect::<Vec<_>>();
    while let Some(node) = pending.pop() {
        if node.children_token.is_some() || node.spans.iter().any(|span| span.source_id != "body") {
            return false;
        }
        pending.extend(&node.children);
    }
    true
}
async fn wait_canceled(op: &Operation) -> AppError {
    loop {
        if let Err(failure) = op.check() {
            return failure;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
fn wire(node: &Node, bytes: &[u8], header_only: bool) -> String {
    let mut hash = Sha256::new();
    for span in &node.spans {
        let start = span.offset.parse::<usize>().unwrap_or(usize::MAX);
        let mut length = span.length.parse::<usize>().unwrap_or(usize::MAX);
        if header_only
            && matches!(node.value, Some(Value::Object | Value::Array))
            && !node.children.is_empty()
            && let Some(first) = node
                .children
                .iter()
                .flat_map(|child| &child.spans)
                .filter(|child| child.source_id == span.source_id)
                .filter_map(|child| child.offset.parse::<usize>().ok())
                .min()
        {
            length = length.min(first.saturating_sub(start));
        }
        if let Some(end) = start.checked_add(length)
            && let Some(value) = bytes.get(start..end)
        {
            hash.update(value);
        }
    }
    hex::encode(hash.finalize())
}

fn flatten<'a>(
    nodes: &'a [Node],
    bytes: &[u8],
    path: &str,
    label: &str,
    result: &mut BTreeMap<String, Field<'a>>,
) {
    let mut occurrences = BTreeMap::<String, usize>::new();
    for node in nodes {
        let label_name = node.name.strip_prefix("Partial ").unwrap_or(&node.name);
        let key = node.children.iter().find(|child| child.name == "Key");
        let name = if let Some(key) = key {
            // Full encoded keys, including previewed/non-string keys, establish identity.
            format!("key:{}", wire(key, bytes, false))
        } else {
            label_name
                .split(" · ")
                .next()
                .unwrap_or(label_name)
                .to_owned()
        };
        let occurrence = occurrences.entry(name.clone()).or_default();
        *occurrence += 1;
        let next = format!("{path}/{name}#{}", *occurrence);
        let display = format!(
            "{label}/{} [{}]",
            label_name.split(" · ").next().unwrap_or(label_name),
            *occurrence
        );
        result.insert(
            next.clone(),
            Field {
                name: display.clone(),
                node,
                wire: wire(node, bytes, true),
            },
        );
        flatten(&node.children, bytes, &next, &display, result);
    }
}

fn node(id: String, name: String, value: Option<Value>, children: Vec<Node>) -> Node {
    Node {
        id,
        name,
        value,
        format: None,
        spans: vec![],
        children,
        children_token: None,
        presentation: None,
    }
}
fn field_value(field: Option<&Field<'_>>, complete: bool) -> Value {
    field.map_or_else(
        || {
            Value::String(
                if complete {
                    "Absent"
                } else {
                    "Unresolved; parse incomplete"
                }
                .into(),
            )
        },
        |field| {
            if matches!(field.node.value, Some(Value::Bytes)) {
                Value::String(format!("Bytes · SHA-256 {}", field.wire))
            } else if matches!(field.node.value, Some(Value::Object | Value::Array)) {
                Value::String(
                    if matches!(field.node.value, Some(Value::Array)) {
                        "Array (container)"
                    } else {
                        "Object (container)"
                    }
                    .into(),
                )
            } else {
                field.node.value.clone().unwrap_or(Value::Object)
            }
        },
    )
}

fn side_node(id: String, name: &str, field: Option<&Field<'_>>, complete: bool) -> Node {
    let mut result = node(id, name.into(), Some(field_value(field, complete)), vec![]);
    result.format = field.and_then(|field| field.node.format.clone());
    result
}

fn finding(
    index: usize,
    left: Option<&Field<'_>>,
    right: Option<&Field<'_>>,
    complete: bool,
) -> Node {
    let id = format!("field.{index}");
    let state = match (left, right, complete) {
        (None, _, true) => "added",
        (_, None, true) => "removed",
        (None, _, false) | (_, None, false) => "unresolved",
        _ => "changed",
    };
    let name = left.or(right).map_or("Field", |field| field.name.as_str());
    let source = |field: Option<&Field<'_>>| {
        field.map_or_else(
            || "No matched field".into(),
            |field| {
                field
                    .node
                    .spans
                    .iter()
                    .map(|span| format!("decoded offset {} · {} bytes", span.offset, span.length))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
        )
    };
    let reason = if left
        .zip(right)
        .is_some_and(|(a, b)| a.node.value == b.node.value && a.node.format == b.node.format)
    {
        "Wire encoding differs"
    } else {
        "Type, value or occurrence differs"
    };
    node(
        id.clone(),
        format!("{} · {state}", name.chars().take(100).collect::<String>()),
        None,
        vec![
            node(
                format!("{id}.path"),
                "Field path".into(),
                Some(Value::String(name.to_owned())),
                vec![],
            ),
            node(
                format!("{id}.reason"),
                "Change".into(),
                Some(Value::String(reason.into())),
                vec![],
            ),
            side_node(format!("{id}.left"), "Left", left, complete),
            side_node(format!("{id}.right"), "Right", right, complete),
            node(
                format!("{id}.source"),
                "Source ranges".into(),
                Some(Value::String(format!(
                    "Left: {}\nRight: {}",
                    source(left),
                    source(right)
                ))),
                vec![],
            ),
        ],
    )
}

fn compare(
    left: &VisualizationDocument,
    left_bytes: &[u8],
    right: &VisualizationDocument,
    right_bytes: &[u8],
    section: &str,
    format: &str,
) -> Result<VisualizationDocument, AppError> {
    let mut a = BTreeMap::new();
    let mut b = BTreeMap::new();
    flatten(&left.roots, left_bytes, "", "", &mut a);
    flatten(&right.roots, right_bytes, "", "", &mut b);
    let complete = left.status == ParseStatus::Complete && right.status == ParseStatus::Complete;
    let mut roots = Vec::new();
    let mut changes = 0;
    let mut equal = 0;
    for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        let left = a.get(key);
        let right = b.get(key);
        if left.zip(right).is_some_and(|(left, right)| {
            left.node.value == right.node.value
                && left.node.format == right.node.format
                && left.wire == right.wire
        }) {
            equal += 1;
            continue;
        }
        changes += 1;
        if roots.len() >= MAX_FINDINGS {
            continue;
        }
        roots.push(finding(roots.len(), left, right, complete));
    }
    let mut diagnostics = left
        .diagnostics
        .iter()
        .map(|item| ("Left", item))
        .chain(right.diagnostics.iter().map(|item| ("Right", item)))
        .take(32)
        .map(|(side, item)| Diagnostic {
            code: item.code.clone(),
            message: format!("{side}: {}", item.message),
            node_id: None,
        })
        .collect::<Vec<_>>();
    let limited = changes > MAX_FINDINGS
        || left.status == ParseStatus::Limited
        || right.status == ParseStatus::Limited;
    if changes > MAX_FINDINGS {
        diagnostics.push(Diagnostic { code: "finding-limit".into(), message: "Showing the first 512 findings; additional changes remain. Narrow the body or inspect the source.".into(), node_id: None });
    }
    if roots.is_empty() {
        roots.push(node(
            "equal".into(),
            if complete {
                "No field changes"
            } else {
                "No differences in parsed fields; coverage incomplete"
            }
            .into(),
            None,
            vec![],
        ));
    }
    let document = VisualizationDocument {
        title: format!(
            "{section} · {} field comparison",
            match format {
                "msgpack" => "MessagePack",
                "cbor" => "CBOR",
                "bson" => "BSON",
                "bond" => "Bond Compact Binary",
                _ => "Protobuf",
            }
        ),
        summary: Some(format!(
            "{changes} findings · {equal} equal fields. Left: {:?}; right: {:?}. Matches encoded keys/field labels and ordered duplicate occurrences. Values and wire encodings are compared; Possible interpretations remain alternatives. Source offsets refer to decoded snapshot bodies; original byte ranges remain in the comparison.",
            left.status, right.status
        )),
        status: if limited {
            ParseStatus::Limited
        } else if complete {
            ParseStatus::Complete
        } else {
            ParseStatus::Partial
        },
        updates: Some(transmog_extensions::UpdatePolicy::Final),
        presentation: Presentation::Tree,
        roots,
        sources: vec![],
        diagnostics,
    };
    document
        .validate(&BTreeMap::new(), Limits::default())
        .map_err(|_| error("Field comparison exceeded its presentation limit"))?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_extensions::Span;

    #[test]
    fn comparison_retains_only_declared_format_metadata_without_reapplying_codings() {
        let mut hidden =
            transmog_core::HeaderField::try_new("Content-Type", "application/cbor-seq").unwrap();
        hidden.redact_value();
        let block = transmog_core::HeaderBlock::from_fields(vec![
            transmog_core::HeaderField::try_new("Content-Type", "application/cbor-seq").unwrap(),
            transmog_core::HeaderField::try_new("Content-Encoding", "gzip").unwrap(),
            transmog_core::HeaderField::try_new("Authorization", "private").unwrap(),
            hidden,
        ]);
        assert_eq!(content_types(&block, false).unwrap(), []);
        let projected = content_types(&block, true).unwrap();
        assert_eq!(projected.len(), 2);
        assert_eq!(
            projected[0].value.as_deref(),
            Some(b"application/cbor-seq".as_slice())
        );
        assert!(projected[1].redacted && projected[1].value.is_none());
    }

    #[test]
    fn partial_containers_align_known_children_without_losing_changed_values() {
        let left = doc(
            vec![field(
                "Value 1",
                Value::Array,
                0,
                3,
                vec![field("Item 1", Value::Integer("1".into()), 1, 1, vec![])],
            )],
            ParseStatus::Complete,
        );
        let right = doc(
            vec![field(
                "Partial Value 1",
                Value::Array,
                0,
                3,
                vec![field("Item 1", Value::Integer("2".into()), 1, 1, vec![])],
            )],
            ParseStatus::Malformed,
        );
        let result = compare(
            &left,
            &[0x82, 1, 3],
            &right,
            &[0x82, 2, 255],
            "Response",
            "cbor",
        )
        .unwrap();
        assert_eq!(result.status, ParseStatus::Partial);
        assert!(
            result
                .roots
                .iter()
                .any(|node| node.name.contains("Item 1") && node.name.ends_with("changed"))
        );
    }
    #[test]
    fn finding_labels_keep_the_state_and_complete_path_when_shortened() {
        let name = "long".repeat(100);
        let left = doc(
            vec![field(&name, Value::Integer("1".into()), 0, 1, vec![])],
            ParseStatus::Complete,
        );
        let right = doc(
            vec![field(&name, Value::Integer("2".into()), 0, 1, vec![])],
            ParseStatus::Complete,
        );
        let result = compare(&left, &[1], &right, &[2], "Response", "protobuf").unwrap();
        assert!(result.roots[0].name.ends_with("changed"));
        assert!(
            matches!(&result.roots[0].children[0].value, Some(Value::String(value)) if value.contains(&name))
        );
    }
    #[test]
    fn parser_limits_remain_limited_in_native_findings() {
        let value = doc(vec![], ParseStatus::Limited);
        assert_eq!(
            compare(&value, &[], &value, &[], "Response", "bond")
                .unwrap()
                .status,
            ParseStatus::Limited
        );
    }
    #[test]
    fn deferred_or_foreign_sources_cannot_prove_field_equality() {
        let mut node = field("Field 1", Value::Object, 0, 1, vec![]);
        node.children_token = Some("deferred".into());
        assert!(!comparable(&doc(vec![node.clone()], ParseStatus::Complete)));
        node.children_token = None;
        node.spans[0].source_id = "derived".into();
        assert!(!comparable(&doc(vec![node], ParseStatus::Complete)));
    }

    fn field(name: &str, value: Value, offset: usize, length: usize, children: Vec<Node>) -> Node {
        let mut result = node(
            format!("n{offset}-{length}"),
            name.into(),
            Some(value),
            children,
        );
        result.spans.push(Span {
            source_id: "body".into(),
            offset: offset.to_string(),
            length: length.to_string(),
        });
        result
    }
    fn doc(roots: Vec<Node>, status: ParseStatus) -> VisualizationDocument {
        VisualizationDocument {
            title: "Test".into(),
            summary: None,
            status,
            updates: None,
            presentation: Presentation::Tree,
            roots,
            sources: vec![],
            diagnostics: vec![],
        }
    }
    fn entry(key: &str, offset: usize, value: i32) -> Node {
        field(
            &format!("Key {key}"),
            Value::Object,
            offset,
            3,
            vec![
                field("Key", Value::String(key.into()), offset, 2, vec![]),
                field(
                    "Value",
                    Value::Integer(value.to_string()),
                    offset + 2,
                    1,
                    vec![],
                ),
            ],
        )
    }
    fn map(entries: Vec<Node>, size: usize) -> VisualizationDocument {
        doc(
            vec![field("Value 1", Value::Object, 0, size, entries)],
            ParseStatus::Complete,
        )
    }
    #[test]
    fn matches_keys_across_reordering_and_keeps_duplicate_occurrence_order() {
        let left = map(vec![entry("a", 1, 1), entry("b", 4, 2)], 7);
        let right = map(vec![entry("b", 1, 2), entry("a", 4, 1)], 7);
        let result = compare(
            &left,
            &[0x82, 0x61, b'a', 1, 0x61, b'b', 2],
            &right,
            &[0x82, 0x61, b'b', 2, 0x61, b'a', 1],
            "Response",
            "msgpack",
        )
        .unwrap();
        assert_eq!(result.roots[0].name, "No field changes");
        let left = map(vec![entry("a", 1, 1), entry("a", 4, 2)], 7);
        let right = map(vec![entry("a", 1, 2), entry("a", 4, 1)], 7);
        let result = compare(
            &left,
            &[0x82, 0x61, b'a', 1, 0x61, b'a', 2],
            &right,
            &[0x82, 0x61, b'a', 2, 0x61, b'a', 1],
            "Response",
            "msgpack",
        )
        .unwrap();
        assert_eq!(result.roots.len(), 2);
        assert!(
            result
                .roots
                .iter()
                .any(|node| node.name.contains("Key a [2]"))
        );
    }
    #[test]
    fn comparison_covers_bytes_beyond_the_string_preview() {
        let document = doc(
            vec![field(
                "Field 1",
                Value::String("same preview".into()),
                0,
                2000,
                vec![],
            )],
            ParseStatus::Complete,
        );
        let a = vec![b'a'; 2000];
        let mut b = a.clone();
        b[1999] = b'b';
        let result = compare(&document, &a, &document, &b, "Response", "protobuf").unwrap();
        assert!(result.roots[0].name.ends_with("changed"));
    }
    #[test]
    fn incomplete_parse_never_proves_absence_or_whole_body_equality() {
        let left = doc(
            vec![field("Field 1", Value::Integer("1".into()), 0, 1, vec![])],
            ParseStatus::Complete,
        );
        let right = doc(vec![], ParseStatus::Malformed);
        let result = compare(&left, &[1], &right, &[255], "Response", "protobuf").unwrap();
        assert_eq!(result.status, ParseStatus::Partial);
        assert!(result.roots[0].name.ends_with("unresolved"));
        assert!(
            matches!(&result.roots[0].children.iter().find(|child| child.name == "Right").unwrap().value, Some(Value::String(value)) if value.contains("Unresolved"))
        );
    }
    #[test]
    fn empty_containers_and_wire_encoding_changes_are_findings() {
        let a = doc(
            vec![field("Value 1", Value::Array, 0, 1, vec![])],
            ParseStatus::Complete,
        );
        let b = doc(
            vec![field("Value 1", Value::Object, 0, 1, vec![])],
            ParseStatus::Complete,
        );
        assert!(
            compare(&a, &[0x90], &b, &[0x80], "Response", "msgpack")
                .unwrap()
                .roots[0]
                .name
                .ends_with("changed")
        );
        let a = doc(
            vec![field("Field 1", Value::Integer("1".into()), 0, 1, vec![])],
            ParseStatus::Complete,
        );
        let b = doc(
            vec![field("Field 1", Value::Integer("1".into()), 0, 2, vec![])],
            ParseStatus::Complete,
        );
        assert!(
            compare(&a, &[1], &b, &[0x81, 0], "Response", "protobuf")
                .unwrap()
                .roots[0]
                .name
                .ends_with("changed")
        );
    }
    #[test]
    fn collection_headers_and_full_non_string_keys_are_compared() {
        let key = |byte: i32| {
            field(
                "Key",
                Value::Array,
                1,
                2,
                vec![field(
                    "Item 1",
                    Value::Integer(byte.to_string()),
                    2,
                    1,
                    vec![],
                )],
            )
        };
        let a = field(
            "Entry 1",
            Value::Object,
            1,
            3,
            vec![
                key(1),
                field("Value", Value::Integer("3".into()), 3, 1, vec![]),
            ],
        );
        let b = field(
            "Entry 1",
            Value::Object,
            1,
            3,
            vec![
                key(2),
                field("Value", Value::Integer("3".into()), 3, 1, vec![]),
            ],
        );
        let result = compare(
            &map(vec![a], 4),
            &[0xa1, 0x81, 1, 3],
            &map(vec![b], 4),
            &[0xa1, 0x81, 2, 3],
            "Response",
            "cbor",
        )
        .unwrap();
        assert!(result.roots.iter().any(|node| node.name.ends_with("added")));
        assert!(
            result
                .roots
                .iter()
                .any(|node| node.name.ends_with("removed"))
        );
    }
    #[test]
    fn finding_limits_remain_explicit_after_counting_all_changes() {
        let roots = (0..600)
            .map(|index| {
                field(
                    &format!("Field {index}"),
                    Value::Integer("1".into()),
                    index,
                    1,
                    vec![],
                )
            })
            .collect();
        let a = doc(roots, ParseStatus::Complete);
        let b = doc(vec![], ParseStatus::Complete);
        let result = compare(&a, &vec![1; 600], &b, &[], "Response", "protobuf").unwrap();
        assert_eq!(result.roots.len(), 512);
        assert_eq!(result.status, ParseStatus::Limited);
        assert!(result.summary.unwrap().starts_with("600 findings"));
    }
    #[test]
    fn visibility_cancels_field_operations_without_canceling_ordinary_comparisons() {
        let registry = super::super::ComparisonRegistry::default();
        let fields = registry.begin("fields:abc").unwrap();
        let ordinary = registry.begin("ordinary").unwrap();
        registry.cancel_fields();
        assert!(fields.check().is_err());
        assert!(ordinary.check().is_ok());
    }
    #[test]
    fn cancellation_before_field_startup_is_not_forgotten() {
        let app = Application::new(crate::AppConfig::default()).unwrap();
        app.cancel_comparison("queued-fields");
        assert!(app.comparisons.begin("fields:queued-fields").is_err());
        assert!(app.comparisons.begin("ordinary").is_ok());
    }
}
