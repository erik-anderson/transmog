//! Non-sensitive presentation preferences shared by desktop workspaces.

use serde::{Deserialize, Serialize};

/// Available traffic metadata columns.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficColumn {
    /// Request verb.
    Method,
    /// HTTP response status.
    Status,
    /// Originating process name.
    Process,
    /// Originating process identifier.
    Pid,
    /// Target host.
    Host,
    /// Target path and query.
    Path,
    /// Complete original target URL.
    Url,
    /// Client-facing HTTP version.
    Protocol,
    /// Exchange duration in milliseconds.
    Duration,
    /// Client-visible response bytes.
    ResponseBytes,
    /// Client request bytes.
    RequestBytes,
    /// Exchange completion category.
    State,
    /// Client-visible response media type.
    ContentType,
    /// Exchange start timestamp.
    StartedAt,
}

/// One column's persisted presentation, without captured traffic values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnPreference {
    /// Metadata column identity.
    pub id: TrafficColumn,
    /// Width in logical CSS pixels.
    pub width: u16,
    /// Whether the column is shown.
    pub visible: bool,
    /// Keep the column at the leading edge during horizontal scrolling.
    pub pinned: bool,
}

/// Main traffic and inspector arrangement.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficLayout {
    /// Traffic list above the inspector.
    #[default]
    Stacked,
    /// Traffic list to the left of the inspector.
    SideBySide,
}

/// Durable layout and table presentation preferences.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePreferences {
    /// Collapse navigation to its icon rail.
    pub sidebar_collapsed: bool,
    /// Traffic layout preset.
    pub layout: TrafficLayout,
    /// Percentage allocated to the traffic list.
    pub list_split: u8,
    /// Percentage allocated to the request inspector.
    pub request_split: u8,
    /// Request headers/body split percentage.
    pub request_body_split: u8,
    /// Response headers/body split percentage.
    pub response_body_split: u8,
    /// Wrap table cell contents.
    pub wrap_cells: bool,
    /// Use compact row spacing.
    pub compact_rows: bool,
    /// Ordered column presentation preferences.
    pub columns: Vec<ColumnPreference>,
}

impl Default for WorkspacePreferences {
    fn default() -> Self {
        use TrafficColumn::{
            ContentType, Duration, Host, Method, Path, Pid, Process, Protocol, RequestBytes,
            ResponseBytes, StartedAt, State, Status, Url,
        };
        Self {
            sidebar_collapsed: false,
            layout: TrafficLayout::Stacked,
            list_split: 45,
            request_split: 45,
            request_body_split: 35,
            response_body_split: 35,
            wrap_cells: false,
            compact_rows: true,
            columns: [
                (Method, 76),
                (Status, 84),
                (Process, 180),
                (Host, 180),
                (Path, 260),
                (Duration, 94),
                (ResponseBytes, 112),
                (Protocol, 100),
                (RequestBytes, 112),
                (State, 104),
                (ContentType, 160),
                (StartedAt, 126),
                (Pid, 84),
                (Url, 340),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (id, width))| ColumnPreference {
                id,
                width,
                visible: index < 7,
                pinned: index < 2,
            })
            .collect(),
        }
    }
}

impl WorkspacePreferences {
    pub(crate) fn is_valid(&self) -> bool {
        let ids: std::collections::BTreeSet<_> =
            self.columns.iter().map(|column| column.id).collect();
        self.columns.len() == 14
            && ids.len() == 14
            && self.columns.iter().any(|column| column.visible)
            && self
                .columns
                .iter()
                .all(|column| (32..=1200).contains(&column.width))
            && [
                self.list_split,
                self.request_split,
                self.request_body_split,
                self.response_body_split,
            ]
            .into_iter()
            .all(|split| (15..=85).contains(&split))
    }
}
