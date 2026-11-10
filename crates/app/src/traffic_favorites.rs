//! Narrow, serialized ownership of explicitly authored Traffic values.
use crate::sessions::{PreparedFilter, facets::prepare_value};
use crate::{
    AppError, Application, ErrorCategory, FilterOperator, SessionColumnFilter, TrafficDimension,
    TrafficFilterValue,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
const MAX_LIBRARY_BYTES: usize = 4 * 1024 * 1024;

/// One explicitly saved membership term or additional requirement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficFavorite {
    /// Stable built-in field identity.
    pub dimension: TrafficDimension,
    /// Membership term (absent for an additional requirement).
    pub value: Option<TrafficFilterValue>,
    /// Authored comparison (absent for membership terms).
    pub condition: Option<SessionColumnFilter>,
}
/// Bounded library page, independently of any window's selections.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficFavoritesPage {
    /// Monotonic library revision.
    pub revision: u64,
    /// Explicitly authored entries only.
    pub entries: Vec<TrafficFavorite>,
    /// Next offset in stable canonical order.
    pub next_offset: Option<usize>,
}
/// An idempotent narrow library edit.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TrafficFavoritesChange {
    /// Add a complete validated batch, or make no change.
    Add {
        /// Complete batch to validate before saving.
        entries: Vec<TrafficFavorite>,
    },
    /// Explicitly remove one entry, including a previously cleared entry.
    Remove {
        /// Canonical identity to remove.
        entry: TrafficFavorite,
    },
    /// Clear one field and create a bounded Undo receipt.
    Clear {
        /// Only this field is cleared.
        dimension: TrafficDimension,
    },
    /// Restore only entries untouched since their clear.
    Undo {
        /// Receipt returned by a successful clear.
        token: String,
    },
}
/// Mutation feedback derived from the committed change.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficFavoritesResult {
    /// Committed revision.
    pub revision: u64,
    /// Number actually added or removed.
    pub changed: usize,
    /// Clear receipt, if anything was removed.
    pub undo: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
struct Stored {
    revision: u64,
    entries: BTreeMap<String, TrafficFavorite>,
}
#[derive(Clone, PartialEq)]
struct Receipt {
    token: String,
    entries: Vec<TrafficFavorite>,
}
#[derive(Default)]
struct State {
    stored: Stored,
    path: Option<PathBuf>,
    receipts: VecDeque<Receipt>,
    load_error: bool,
}
#[derive(Clone, Default)]
pub(crate) struct Library(Arc<Mutex<State>>);
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Internal, message, true)
}
fn canonical(mut entry: TrafficFavorite) -> Result<TrafficFavorite, AppError> {
    match (&entry.value, &mut entry.condition) {
        (Some(value), None) => {
            let value = prepare_value(entry.dimension, value)?;
            // Unicode case normalization can expand the stored character count.
            prepare_value(entry.dimension, &value)?;
            entry.value = Some(value);
        }
        (None, Some(condition))
            if entry.dimension == TrafficDimension::Column(condition.column) =>
        {
            PreparedFilter::new(condition)?;
            condition.value = if condition.operator == FilterOperator::Equals
                && !crate::sessions::numeric_column(condition.column)
            {
                if let TrafficFilterValue::Text(value) = prepare_value(
                    entry.dimension,
                    &TrafficFilterValue::Text(condition.value.clone()),
                )? {
                    value
                } else {
                    condition.value.to_lowercase()
                }
            } else if crate::sessions::numeric_column(condition.column)
                && condition.operator != FilterOperator::Contains
            {
                condition
                    .value
                    .parse::<u64>()
                    .map_err(|_| invalid("Enter a decimal integer"))?
                    .to_string()
            } else {
                condition.value.to_lowercase()
            };
            PreparedFilter::new(condition)?;
        }
        _ => {
            return Err(invalid(
                "A Favorite needs one typed term or one comparison for its field",
            ));
        }
    }
    Ok(entry)
}
fn key(entry: &TrafficFavorite) -> String {
    serde_json::to_string(entry).expect("serializable Favorite")
}
impl Library {
    pub(crate) fn load(path: Option<&Path>) -> Self {
        let path = path.map(|path| path.with_extension("traffic-favorites.json"));
        let mut state = State {
            path,
            ..State::default()
        };
        if let Some(path) = &state.path {
            match std::fs::metadata(path).and_then(|metadata| {
                if metadata.len() > MAX_LIBRARY_BYTES as u64 {
                    return Err(std::io::Error::other(
                        "Favorites file exceeds its read budget",
                    ));
                }
                std::fs::read(path)
            }) {
                Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
                    Ok(stored)
                        if stored.entries.len() <= 4096
                            && stored.entries.iter().all(|(id, entry)| {
                                canonical(entry.clone()).is_ok_and(|value| key(&value) == *id)
                            }) =>
                    {
                        state.stored = stored;
                    }
                    _ => state.load_error = true,
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => state.load_error = true,
            }
        }
        Self(Arc::new(Mutex::new(state)))
    }
    fn page(
        &self,
        dimension: TrafficDimension,
        offset: usize,
        limit: usize,
    ) -> Result<TrafficFavoritesPage, AppError> {
        if !(1..=256).contains(&limit) {
            return Err(invalid("Read 1–256 Favorites per page"));
        }
        let state = self
            .0
            .lock()
            .map_err(|_| unavailable("Favorites unavailable"))?;
        if state.load_error {
            return Err(unavailable(
                "Favorites could not be read. Restore the local Favorites file, then restart Transmog.",
            ));
        }
        let entries: Vec<_> = state
            .stored
            .entries
            .values()
            .filter(|entry| entry.dimension == dimension)
            .cloned()
            .collect();
        Ok(TrafficFavoritesPage {
            revision: state.stored.revision,
            next_offset: (offset.saturating_add(limit) < entries.len())
                .then_some(offset.saturating_add(limit)),
            entries: entries.into_iter().skip(offset).take(limit).collect(),
        })
    }
    fn change(&self, change: TrafficFavoritesChange) -> Result<TrafficFavoritesResult, AppError> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| unavailable("Favorites unavailable"))?;
        if state.load_error {
            return Err(unavailable(
                "Favorites could not be read; existing data was preserved",
            ));
        }
        let mut next = state.stored.clone();
        let revision = next
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("Favorites revision limit reached"))?;
        let mut receipts = state.receipts.clone();
        let mut changed = 0;
        let mut undo = None;
        match change {
            TrafficFavoritesChange::Add { entries } => {
                if entries.len() > 256 {
                    return Err(invalid("Save at most 256 Favorites in one batch"));
                }
                let entries = entries
                    .into_iter()
                    .map(canonical)
                    .collect::<Result<Vec<_>, _>>()?;
                for entry in entries {
                    changed += usize::from(next.entries.insert(key(&entry), entry).is_none());
                }
            }
            TrafficFavoritesChange::Remove { entry } => {
                let entry = canonical(entry)?;
                changed = usize::from(next.entries.remove(&key(&entry)).is_some());
                // A later explicit removal invalidates older Undo intent, even if already absent.
                for receipt in &mut receipts {
                    receipt.entries.retain(|value| value != &entry);
                }
            }
            TrafficFavoritesChange::Clear { dimension } => {
                for receipt in &mut receipts {
                    receipt.entries.retain(|entry| entry.dimension != dimension);
                }
                let entries: Vec<_> = next
                    .entries
                    .values()
                    .filter(|entry| entry.dimension == dimension)
                    .cloned()
                    .collect();
                changed = entries.len();
                for entry in &entries {
                    next.entries.remove(&key(entry));
                }
                if changed > 0 {
                    let token = format!("clear-{revision}");
                    undo = Some(token.clone());
                    receipts.push_back(Receipt { token, entries });
                    while receipts.len() > 32 {
                        receipts.pop_front();
                    }
                }
            }
            TrafficFavoritesChange::Undo { token } => {
                if let Some(receipt) = receipts.iter().find(|receipt| receipt.token == token) {
                    for entry in &receipt.entries {
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            next.entries.entry(key(entry))
                        {
                            slot.insert(entry.clone());
                            changed += 1;
                        }
                    }
                }
                receipts.retain(|receipt| receipt.token != token);
            }
        }
        if next.entries == state.stored.entries && receipts == state.receipts {
            return Ok(TrafficFavoritesResult {
                revision: state.stored.revision,
                changed: 0,
                undo: None,
            });
        }
        next.revision = revision;
        let bytes = encode_library(&next)?;
        if let Some(path) = &state.path {
            persist(path, &bytes)?;
        }
        state.stored = next;
        state.receipts = receipts;
        Ok(TrafficFavoritesResult {
            revision: state.stored.revision,
            changed,
            undo,
        })
    }
}
fn encode_library(stored: &Stored) -> Result<Vec<u8>, AppError> {
    if stored.entries.len() > 4096
        || stored
            .entries
            .values()
            .fold(BTreeMap::new(), |mut counts, entry| {
                *counts.entry(entry.dimension).or_insert(0) += 1;
                counts
            })
            .values()
            .any(|count| *count > 1024)
    {
        return Err(invalid(
            "Favorites capacity reached: 1024 per field, 4096 total. Remove saved values and retry.",
        ));
    }
    let bytes = serde_json::to_vec(stored)
        .map_err(|_| unavailable("Favorites could not be saved. Your draft is intact."))?;
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(invalid(
            "Favorites storage capacity reached. Remove saved values or shorten the list and retry.",
        ));
    }
    Ok(bytes)
}
fn persist(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().ok_or("Favorites folder unavailable")?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    };
    write().map_err(|_| {
        unavailable("Favorites could not be saved. Your draft is intact; retry saving.")
    })
}
impl Application {
    /// Reads only explicitly saved Traffic Favorites.
    /// # Errors
    /// Returns invalid paging or unavailable storage.
    pub fn traffic_favorites(
        &self,
        dimension: TrafficDimension,
        offset: usize,
        limit: usize,
    ) -> Result<TrafficFavoritesPage, AppError> {
        self.traffic_favorites.page(dimension, offset, limit)
    }
    /// Commits a narrow Favorite edit independently of Traffic queries.
    /// # Errors
    /// Returns invalid terms, exceeded capacity or a failed atomic write.
    pub fn change_traffic_favorites(
        &self,
        change: TrafficFavoritesChange,
    ) -> Result<TrafficFavoritesResult, AppError> {
        self.traffic_favorites.change(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host(value: &str) -> TrafficFavorite {
        TrafficFavorite {
            dimension: TrafficDimension::Column(crate::TrafficColumn::Host),
            value: Some(TrafficFilterValue::Text(value.into())),
            condition: None,
        }
    }
    #[test]
    fn library_starts_empty_persists_batches_and_undo_preserves_later_intent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts.json");
        let owner = Library::load(Some(&path));
        let other = owner.clone();
        let dimension = host("a.test").dimension;
        assert_eq!(
            owner.page(dimension, 0, 256).unwrap().entries,
            [] as [TrafficFavorite; 0]
        );
        owner
            .change(TrafficFavoritesChange::Add {
                entries: vec![host("A.TEST."), host("a.test"), host("b.test")],
            })
            .unwrap();
        assert_eq!(other.page(dimension, 0, 1).unwrap().next_offset, Some(1));
        assert!(
            owner
                .change(TrafficFavoritesChange::Add {
                    entries: vec![host("c.test"), host("a:80")]
                })
                .is_err()
        );
        assert_eq!(other.page(dimension, 0, 256).unwrap().entries.len(), 2);
        let clear = other
            .change(TrafficFavoritesChange::Clear { dimension })
            .unwrap();
        assert_eq!(clear.changed, 2);
        owner
            .change(TrafficFavoritesChange::Add {
                entries: vec![host("c.test")],
            })
            .unwrap();
        owner
            .change(TrafficFavoritesChange::Remove {
                entry: host("a.test"),
            })
            .unwrap();
        let undo = owner
            .change(TrafficFavoritesChange::Undo {
                token: clear.undo.unwrap(),
            })
            .unwrap();
        assert_eq!(undo.changed, 1);
        let restored = Library::load(Some(&path)).page(dimension, 0, 256).unwrap();
        assert_eq!(restored.entries, vec![host("b.test"), host("c.test")]);
    }
    #[test]
    fn failed_write_keeps_last_good_state_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts.json");
        let library = Library::load(Some(&path));
        library
            .change(TrafficFavoritesChange::Add {
                entries: vec![host("a.test")],
            })
            .unwrap();
        let target = path.with_extension("traffic-favorites.json");
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        assert!(
            library
                .change(TrafficFavoritesChange::Add {
                    entries: vec![host("b.test")]
                })
                .is_err()
        );
        assert_eq!(
            library
                .page(host("a.test").dimension, 0, 256)
                .unwrap()
                .entries,
            vec![host("a.test")]
        );
        std::fs::remove_dir(&target).unwrap();
        library
            .change(TrafficFavoritesChange::Add {
                entries: vec![host("b.test")],
            })
            .unwrap();
        assert_eq!(
            Library::load(Some(&path))
                .page(host("a.test").dimension, 0, 256)
                .unwrap()
                .entries
                .len(),
            2
        );
    }
    #[test]
    fn unicode_library_capacity_never_commits_an_unreadable_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts.json");
        let target = path.with_extension("traffic-favorites.json");
        let library = Library::load(Some(&path));
        let fields = [
            crate::TrafficColumn::Url,
            crate::TrafficColumn::Path,
            crate::TrafficColumn::Process,
            crate::TrafficColumn::Method,
        ];
        for batch in 0..16 {
            let dimension = TrafficDimension::Column(fields[batch / 4]);
            let before = std::fs::read(&target).unwrap_or_default();
            let result = library.change(TrafficFavoritesChange::Add {
                entries: (0..256)
                    .map(|index| TrafficFavorite {
                        dimension,
                        value: Some(TrafficFilterValue::Text(format!(
                            "{}-{}",
                            "💠".repeat(240),
                            batch * 256 + index
                        ))),
                        condition: None,
                    })
                    .collect(),
            });
            if let Err(error) = result {
                assert!(error.to_string().contains("capacity"));
                assert_eq!(std::fs::read(&target).unwrap(), before);
                assert_eq!(
                    library.page(dimension, 0, 1).unwrap().revision,
                    Library::load(Some(&path))
                        .page(dimension, 0, 1)
                        .unwrap()
                        .revision
                );
                return;
            }
            assert!(
                Library::load(Some(&path)).page(dimension, 0, 1).is_ok(),
                "A successful Favorite save could not be read after restart"
            );
        }
        panic!("The fixture did not reach the serialized library capacity");
    }
    #[test]
    fn normalized_favorites_must_still_fit_the_reload_limits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts.json");
        let library = Library::load(Some(&path));
        let dimension = TrafficDimension::Column(crate::TrafficColumn::Path);
        for count in [127, 128] {
            let value = "İ".repeat(count);
            for condition in [
                None,
                Some(FilterOperator::Contains),
                Some(FilterOperator::Equals),
            ] {
                let entry = TrafficFavorite {
                    dimension,
                    value: condition
                        .is_none()
                        .then(|| TrafficFilterValue::Text(value.clone())),
                    condition: condition.map(|operator| SessionColumnFilter {
                        column: crate::TrafficColumn::Path,
                        operator,
                        value: value.clone(),
                    }),
                };
                assert_eq!(
                    library
                        .change(TrafficFavoritesChange::Add {
                            entries: vec![entry]
                        })
                        .is_ok(),
                    count == 127
                );
            }
        }
        assert_eq!(
            Library::load(Some(&path))
                .page(dimension, 0, 256)
                .unwrap()
                .entries
                .len(),
            3
        );
    }
    #[test]
    fn idempotent_retries_do_not_require_another_write() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scripts.json");
        let target = path.with_extension("traffic-favorites.json");
        let backup = root.path().join("last-good.json");
        let library = Library::load(Some(&path));
        let first = library
            .change(TrafficFavoritesChange::Add {
                entries: vec![host("a.test")],
            })
            .unwrap();
        let bytes = std::fs::read(&target).unwrap();
        std::fs::rename(&target, &backup).unwrap();
        std::fs::create_dir(&target).unwrap();
        for change in [
            TrafficFavoritesChange::Add {
                entries: vec![host("A.TEST.")],
            },
            TrafficFavoritesChange::Remove {
                entry: host("absent.test"),
            },
            TrafficFavoritesChange::Undo {
                token: "already-consumed".into(),
            },
            TrafficFavoritesChange::Clear {
                dimension: TrafficDimension::Column(crate::TrafficColumn::Process),
            },
        ] {
            let result = library.change(change).unwrap();
            assert_eq!(result.changed, 0);
            assert_eq!(result.revision, first.revision);
        }
        std::fs::remove_dir(&target).unwrap();
        std::fs::rename(&backup, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(
            Library::load(Some(&path))
                .page(host("a.test").dimension, 0, 256)
                .unwrap()
                .entries,
            vec![host("a.test")]
        );
    }
}
