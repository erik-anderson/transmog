#![deny(missing_docs)]
#![deny(unsafe_code)]

//! Ephemeral, untrusted script evidence shared by isolated hosts and native UI.
//! These types carry no engine handles and grant no source or traffic access.

use serde::{Deserialize, Serialize};

pub mod debugger;
mod logging;
pub mod sources;
pub mod stream;
pub mod supervision;
pub use logging::*;

/// Maximum engine frames retained for one exception including its causes.
pub const MAX_EXCEPTION_FRAMES: usize = 32;
/// Maximum retained causes after the outer exception.
pub const MAX_EXCEPTION_CAUSES: usize = 4;
/// Maximum UTF-8 text bytes across one complete exception diagnostic.
pub const MAX_EXCEPTION_TEXT_BYTES: usize = 16 * 1024;

/// One position in an immutable executing or mapped source artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceLocation {
    /// Module identity, not permission to read a file or fetch a URL.
    pub source: String,
    /// One-based line number.
    pub line: u32,
    /// One-based UTF-16 column number.
    pub column: u32,
}

/// One engine-owned frame with separately attributed original-source mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StackFrame {
    /// Name supplied by the engine; empty for an anonymous frame.
    pub function: String,
    /// Executing JavaScript location, if available.
    pub generated: Option<SourceLocation>,
    /// Original source location verified against the exact executing artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<SourceLocation>,
    /// Why a declared source map could not provide an original position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping_issue: Option<MappingIssue>,
}

/// Static source-map fallback explanations, independent of authored exception text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MappingIssue {
    /// The declared map is absent from the verified package inventory.
    Missing,
    /// Invalid, unsupported, or excessive source-map data.
    Invalid,
    /// No token covers this generated position.
    Unmapped,
    /// The original source escapes the verified package namespace.
    Source,
    /// Adding original locations would exceed the exception evidence budget.
    Budget,
}

/// Safe-to-copy snapshot of one thrown value. Its text remains untrusted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExceptionValue {
    /// Error name from a safely readable data property, if available.
    pub name: Option<String>,
    /// Bounded message or primitive representation; never invokes user conversion.
    pub message: String,
    /// Engine frames; never parsed from an authored `stack` property.
    pub frames: Vec<StackFrame>,
    /// Some details could not be inspected without executing user code.
    pub unavailable: bool,
}

/// Bounded exception snapshot, kept separate from host-controlled status text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExceptionDiagnostic {
    /// Native immutable source-cache identity; workers cannot grant source access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// Outermost thrown value.
    pub exception: ExceptionValue,
    /// Ordered `cause` chain, flattened to avoid recursive wire allocations.
    pub causes: Vec<ExceptionValue>,
    /// Capture reached a byte, frame, cause, or cycle bound.
    pub truncated: bool,
}

impl ExceptionDiagnostic {
    /// Checks evidence received across an untrusted worker boundary.
    /// Call before mapping, cloning, retaining, or exposing received evidence.
    pub fn is_bounded(&self) -> bool {
        if self.artifact.as_ref().is_some_and(|id| id.len() > 512)
            || self.causes.len() > MAX_EXCEPTION_CAUSES
        {
            return false;
        }
        let mut bytes = 0usize;
        let mut frames = 0usize;
        for value in std::iter::once(&self.exception).chain(&self.causes) {
            if value.frames.len() > MAX_EXCEPTION_FRAMES {
                return false;
            }
            bytes = bytes.saturating_add(value.message.len());
            bytes = bytes.saturating_add(value.name.as_ref().map_or(0, String::len));
            frames = frames.saturating_add(value.frames.len());
            for frame in &value.frames {
                bytes = bytes.saturating_add(frame.function.len());
                for location in [&frame.generated, &frame.original].into_iter().flatten() {
                    if location.line == 0 || location.column == 0 {
                        return false;
                    }
                    bytes = bytes.saturating_add(location.source.len());
                }
            }
        }
        frames <= MAX_EXCEPTION_FRAMES && bytes <= MAX_EXCEPTION_TEXT_BYTES
    }

    /// Returns retained generated frames for source-map projection.
    pub fn frames_mut(&mut self) -> impl Iterator<Item = &mut StackFrame> {
        self.exception
            .frames
            .iter_mut()
            .chain(self.causes.iter_mut().flat_map(|v| &mut v.frames))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_limits_include_causes_and_both_source_locations() {
        let mut diagnostic = ExceptionDiagnostic {
            artifact: None,
            exception: ExceptionValue {
                name: None,
                message: String::new(),
                frames: vec![],
                unavailable: false,
            },
            causes: vec![],
            truncated: false,
        };
        assert!(diagnostic.is_bounded());
        diagnostic.exception.message = "x".repeat(MAX_EXCEPTION_TEXT_BYTES);
        assert!(diagnostic.is_bounded());
        diagnostic.causes.push(ExceptionValue {
            name: None,
            message: "x".into(),
            frames: vec![],
            unavailable: false,
        });
        assert!(!diagnostic.is_bounded());
        diagnostic.exception.message.clear();
        diagnostic.exception.frames = vec![StackFrame {
            function: String::new(),
            generated: Some(SourceLocation {
                source: "main.js".into(),
                line: 0,
                column: 1,
            }),
            original: None,
            mapping_issue: None,
        }];
        assert!(!diagnostic.is_bounded());
    }
}
