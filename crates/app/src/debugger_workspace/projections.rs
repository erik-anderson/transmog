//! Breakpoint intent shared by both verified source representations.
use super::{DebuggerFile, WorkspaceBreakpoint};
use std::{collections::BTreeMap, sync::Arc};
use transmog_script_diagnostics::SourceLocation;

#[derive(Clone)]
struct Position {
    source: Arc<str>,
    line: u32,
    column: u32,
}
impl Position {
    fn location(&self) -> SourceLocation {
        SourceLocation {
            source: self.source.to_string(),
            line: self.line,
            column: self.column,
        }
    }
}

#[derive(Clone)]
pub(super) struct Map {
    artifact: String,
    generated: String,
    tokens: Vec<(Position, Position)>,
}
impl Map {
    pub(super) fn read(artifact: &str, generated: &str, base: &str, bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 512 * 1024 || generated.len() > 2048 {
            return None;
        }
        let sourcemap::DecodedMap::Regular(map) = sourcemap::decode_slice(bytes).ok()? else {
            return None;
        };
        if map.get_token_count() > 32768 || map.get_source_count() > 128 {
            return None;
        }
        let base = url::Url::parse(base).ok()?;
        let root = if generated.starts_with("file:///transmog-extension/") {
            "file:///transmog-extension/"
        } else {
            "file:///transmog-scripts/"
        };
        // Intern names: a map may reference the same long URL thousands of times.
        let generated_source: Arc<str> = generated.into();
        let mut names = BTreeMap::<String, Option<Arc<str>>>::new();
        let tokens = map
            .tokens()
            .filter_map(|token| {
                let name = token.get_source()?;
                let source = if let Some(source) = names.get(name) {
                    source.clone()?
                } else {
                    let source = base
                        .join(name)
                        .ok()
                        .filter(|source| {
                            source.as_str().starts_with(root)
                                && source.query().is_none()
                                && source.fragment().is_none()
                                && source.as_str().len() <= 2048
                        })
                        .map(|source| Arc::from(source.as_str()));
                    names.insert(name.into(), source.clone());
                    source?
                };
                Some((
                    Position {
                        source: Arc::clone(&generated_source),
                        line: token.get_dst_line().checked_add(1)?,
                        column: token.get_dst_col().checked_add(1)?,
                    },
                    Position {
                        source,
                        line: token.get_src_line().checked_add(1)?,
                        column: token.get_src_col().checked_add(1)?,
                    },
                ))
            })
            .collect();
        Some(Self {
            artifact: artifact.into(),
            generated: generated.into(),
            tokens,
        })
    }
    pub(super) fn key(&self) -> (String, String) {
        (self.artifact.clone(), self.generated.clone())
    }
    pub(super) fn artifact(&self) -> &str {
        &self.artifact
    }
}

pub(super) fn canonicalize(
    point: &mut WorkspaceBreakpoint,
    files: &[(DebuggerFile, String)],
    maps: &[Map],
) {
    let Some(artifact) = point.artifact.as_deref() else {
        return;
    };
    let Some((file, _)) = files.iter().find(|(file, _)| {
        file.id == point.file && file.artifact == artifact && file.language == "javascript"
    }) else {
        return;
    };
    let Some((_, original)) = maps
        .iter()
        .filter(|map| map.artifact == artifact && map.generated == file.source)
        .flat_map(|map| &map.tokens)
        .filter(|(generated, _)| generated.line == point.breakpoint.location.line)
        .min_by_key(|(generated, _)| generated.column.abs_diff(point.breakpoint.location.column))
    else {
        return;
    };
    let Some((file, text)) = files
        .iter()
        .find(|(file, _)| file.artifact == artifact && file.source == original.source.as_ref())
    else {
        return;
    };
    let Some(anchor) = text.lines().nth(original.line as usize - 1) else {
        return;
    };
    point.file.clone_from(&file.id);
    point.anchor = anchor.into();
    point.breakpoint.location = original.location();
}

pub(super) fn project(
    point: &WorkspaceBreakpoint,
    file: &DebuggerFile,
    files: &[(DebuggerFile, String)],
    maps: &[Map],
) -> Option<SourceLocation> {
    let (original, text) = files
        .iter()
        .find(|(source, _)| source.id == point.file && source.artifact == file.artifact)?;
    let position = super::rebind(point, text)?.location;
    if original.id == file.id {
        return Some(position);
    }
    maps.iter()
        .filter(|map| map.artifact == file.artifact && map.generated == file.source)
        .flat_map(|map| &map.tokens)
        .filter(|(_, original)| {
            original.source.as_ref() == position.source && original.line == position.line
        })
        .min_by_key(|(generated, original)| {
            (
                original.column.abs_diff(position.column),
                generated.line,
                generated.column,
            )
        })
        .map(|(generated, _)| generated.location())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_representations_share_one_logical_point_without_loading_a_worker() {
        let generated = "file:///transmog-script/main.js";
        let original = "file:///transmog-scripts/sample/main.ts";
        let bytes=serde_json::to_vec(&serde_json::json!({"version":3,"sources":[original],"names":[],"mappings":"AAAA;AACA"})).unwrap();
        let maps = vec![Map::read("artifact", generated, generated, &bytes).unwrap()];
        let file = |id: &str, source: &str, language: &str| DebuggerFile {
            id: id.into(),
            group: "User scripts".into(),
            package: "sample".into(),
            path: id.into(),
            artifact: "artifact".into(),
            source: source.into(),
            language: language.into(),
            counterpart: None,
            state: "Unloaded".into(),
        };
        let files = vec![
            (
                file("script/sample/main.js", generated, "javascript"),
                "const a=1;\nconst b=2;".into(),
            ),
            (
                file("script/sample/main.ts", original, "typescript"),
                "const a:number=1;\nconst b:number=2;".into(),
            ),
        ];
        let mut point = WorkspaceBreakpoint {
            artifact: Some("artifact".into()),
            file: files[0].0.id.clone(),
            anchor: "const b=2;".into(),
            enabled: true,
            breakpoint: transmog_script_diagnostics::debugger::SourceBreakpoint {
                id: 1,
                location: SourceLocation {
                    source: generated.into(),
                    line: 2,
                    column: 1,
                },
                log_expression: None,
                condition: Some("b===2".into()),
            },
        };
        canonicalize(&mut point, &files, &maps);
        assert_eq!(point.file, files[1].0.id);
        assert_eq!(point.anchor, "const b:number=2;");
        assert_eq!(
            project(&point, &files[0].0, &files, &maps).unwrap().source,
            generated
        );
        assert_eq!(
            project(&point, &files[1].0, &files, &maps).unwrap().source,
            original
        );
        point.anchor = "removed".into();
        assert!(project(&point, &files[0].0, &files, &maps).is_none());
        assert!(project(&point, &files[1].0, &files, &maps).is_none());
    }
}
