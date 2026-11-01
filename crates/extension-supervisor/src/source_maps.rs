//! Optional, inventory-bound source maps. Parsing never fetches a URL or file.

use std::collections::BTreeMap;

use transmog_extensions::{Failure, Snapshot};
use transmog_script_diagnostics::{MappingIssue, SourceLocation};
use url::Url;

const MAX_MAP_BYTES: usize = 256 * 1024;
const MAX_MAP_TOKENS: usize = 8192;
const ROOT: &str = "file:///transmog-extension/";

fn decode(bytes: &[u8]) -> Result<sourcemap::SourceMap, MappingIssue> {
    if bytes.len() > MAX_MAP_BYTES {
        return Err(MappingIssue::Invalid);
    }
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| MappingIssue::Invalid)?;
    let mappings = json["mappings"].as_str().ok_or(MappingIssue::Invalid)?;
    if json["version"] != 3
        || json.get("sections").is_some()
        || json["sources"]
            .as_array()
            .is_none_or(|values| values.len() > 128)
        || json.get("names").is_some_and(|names| {
            names
                .as_array()
                .is_none_or(|values| values.len() > MAX_MAP_TOKENS)
        })
        || mappings
            .bytes()
            .filter(|byte| matches!(byte, b',' | b';'))
            .count()
            >= MAX_MAP_TOKENS
    {
        return Err(MappingIssue::Invalid);
    }
    sourcemap::SourceMap::from_slice(bytes).map_err(|_| MappingIssue::Invalid)
}

fn project(
    map: &sourcemap::SourceMap,
    map_path: &str,
    generated: &SourceLocation,
) -> Result<SourceLocation, MappingIssue> {
    // The library's closest-token lookup can return a preceding line. Select a
    // same-line token explicitly, and check ranged-column arithmetic ourselves.
    let token = same_line_token(map, generated.line - 1, generated.column - 1)
        .ok_or(MappingIssue::Unmapped)?;
    let offset = if token.is_range() {
        generated.column - 1 - token.get_dst_col()
    } else {
        0
    };
    let root = Url::parse(ROOT).expect("constant root");
    let base = root.join(map_path).map_err(|_| MappingIssue::Source)?;
    let source = token.get_source().ok_or(MappingIssue::Source)?;
    let original = base.join(source).map_err(|_| MappingIssue::Source)?;
    if !original.as_str().starts_with(ROOT)
        || original.query().is_some()
        || original.fragment().is_some()
        || original.as_str().len() > 2048
    {
        return Err(MappingIssue::Source);
    }
    Ok(SourceLocation {
        source: original.to_string(),
        line: token
            .get_src_line()
            .checked_add(1)
            .ok_or(MappingIssue::Invalid)?,
        column: token
            .get_src_col()
            .checked_add(offset)
            .ok_or(MappingIssue::Invalid)?
            .checked_add(1)
            .ok_or(MappingIssue::Invalid)?,
    })
}

fn same_line_token(
    map: &sourcemap::SourceMap,
    line: u32,
    column: u32,
) -> Option<sourcemap::Token<'_>> {
    let mut low = 0;
    let mut high = usize::try_from(map.get_token_count()).ok()?;
    while low < high {
        let middle = low + (high - low) / 2;
        let token = map.get_token(middle)?;
        if (token.get_dst_line(), token.get_dst_col()) <= (line, column) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let token = map.get_token(low.checked_sub(1)?)?;
    (token.get_dst_line() == line).then_some(token)
}

pub(super) fn map_failure(failure: Failure, snapshot: &Snapshot) -> Failure {
    Mapper::new(snapshot).map_failure(failure)
}

pub(super) fn debug_event(
    snapshot: &Snapshot,
    event: &mut transmog_extensions::debugger::DebugEvent,
) {
    let transmog_extensions::debugger::DebugEventKind::Paused {
        frames, exception, ..
    } = &mut event.event
    else {
        return;
    };
    let mut mapper = Mapper::new(snapshot);
    if let Some(diagnostic) = exception.take() {
        let mut failure = Failure::new(
            transmog_extensions::FailureKind::Exception,
            "Debugger exception",
        );
        failure.exception = Some(diagnostic);
        *exception = mapper.map_failure(failure).exception;
    }
    let diagnostic = transmog_script_diagnostics::ExceptionDiagnostic {
        artifact: None,
        exception: transmog_script_diagnostics::ExceptionValue {
            name: None,
            message: String::new(),
            frames: frames
                .iter()
                .map(|frame| transmog_script_diagnostics::StackFrame {
                    function: String::new(),
                    generated: frame.location.clone(),
                    original: None,
                    mapping_issue: None,
                })
                .collect(),
            unavailable: false,
        },
        causes: vec![],
        truncated: false,
    };
    if !diagnostic.is_bounded() {
        return;
    }
    let mut failure = Failure::new(
        transmog_extensions::FailureKind::Exception,
        "Debugger stack",
    );
    failure.exception = Some(Box::new(diagnostic));
    if let Some(diagnostic) = mapper.map_failure(failure).exception {
        for (frame, mapped) in frames.iter_mut().zip(diagnostic.exception.frames) {
            frame.original = mapped.original;
        }
    }
}
pub(super) fn debug_breakpoints(
    snapshot: &Snapshot,
    values: &[transmog_extensions::debugger::SourceBreakpoint],
) -> Vec<transmog_extensions::debugger::SourceBreakpoint> {
    if !transmog_extensions::debugger::valid_breakpoints(values) {
        return Vec::new();
    }
    let Some(declaration) = snapshot
        .manifest
        .source_maps
        .as_ref()
        .filter(|value| value.version == 1 && value.modules.len() <= 64)
    else {
        return values.to_vec();
    };
    let root = Url::parse(ROOT).expect("constant root");
    let mut budget = 1024 * 1024usize;
    let mut requested = BTreeMap::<(String, u32), Vec<(usize, u32)>>::new();
    for (index, value) in values.iter().enumerate() {
        requested
            .entry((value.location.source.clone(), value.location.line))
            .or_default()
            .push((index, value.location.column));
    }
    let mut result = values.to_vec();
    let mut best = vec![None; values.len()];
    for (module, path) in &declaration.modules {
        let Some(bytes) = snapshot
            .files
            .get(path)
            .filter(|_| snapshot.files.contains_key(module))
        else {
            continue;
        };
        let Some(remaining) = budget.checked_sub(bytes.len()) else {
            continue;
        };
        budget = remaining;
        let Ok(map) = decode(bytes) else { continue };
        let Ok(base) = root.join(path) else { continue };
        let Ok(generated) = root.join(module) else {
            continue;
        };
        for token in map.tokens() {
            let Some(original) = token
                .get_source()
                .and_then(|source| base.join(source).ok())
                .filter(|url| {
                    url.as_str().starts_with(ROOT)
                        && url.query().is_none()
                        && url.fragment().is_none()
                        && url.as_str().len() <= 2048
                })
            else {
                continue;
            };
            let Some((line, column)) = token
                .get_src_line()
                .checked_add(1)
                .zip(token.get_src_col().checked_add(1))
            else {
                continue;
            };
            let Some((generated_line, generated_column)) = token
                .get_dst_line()
                .checked_add(1)
                .zip(token.get_dst_col().checked_add(1))
            else {
                continue;
            };
            if let Some(requests) = requested.get(&(original.to_string(), line)) {
                for (index, requested_column) in requests {
                    let distance = (
                        column.abs_diff(*requested_column),
                        generated_line,
                        generated_column,
                    );
                    if best[*index].is_none_or(|previous| distance < previous) {
                        best[*index] = Some(distance);
                        result[*index].location = SourceLocation {
                            source: generated.to_string(),
                            line: generated_line,
                            column: generated_column,
                        };
                    }
                }
            }
        }
    }
    result
}

pub(super) struct Mapper<'a> {
    snapshot: &'a Snapshot,
    maps: BTreeMap<String, Result<sourcemap::SourceMap, MappingIssue>>,
    remaining: usize,
}
impl<'a> Mapper<'a> {
    pub(super) fn new(snapshot: &'a Snapshot) -> Self {
        Self {
            snapshot,
            maps: BTreeMap::new(),
            remaining: 1024 * 1024,
        }
    }
    pub(super) fn map_failure(&mut self, mut failure: Failure) -> Failure {
        let snapshot = self.snapshot;
        let Some(diagnostic) = &mut failure.exception else {
            return failure;
        };
        diagnostic.artifact = Some(snapshot.digest.clone());
        for frame in diagnostic.frames_mut() {
            frame.original = None;
            frame.mapping_issue = None;
        }
        let Some(declaration) = &snapshot.manifest.source_maps else {
            return failure;
        };
        for frame in diagnostic.frames_mut() {
            let Some(generated) = &frame.generated else {
                continue;
            };
            let root = Url::parse(ROOT).expect("constant root");
            let Some(module) = snapshot.files.keys().find(|module| {
                root.join(module)
                    .is_ok_and(|url| url.as_str() == generated.source)
            }) else {
                continue;
            };
            let Some(map_path) = declaration.modules.get(module) else {
                continue;
            };
            let result = if declaration.version != 1 || declaration.modules.len() > 64 {
                Err(MappingIssue::Invalid)
            } else if !snapshot.files.contains_key(module) || !snapshot.files.contains_key(map_path)
            {
                Err(MappingIssue::Missing)
            } else {
                let map = self.maps.entry(map_path.clone()).or_insert_with(|| {
                    let bytes = &snapshot.files[map_path];
                    if bytes.len() > MAX_MAP_BYTES {
                        return Err(MappingIssue::Invalid);
                    }
                    let Some(remaining) = self.remaining.checked_sub(bytes.len()) else {
                        return Err(MappingIssue::Budget);
                    };
                    self.remaining = remaining;
                    decode(bytes)
                });
                match map {
                    Ok(map) => project(map, map_path, generated),
                    Err(issue) => Err(*issue),
                }
            };
            match result {
                Ok(original) => {
                    frame.original = Some(original);
                    frame.mapping_issue = None;
                }
                Err(issue) => {
                    frame.original = None;
                    frame.mapping_issue = Some(issue);
                }
            }
        }
        if !diagnostic.is_bounded() {
            for frame in diagnostic.frames_mut() {
                if frame.original.take().is_some() {
                    frame.mapping_issue = Some(MappingIssue::Budget);
                }
            }
            diagnostic.truncated = true;
        }
        failure
    }
}

/// Retains only code and declared original-source targets from this verified inventory.
pub(super) fn cache_sources(snapshot: &Snapshot) {
    let mut sources =
        transmog_extensions::diagnostic_sources::SourceBuilder::new(snapshot.digest.clone());
    for (path, bytes) in &snapshot.files {
        if std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext == "js" || ext == "mjs")
            && let Ok(text) = std::str::from_utf8(bytes)
            && let Ok(url) = Url::parse(ROOT).expect("constant root").join(path)
        {
            sources.add(url.as_str(), text);
        }
    }
    if let Some(declaration) = &snapshot.manifest.source_maps {
        let root = Url::parse(ROOT).expect("constant root");
        let mut remaining = 1024 * 1024usize;
        for path in declaration.modules.values().take(64) {
            let Some(bytes) = snapshot.files.get(path) else {
                continue;
            };
            let Some(budget) = remaining.checked_sub(bytes.len()) else {
                continue;
            };
            remaining = budget;
            let Ok(map) = decode(bytes) else { continue };
            let Ok(base) = root.join(path) else { continue };
            for (index, name) in map.sources().enumerate() {
                let Ok(url) = base.join(name) else { continue };
                let Some(path) = url.as_str().strip_prefix(ROOT) else {
                    continue;
                };
                if let Some(bytes) = snapshot.files.get(path).or_else(|| {
                    snapshot.files.iter().find_map(|(name, bytes)| {
                        root.join(name)
                            .ok()
                            .filter(|candidate| candidate == &url)
                            .map(|_| bytes)
                    })
                }) && let Ok(text) = std::str::from_utf8(bytes)
                {
                    sources.add(url.as_str(), text);
                } else if let Ok(index) = u32::try_from(index)
                    && let Some(text) = map.get_source_contents(index)
                {
                    sources.add(url.as_str(), text);
                }
            }
        }
    }
    sources.publish();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_maps_do_not_reconsume_work_budget_and_exhaustion_keeps_generated_positions() {
        let map = br#"{"version":3,"sources":["main.ts"],"sourcesContent":["export const original: number = 7;"],"names":[],"mappings":"AAAA"}"#;
        let manifest=serde_json::from_value(serde_json::json!({
            "formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"example.maps","version":"1.0.0","name":"Maps",
            "visualizers":[{"id":"view","name":"View","entry":"main.js","surfaces":["headers"],"inputs":{"request":{"headers":[],"body":"none"},"response":{"headers":[],"body":"none"}},"matches":[{"direction":"request","pathPrefixes":["/"]}]}],
            "sourceMaps":{"version":1,"modules":{"main.js":"main.map"}},
            "files":[{"path":"main.js","bytes":0},{"path":"main.map","bytes":map.len()}]
        })).unwrap();
        let snapshot = Snapshot::new(
            manifest,
            BTreeMap::from([
                ("main.js".into(), std::sync::Arc::<[u8]>::from([])),
                (
                    "main.map".into(),
                    std::sync::Arc::<[u8]>::from(map.as_slice()),
                ),
            ]),
        )
        .unwrap();
        cache_sources(&snapshot);
        let original_breakpoint = transmog_extensions::debugger::SourceBreakpoint {
            id: 1,
            location: SourceLocation {
                source: format!("{ROOT}main.ts"),
                line: 1,
                column: 1,
            },
        };
        let translated = debug_breakpoints(&snapshot, std::slice::from_ref(&original_breakpoint));
        assert_eq!(translated[0].location.source, format!("{ROOT}main.js"));
        let missing = transmog_extensions::debugger::SourceBreakpoint {
            location: SourceLocation {
                line: 2,
                ..original_breakpoint.location
            },
            ..original_breakpoint
        };
        assert_eq!(
            debug_breakpoints(&snapshot, std::slice::from_ref(&missing)),
            vec![missing]
        );
        let original = transmog_extensions::diagnostic_sources::read(
            &snapshot.digest,
            "file:///transmog-extension/main.ts",
            1,
            1,
        )
        .unwrap();
        assert_eq!(original.lines[0].text, "export const original: number = 7;");
        let mut failure = Failure::new(transmog_extensions::FailureKind::Exception, "safe status");
        failure.exception=Some(serde_json::from_value(serde_json::json!({
            "exception":{"name":"Error","message":"detail","frames":[{"function":"callback","generated":{"source":"file:///transmog-extension/main.js","line":1,"column":1},"original":{"source":"file:///forged.ts","line":1,"column":1}}],"unavailable":false},"causes":[],"truncated":false
        })).unwrap());
        let mut mapper = Mapper::new(&snapshot);
        mapper.remaining = map.len();
        for _ in 0..2 {
            let outcome = mapper.map_failure(failure.clone());
            assert_eq!(
                outcome.exception.unwrap().exception.frames[0]
                    .original
                    .as_ref()
                    .unwrap()
                    .source,
                "file:///transmog-extension/main.ts"
            );
            assert_eq!(mapper.remaining, 0);
        }
        let mut exhausted = Mapper::new(&snapshot);
        exhausted.remaining = 0;
        let outcome = exhausted.map_failure(failure);
        assert_eq!(outcome.message, "safe status");
        let diagnostic = outcome.exception.unwrap();
        assert_eq!(
            diagnostic.exception.frames[0].mapping_issue,
            Some(MappingIssue::Budget)
        );
        assert!(diagnostic.exception.frames[0].original.is_none());
        assert!(diagnostic.exception.frames[0].generated.is_some());
    }

    #[test]
    fn maps_are_bounded_before_token_decoding_and_never_resolve_external_sources() {
        let make = |source: &str| {
            serde_json::to_vec(&serde_json::json!({
            "version":3,"sources":[source],"names":[],"mappings":"AACI","sourcesContent":["source"]
        })).unwrap()
        };
        let generated = SourceLocation {
            source: format!("{ROOT}src/main.js"),
            line: 1,
            column: 1,
        };
        let map = decode(&make("../src/main.ts")).unwrap();
        let original = project(&map, "maps/main.js.map", &generated).unwrap();
        assert_eq!(original.source, format!("{ROOT}src/main.ts"));
        assert_eq!((original.line, original.column), (2, 5));
        let unmapped = SourceLocation {
            line: 2,
            ..generated.clone()
        };
        assert_eq!(
            project(&map, "maps/main.js.map", &unmapped),
            Err(MappingIssue::Unmapped)
        );
        for source in [
            "https://example.test/code.ts",
            "file:///private/code.ts",
            "../../../private.ts",
        ] {
            assert_eq!(
                project(
                    &decode(&make(source)).unwrap(),
                    "maps/main.js.map",
                    &generated
                ),
                Err(MappingIssue::Source)
            );
        }
        assert!(decode(&vec![b' '; MAX_MAP_BYTES + 1]).is_err());
        let oversized = serde_json::to_vec(&serde_json::json!({"version":3,"sources":["main.ts"],"names":[],"mappings":"AAAA,".repeat(MAX_MAP_TOKENS)})).unwrap();
        assert!(decode(&oversized).is_err());
        assert!(decode(br#"{"version":3,"sections":[]}"#).is_err());
    }
}
