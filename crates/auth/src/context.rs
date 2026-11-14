//! Per-inspection retained values, structural output and cooperative cancellation.
use crate::limits::{
    MAX_COVERAGE_DETAIL_BYTES, MAX_COVERAGE_SOURCES, MAX_FIELD_NAME_BYTES, MAX_FINDINGS,
    MAX_PREVIEW_BYTES,
};
use crate::{Coverage, Field, Finding, Inspection, MAX_CANDIDATE, MAX_FIELDS, MAX_TOTAL, clip};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Context<'a> {
    pub(crate) result: Inspection,
    canceled: &'a AtomicBool,
    remaining_bytes: usize,
    pub(crate) captured_at: Option<i64>,
}
impl<'a> Context<'a> {
    pub(crate) fn new(canceled: &'a AtomicBool) -> Self {
        Self {
            result: Inspection::default(),
            canceled,
            remaining_bytes: MAX_TOTAL,
            captured_at: None,
        }
    }
    pub(crate) fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Relaxed)
    }
    pub(crate) fn reserve_bytes(&mut self, count: usize) -> bool {
        if self.is_canceled() {
            return false;
        }
        if count > self.remaining_bytes {
            self.limited();
            return false;
        }
        self.remaining_bytes -= count;
        true
    }
    pub(crate) fn limited(&mut self) {
        if self.result.diagnostics.is_empty() {
            self.result
                .diagnostics
                .push("Inspection reached a size or structure limit; coverage is partial.".into());
        }
    }
    pub(crate) fn token(&mut self, finding: &mut Finding, raw: &str, operation: &'static str) {
        if let Some(field) = finding
            .fields
            .iter()
            .find(|field| self.result.value(&field.id) == Some(raw))
        {
            finding.token_value = Some(field.id.clone());
            finding.converter_operation = Some(operation);
            return;
        }
        if raw.len() > MAX_CANDIDATE
            || self.result.values.len() >= MAX_FIELDS
            || raw.len() > self.remaining_bytes
        {
            // Optional handoff retention must not change the structural outcome of captured evidence.
            return;
        }
        if !self.reserve_bytes(raw.len()) {
            return;
        }
        finding.token_value = Some(format!("v{}", self.result.values.len()));
        finding.converter_operation = Some(operation);
        self.result.values.push(raw.into());
    }
    pub(crate) fn field(
        &mut self,
        finding: &mut Finding,
        name: &str,
        value: impl Into<String>,
        sensitive: bool,
        note: &str,
    ) {
        if self.result.values.len() >= MAX_FIELDS || self.is_canceled() {
            self.limited();
            finding.status = "limited".into();
            return;
        }
        let value = value.into();
        if value.len() > MAX_CANDIDATE || !self.reserve_bytes(value.len()) {
            finding.status = "limited".into();
            return;
        }
        let id = format!("v{}", self.result.values.len());
        let shown = if sensitive {
            "••••••••".into()
        } else {
            clip(&value, MAX_PREVIEW_BYTES)
        };
        self.result.values.push(value);
        finding.fields.push(Field {
            id,
            name: clip(name, MAX_FIELD_NAME_BYTES),
            value: shown,
            sensitive,
            available: true,
            note: note.into(),
            technical: false,
        });
    }
    /// Preserve a raw representation or secondary protocol metadata after readable scratch data.
    pub(crate) fn technical_field(
        &mut self,
        finding: &mut Finding,
        name: &str,
        value: impl Into<String>,
        sensitive: bool,
        note: &str,
    ) {
        let index = finding.fields.len();
        self.field(finding, name, value, sensitive, note);
        if let Some(field) = finding.fields.get_mut(index) {
            field.technical = true;
        }
    }
    pub(crate) fn push(&mut self, mut finding: Finding) {
        fn name_children(finding: &mut Finding) {
            for (index, child) in finding.children.iter_mut().enumerate() {
                child.id = format!("{}.{index}", finding.id);
                name_children(child);
            }
        }
        if self.result.findings.len() >= MAX_FINDINGS {
            self.limited();
            return;
        }
        finding.id = format!("f{}", self.result.findings.len());
        name_children(&mut finding);
        self.result.findings.push(finding);
    }
    pub(crate) fn coverage(&mut self, source: &str, status: &str, detail: &str) {
        if let Some(current) = self
            .result
            .coverage
            .iter_mut()
            .find(|coverage| coverage.source == source)
        {
            let rank = |status: &str| match status {
                "complete" => 0,
                "skipped" => 1,
                "pending" => 2,
                "unsupported" => 3,
                "limited" => 5,
                "redacted" => 6,
                _ => 4,
            };
            if rank(status) > rank(&current.status) {
                current.status = status.into();
            }
            if !current.detail.contains(detail) {
                current.detail = clip(
                    &format!("{} {detail}", current.detail),
                    MAX_COVERAGE_DETAIL_BYTES,
                );
            }
            return;
        }
        if self.result.coverage.len() < MAX_COVERAGE_SOURCES {
            self.result.coverage.push(Coverage {
                source: source.into(),
                status: status.into(),
                detail: detail.into(),
            });
        }
    }
}
