//! Internal decoded material. This type intentionally has no Serialize implementation.
//! Inspection exposes masked summaries; the scratch adapter explicitly exposes full values.
use crate::{Finding, Inspection, Source, json::Node};

pub(crate) struct DecodedValue {
    pub name: String,
    pub value: Node,
    pub sensitive: bool,
    pub note: String,
    pub technical: bool,
}

pub(crate) struct DecodedDocument {
    pub id: String,
    pub format: String,
    pub envelope: bool,
    pub role: String,
    pub recognition: String,
    pub status: String,
    pub complete: bool,
    pub verification: String,
    pub source: Source,
    pub values: Vec<DecodedValue>,
    pub content: Vec<(String, Node)>,
    pub children: Vec<Self>,
    pub diagnostics: Vec<String>,
    original_token: Option<String>,
    structured_values: bool,
}

impl DecodedDocument {
    pub fn from_inspection(mut inspection: Inspection) -> Vec<Self> {
        fn take(finding: Finding, values: &mut [String]) -> DecodedDocument {
            let original_token = finding
                .token_value
                .as_ref()
                .and_then(|id| id.strip_prefix('v'))
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| values.get(index))
                .cloned();
            DecodedDocument {
                id: finding.id,
                format: finding.format,
                envelope: finding.envelope,
                role: finding.role,
                recognition: finding.recognition,
                status: finding.status,
                complete: finding.complete,
                verification: finding.verification,
                source: finding.source,
                values: finding
                    .fields
                    .into_iter()
                    .filter_map(|field| {
                        let index = field.id.strip_prefix('v')?.parse::<usize>().ok()?;
                        Some(DecodedValue {
                            name: field.name,
                            value: Node::Text(std::mem::take(values.get_mut(index)?)),
                            sensitive: field.sensitive,
                            note: field.note,
                            technical: field.technical,
                        })
                    })
                    .collect(),
                content: finding.content,
                children: finding
                    .children
                    .into_iter()
                    .map(|child| take(child, values))
                    .collect(),
                diagnostics: finding.diagnostics,
                original_token,
                structured_values: false,
            }
        }
        inspection
            .findings
            .into_iter()
            .map(|finding| take(finding, &mut inspection.values))
            .collect()
    }

    /// Credential-bearing scratch JSON, requested explicitly by the converter.
    pub fn scratch(mut self) -> Node {
        let mut layers = vec![];
        // The same protocol-owned envelope fact and original-range criteria drive Auth presentation.
        // Siblings and different ranges are independent artifacts; never promote across them.
        while self.envelope
            && self.children.len() == 1
            && self.source.same_range(&self.children[0].source)
        {
            let Some(child) = self.children.pop() else {
                break;
            };
            layers.push(self);
            self = child;
        }
        let mut output = vec![
            ("format".into(), Node::Text(self.format.clone())),
            ("status".into(), Node::Text(self.status.clone())),
        ];
        // These are typed parser content members, never captured field labels or inferred roles.
        let payload = ["payload", "payloadText", "payloadBytesBase64"]
            .iter()
            .find(|key| self.content.iter().any(|(name, _)| name == **key))
            .copied();
        let parameters = self.content.iter().any(|(name, value)| {
            name == "parameters"
                && (self.format == "Digest"
                    || matches!(value, Node::Array(values) if !values.is_empty()))
        });
        if let Some(key) = payload {
            self.structured_values = true;
            self.take_content(key, &mut output);
        } else if parameters {
            self.structured_values = true;
            self.take_content("parameters", &mut output);
        } else {
            self.take_content("header", &mut output);
            let values: Vec<_> = self
                .values
                .iter()
                .filter(|value| !value.technical)
                .map(|value| (value.name.clone(), value.value.clone()))
                .collect();
            if !values.is_empty() {
                output.push(("fields".into(), Node::Object(values)));
            }
        }
        output.push(("verification".into(), Node::Text(self.verification.clone())));
        if !self.children.is_empty() {
            output.push((
                "children".into(),
                Node::Array(
                    std::mem::take(&mut self.children)
                        .into_iter()
                        .map(Self::scratch)
                        .collect(),
                ),
            ));
        }
        layers.push(self);
        let diagnostics: Vec<_> = layers
            .iter()
            .filter(|layer| !layer.diagnostics.is_empty())
            .map(|layer| {
                Node::Object(vec![
                    ("layer".into(), Node::Text(layer.id.clone())),
                    ("format".into(), Node::Text(layer.format.clone())),
                    (
                        "messages".into(),
                        Node::Array(layer.diagnostics.iter().cloned().map(Node::Text).collect()),
                    ),
                ])
            })
            .collect();
        if !diagnostics.is_empty() {
            output.push(("diagnostics".into(), Node::Array(diagnostics)));
        }
        output.push((
            "details".into(),
            Node::Object(vec![(
                "layers".into(),
                Node::Array(layers.into_iter().map(Self::details).collect()),
            )]),
        ));
        Node::Object(output)
    }

    fn take_content(&mut self, key: &str, output: &mut Vec<(String, Node)>) {
        if let Some(index) = self.content.iter().position(|(name, _)| name == key) {
            output.push(self.content.remove(index));
        }
    }

    fn details(self) -> Node {
        let mut output = vec![
            ("id".into(), Node::Text(self.id)),
            ("format".into(), Node::Text(self.format)),
            ("role".into(), Node::Text(self.role)),
            ("status".into(), Node::Text(self.status)),
            ("recognition".into(), Node::Text(self.recognition)),
            ("verification".into(), Node::Text(self.verification)),
            ("source".into(), source(self.source)),
        ];
        if let Some(original) = self.original_token.filter(|original| {
            !self
                .values
                .iter()
                .any(|value| value.value.string() == Some(original))
        }) {
            output.push(("originalToken".into(), Node::Text(original)));
        }
        if !self.content.is_empty() {
            output.push(("decoded".into(), Node::Object(self.content)));
        }
        let notes: Vec<_> = self
            .values
            .iter()
            .filter(|value| !value.note.is_empty())
            .map(|value| (value.name.clone(), Node::Text(value.note.clone())))
            .collect();
        let sensitive: Vec<_> = self
            .values
            .iter()
            .filter(|value| value.sensitive)
            .map(|value| Node::Text(value.name.clone()))
            .collect();
        let values: Vec<_> = self
            .values
            .into_iter()
            .filter(|value| value.technical || !self.structured_values)
            .map(|value| (value.name, value.value))
            .collect();
        if !values.is_empty() {
            // Objects retain ordering and duplicate member names in the lossless JSON writer.
            output.push(("values".into(), Node::Object(values)));
        }
        if !notes.is_empty() {
            output.push(("notes".into(), Node::Object(notes)));
        }
        if !sensitive.is_empty() {
            output.push(("sensitiveFields".into(), Node::Array(sensitive)));
        }
        Node::Object(output)
    }
}

fn source(source: Source) -> Node {
    let optional = |value: Option<usize>| {
        Node::Scalar(value.map_or_else(|| "null".into(), |value| value.to_string()))
    };
    Node::Object(vec![
        ("kind".into(), Node::Text(source.kind)),
        ("label".into(), Node::Text(source.label)),
        (
            "occurrence".into(),
            Node::Scalar(source.occurrence.to_string()),
        ),
        ("headerIndex".into(), optional(source.header_index)),
        ("offset".into(), optional(source.offset)),
        ("length".into(), optional(source.length)),
        (
            "transforms".into(),
            Node::Array(source.transforms.into_iter().map(Node::Text).collect()),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Finding};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn promotion_requires_an_envelope_one_child_and_the_same_original_range() {
        for (envelope, child_count, different_range, promoted) in [
            (true, 1, false, true),
            (false, 1, false, false),
            (true, 2, false, false),
            (true, 1, true, false),
        ] {
            let flag = AtomicBool::new(false);
            let mut context = Context::new(&flag);
            let source = Source {
                kind: "header".into(),
                label: "original".into(),
                offset: Some(0),
                length: Some(10),
                ..Source::default()
            };
            let mut root = Finding::new("Bearer", "unknown", source.clone());
            root.envelope = envelope;
            root.status = "opaque".into();
            root.diagnostics.push("Outer observation".into());
            for _ in 0..child_count {
                let mut source = source.clone();
                if different_range {
                    source.offset = Some(1);
                }
                source.transforms.push("Inner decoder".into());
                let mut child = Finding::new("JWS", "compact token", source);
                child.content.push((
                    "payload".into(),
                    Node::Object(vec![("sub".into(), Node::Text("reader".into()))]),
                ));
                root.children.push(child);
            }
            context.push(root);
            let document = DecodedDocument::from_inspection(context.result)
                .pop()
                .unwrap()
                .scratch();
            let document: serde_json::Value = serde_json::from_str(&document.pretty()).unwrap();
            assert_eq!(document["format"], if promoted { "JWS" } else { "Bearer" });
            assert_eq!(
                document["status"],
                if promoted { "decoded" } else { "opaque" }
            );
            assert_eq!(document["details"]["layers"][0]["role"], "unknown");
            assert_eq!(document["diagnostics"][0]["format"], "Bearer");
            assert_eq!(
                document["diagnostics"][0]["messages"][0],
                "Outer observation"
            );
            if promoted {
                assert_eq!(document["payload"]["sub"], "reader");
                assert_eq!(
                    document["details"]["layers"][1]["source"]["transforms"][0],
                    "Inner decoder"
                );
            } else {
                assert_eq!(document["children"].as_array().unwrap().len(), child_count);
            }
        }
    }
}
