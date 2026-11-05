//! Per-window presentation choices contain no worker values or execution authority.
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebuggerLayout {
    pub sources_split: u8,
    pub inspector_split: u8,
    pub console_split: u8,
    pub sources_visible: bool,
    pub inspector_visible: bool,
    pub console_visible: bool,
}
impl Default for DebuggerLayout {
    fn default() -> Self {
        Self {
            sources_split: 18,
            inspector_split: 22,
            console_split: 70,
            sources_visible: true,
            inspector_visible: true,
            console_visible: true,
        }
    }
}
impl DebuggerLayout {
    pub fn valid(&self) -> bool {
        (15..=40).contains(&self.sources_split)
            && (15..=40).contains(&self.inspector_split)
            && (45..=85).contains(&self.console_split)
    }
}
pub(super) struct Presentation {
    pub placement: String,
    pub layout: DebuggerLayout,
}
impl Default for Presentation {
    fn default() -> Self {
        Self {
            placement: "window".into(),
            layout: DebuggerLayout::default(),
        }
    }
}
