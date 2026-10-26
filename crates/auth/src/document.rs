//! Internal decoded material. This type intentionally has no Serialize implementation.
//! Inspection exposes masked summaries; the scratch adapter explicitly exposes full values.
use crate::{Finding, Inspection, Source, json::Node};

pub(crate) struct DecodedValue {
    pub name: String,
    pub value: Node,
    pub sensitive: bool,
    pub note: String,
}

pub(crate) struct DecodedDocument {
    pub format: String,
    pub role: String,
    pub recognition: String,
    pub status: String,
    pub verification: String,
    pub source: Source,
    pub values: Vec<DecodedValue>,
    pub content: Vec<(String, Node)>,
    pub children: Vec<Self>,
    pub diagnostics: Vec<String>,
}

impl DecodedDocument {
    pub fn from_inspection(mut inspection: Inspection) -> Vec<Self> {
        fn take(finding: Finding, values: &mut [String]) -> DecodedDocument {
            DecodedDocument {
                format: finding.format,
                role: finding.role,
                recognition: finding.recognition,
                status: finding.status,
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
            }
        }
        inspection
            .findings
            .into_iter()
            .map(|finding| take(finding, &mut inspection.values))
            .collect()
    }

    /// Credential-bearing scratch JSON, requested explicitly by the converter.
    pub fn scratch(self) -> Node {
        let text = |value| Node::Text(value);
        Node::Object(vec![
            ("format".into(), text(self.format)),
            ("role".into(), text(self.role)),
            ("recognition".into(), text(self.recognition)),
            ("status".into(), text(self.status)),
            ("verification".into(), text(self.verification)),
            (
                "values".into(),
                Node::Array(
                    self.values
                        .into_iter()
                        .map(|value| {
                            Node::Object(vec![
                                ("name".into(), text(value.name)),
                                ("value".into(), value.value),
                                (
                                    "sensitive".into(),
                                    Node::Scalar(value.sensitive.to_string()),
                                ),
                                ("note".into(), text(value.note)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("decoded".into(), Node::Object(self.content)),
            (
                "children".into(),
                Node::Array(self.children.into_iter().map(Self::scratch).collect()),
            ),
            (
                "diagnostics".into(),
                Node::Array(self.diagnostics.into_iter().map(text).collect()),
            ),
            (
                "provenance".into(),
                Node::Array(self.source.transforms.into_iter().map(text).collect()),
            ),
        ])
    }
}
