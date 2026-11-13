//! Ephemeral verified source snapshots. This cache never opens a path or URL.
//! Only native supervisors publish code; reads are explicit local developer actions.

use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 32;
const MAX_FILES: usize = 256;
static SOURCES: Mutex<VecDeque<Artifact>> = Mutex::new(VecDeque::new());
struct Artifact {
    key: String,
    files: BTreeMap<String, Arc<str>>,
    bytes: usize,
}

/// Builds a bounded immutable artifact before publishing it atomically.
pub struct SourceBuilder {
    artifact: Artifact,
}
impl SourceBuilder {
    /// Native identity includes every executable and mapping input hash.
    pub fn new(key: String) -> Self {
        Self {
            artifact: Artifact {
                key,
                files: BTreeMap::new(),
                bytes: 0,
            },
        }
    }
    /// Adds a complete UTF-8 file. Excessive files remain explicitly unavailable.
    /// Duplicate identities never replace earlier exact bytes.
    pub fn add(&mut self, name: &str, text: &str) {
        if name.is_empty()
            || name.len() > 2048
            || text.len() > MAX_FILE_BYTES
            || self.artifact.files.len() >= MAX_FILES
            || self.artifact.files.contains_key(name)
            || self
                .artifact
                .bytes
                .saturating_add(name.len())
                .saturating_add(text.len())
                > MAX_ARTIFACT_BYTES
        {
            return;
        }
        self.artifact.bytes += name.len() + text.len();
        self.artifact.files.insert(name.to_owned(), Arc::from(text));
    }
    /// Publishes inert sources under a global FIFO byte and artifact bound.
    pub fn publish(self) {
        let artifact = self.artifact;
        if artifact.key.is_empty() || artifact.key.len() > 512 || artifact.files.is_empty() {
            return;
        }
        let mut cache = SOURCES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.iter().any(|entry| entry.key == artifact.key) {
            return;
        }
        while cache.len() >= MAX_ARTIFACTS
            || cache
                .iter()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                .saturating_add(artifact.bytes)
                > MAX_BYTES
        {
            cache.pop_front();
        }
        cache.push_back(artifact);
    }
}

/// Bounded text on one actual source line; columns use UTF-16 like V8/Monaco.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLine {
    /// One-based line in the immutable file.
    pub number: u32,
    /// One-based UTF-16 column where this preview starts.
    pub start_column: u32,
    /// Inert text; large lines are centered around the requested column.
    pub text: String,
    /// Some characters on this line are omitted.
    pub shortened: bool,
}
/// Exact source evidence; never substitutes a newer draft or rereads disk.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePage {
    /// Native immutable artifact identity.
    pub artifact: String,
    /// Exact registered module or original source identity.
    pub source: String,
    /// Requested line in this artifact.
    pub line: u32,
    /// Requested UTF-16 column in this artifact.
    pub column: u32,
    /// Number of actual source lines.
    pub total_lines: u32,
    /// Finite read-only preview around the requested frame.
    pub lines: Vec<SourceLine>,
}
/// Reads at most 41 lines/32 KiB from a registered snapshot. No filesystem access.
pub fn read(artifact: &str, source: &str, line: u32, column: u32) -> Option<SourcePage> {
    if artifact.len() > 512 || source.len() > 2048 || line == 0 || column == 0 {
        return None;
    }
    let text = {
        let cache = SOURCES.lock().ok()?;
        Arc::clone(
            cache
                .iter()
                .find(|entry| entry.key == artifact)?
                .files
                .get(source)?,
        )
    };
    // split preserves the final empty line, matching engine/editor coordinates.
    let total_lines = u32::try_from(text.split('\n').count()).ok()?;
    if line > total_lines {
        return None;
    }
    let first = line.saturating_sub(20).max(1);
    let mut budget = 32 * 1024;
    let mut lines = Vec::new();
    for (index, value) in text
        .split('\n')
        .enumerate()
        .skip((first - 1) as usize)
        .take(41)
    {
        let number = u32::try_from(index).ok()?.checked_add(1)?;
        let value = value.trim_suffix('\r');
        let target = if number == line {
            column.saturating_sub(1)
        } else {
            0
        };
        if number == line && target > u32::try_from(value.encode_utf16().count()).ok()? {
            return None;
        }
        let start = target.saturating_sub(128);
        let mut units = 0u32;
        let mut start_column = 1;
        let mut output = String::new();
        let limit = budget.min(768);
        for character in value.chars() {
            if units < start {
                units += u32::try_from(character.len_utf16()).ok()?;
                start_column = units + 1;
                continue;
            }
            if output.len() + character.len_utf8() > limit {
                break;
            }
            output.push(character);
        }
        budget -= output.len();
        let shortened = output.len() < value.len();
        lines.push(SourceLine {
            number,
            start_column,
            text: output,
            shortened,
        });
    }
    Some(SourcePage {
        artifact: artifact.to_owned(),
        source: source.to_owned(),
        line,
        column,
        total_lines,
        lines,
    })
}

#[cfg(test)]
mod tests {
    static SOURCES_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[test]
    fn navigation_reads_exact_snapshot_and_never_resolves_external_locations() {
        let _serial = SOURCES_TEST.lock().unwrap();
        let mut source = super::SourceBuilder::new("immutable-source-test".into());
        source.add(
            "file:///module.js",
            "const emoji = '😀';\nthrow Error('old revision');\n",
        );
        source.publish();
        let mut newer = super::SourceBuilder::new("immutable-source-test".into());
        newer.add("file:///module.js", "different draft");
        newer.publish();
        let page = super::read("immutable-source-test", "file:///module.js", 2, 1).unwrap();
        assert_eq!(page.total_lines, 3);
        assert_eq!(page.lines[1].text, "throw Error('old revision');");
        assert!(super::read("immutable-source-test", "file:///C:/secret", 1, 1).is_none());
        assert!(super::read("immutable-source-test", "file:///module.js", 4, 1).is_none());
        assert!(super::read("immutable-source-test", "file:///module.js", 1, 0).is_none());
    }

    #[test]
    fn source_previews_bound_large_unicode_lines_and_validate_utf16_positions() {
        let _serial = SOURCES_TEST.lock().unwrap();
        let mut source = super::SourceBuilder::new("unicode-source-test".into());
        source.add("module.js", &format!("{}\n\n", "😀界".repeat(20000)));
        source.publish();
        let page = super::read("unicode-source-test", "module.js", 1, 30000).unwrap();
        assert!(page.lines[0].shortened);
        assert!(page.lines[0].start_column < page.column);
        assert!(
            page.lines[0].start_column
                + u32::try_from(page.lines[0].text.encode_utf16().count()).unwrap()
                > page.column
        );
        assert!(page.lines.iter().map(|line| line.text.len()).sum::<usize>() <= 32 * 1024);
        assert!(super::read("unicode-source-test", "module.js", 1, 60002).is_none());
        assert!(super::read("unicode-source-test", "module.js", 3, 1).is_some());
    }

    #[test]
    fn source_pressure_expires_old_artifacts_instead_of_substituting_newer_code() {
        let _serial = SOURCES_TEST.lock().unwrap();
        for revision in 0..=super::MAX_ARTIFACTS {
            let mut source = super::SourceBuilder::new(format!("bounded-source-{revision}"));
            source.add("main.js", &format!("revision {revision}"));
            source.publish();
        }
        assert!(super::read("bounded-source-0", "main.js", 1, 1).is_none());
        let current = super::read("bounded-source-32", "main.js", 1, 1).unwrap();
        assert_eq!(current.lines[0].text, "revision 32");
        assert!(super::SOURCES.lock().unwrap().len() <= super::MAX_ARTIFACTS);
    }
}
