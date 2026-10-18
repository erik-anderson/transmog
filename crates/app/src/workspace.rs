//! Non-sensitive presentation preferences shared by desktop workspaces.

use serde::{Deserialize, Serialize};

/// Per-user optional response-set guide history; no content or in-progress step.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseSetGuideHistory {
    /// The invitation has never been offered.
    #[default]
    NotOffered,
    /// Offered once, whether dismissed explicitly or left behind.
    Offered,
    /// Started previously, whether ended early or completed; help offers replay.
    Started,
}

/// Narrow guide operation independent of layout and automation changes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseSetGuideAction {
    /// Atomically claim the one-time invitation.
    Offer,
    /// Remember a first start or replay.
    Start,
}

/// Result of an atomic guide-history update.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSetGuideUpdate {
    /// Current durable history, including other windows' changes.
    pub history: ResponseSetGuideHistory,
    /// Only the caller which first claimed the invitation receives true.
    pub offered: bool,
}

/// Available traffic metadata columns.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficColumn {
    /// Immutable workspace admission number.
    RequestNumber,
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
    /// Original HTTP exchange duration, independent of stream lifetime.
    HttpDuration,
    /// Decoded client stream-message bytes, unavailable for ordinary HTTP.
    StreamSent,
    /// Decoded server stream-message bytes, unavailable for ordinary HTTP.
    StreamReceived,
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
    /// Optional guide history, preserved independently of layout writes.
    #[serde(default)]
    pub response_set_guide: ResponseSetGuideHistory,
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
    /// Percentage allocated to the autoresponse rule list beside its properties.
    pub autoresponse_split: u8,
    /// Legacy wrapping preference retained for saved-settings compatibility; ignored by Traffic.
    pub wrap_cells: bool,
    /// Use compact row spacing.
    pub compact_rows: bool,
    /// Ordered column presentation preferences.
    pub columns: Vec<ColumnPreference>,
}

impl Default for WorkspacePreferences {
    fn default() -> Self {
        use TrafficColumn::{
            ContentType, Duration, Host, HttpDuration, Method, Path, Pid, Process, Protocol,
            RequestBytes, RequestNumber, ResponseBytes, StartedAt, State, Status, StreamReceived,
            StreamSent, Url,
        };
        Self {
            response_set_guide: ResponseSetGuideHistory::NotOffered,
            sidebar_collapsed: false,
            layout: TrafficLayout::Stacked,
            list_split: 45,
            request_split: 45,
            request_body_split: 35,
            response_body_split: 35,
            autoresponse_split: 45,
            wrap_cells: false,
            compact_rows: true,
            columns: [
                (RequestNumber, 72),
                (Method, 76),
                (Status, 84),
                (Process, 180),
                (Host, 180),
                (Path, 260),
                (Duration, 94),
                (ResponseBytes, 112),
                (Protocol, 210),
                (RequestBytes, 112),
                (State, 104),
                (ContentType, 160),
                (StartedAt, 126),
                (Pid, 84),
                (Url, 340),
                (HttpDuration, 112),
                (StreamSent, 120),
                (StreamReceived, 132),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (id, width))| ColumnPreference {
                id,
                width,
                visible: index < 9,
                pinned: index < 3,
            })
            .collect(),
        }
    }
}

impl WorkspacePreferences {
    /// Adds only known new columns to otherwise complete supported older layouts.
    pub(crate) fn migrate_columns(&mut self) {
        let ids: std::collections::BTreeSet<_> =
            self.columns.iter().map(|column| column.id).collect();
        if ids.len() != self.columns.len() || !self.columns.iter().any(|column| column.visible) {
            return;
        }
        let missing: Vec<_> = Self::default()
            .columns
            .into_iter()
            .filter(|column| !ids.contains(&column.id))
            .collect();
        if missing.iter().any(|column| {
            !matches!(
                column.id,
                TrafficColumn::RequestNumber
                    | TrafficColumn::HttpDuration
                    | TrafficColumn::StreamSent
                    | TrafficColumn::StreamReceived
            )
        }) {
            return;
        }
        for column in missing {
            if column.id == TrafficColumn::RequestNumber {
                self.columns.insert(0, column);
            } else {
                self.columns.push(column);
            }
        }
    }
    pub(crate) fn is_valid(&self) -> bool {
        let ids: std::collections::BTreeSet<_> =
            self.columns.iter().map(|column| column.id).collect();
        self.columns.len() == 18
            && ids.len() == 18
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
                self.autoresponse_split,
            ]
            .into_iter()
            .all(|split| (15..=85).contains(&split))
    }
}
