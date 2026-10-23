#![deny(missing_docs)]

//! Runtime-free SWC parsing and TypeScript emission shared by both script hosts.
//!
//! Keep the parser versions, syntax switches and transform configuration pinned:
//! changing a compiler default must not silently change executable contracts.

use std::sync::{Arc, Mutex};

use swc_common::{
    FileName, GLOBALS, Globals, Mark, SourceMap, Spanned,
    comments::SingleThreadedComments,
    errors::{
        Diagnostic, DiagnosticBuilder, Emitter as DiagnosticEmitter, HANDLER, Handler, Level,
    },
    source_map::{SmallPos, SourceMapGenConfig},
    sync::Lrc,
};
use swc_ecma_ast::{EsVersion, Module, Program};
use swc_ecma_codegen::{Emitter, Node, text_writer::JsWriter};
use swc_ecma_parser::{
    EsSyntax, Parser, StringInput, Syntax, TsSyntax, error::SyntaxError, lexer::Lexer,
};
use swc_ecma_transforms_base::{fixer::fixer, helpers, hygiene::hygiene, resolver};
use swc_ecma_transforms_typescript::{
    Config as TypeScriptConfig, ImportsNotUsedAsValues, TsImportExportAssignConfig, typescript,
};
use url::Url;

/// SWC AST types used by the authoritative contract validators.
pub use swc_ecma_ast as ast;
/// SWC traversal traits used by the authoritative contract validators.
pub use swc_ecma_visit as visit;

// Match the existing parser and emitter target. This does not downlevel runtime
// JavaScript or expand the extensions' ES2022 compatibility promise.
const ES_VERSION: EsVersion = EsVersion::Es2021;

/// The executable language of one module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModuleSyntax {
    /// Packaged JavaScript, parsed without accepting TypeScript annotations.
    JavaScript,
    /// Traffic-script TypeScript, with JSX disabled.
    TypeScript,
}

/// A parsed module and the source metadata required for a separate source map.
pub struct ParsedModule {
    module: Module,
    comments: SingleThreadedComments,
    source_map: Lrc<SourceMap>,
    recoverable_errors: Vec<swc_ecma_parser::error::Error>,
}

/// Executable JavaScript and a separate map with original source contents.
pub struct EmittedModule {
    /// Emitted ECMAScript module text, retaining comments.
    pub text: String,
    /// UTF-8 JSON source map with the original module URL and inline source.
    pub source_map: Vec<u8>,
}

/// Parses a module without loading dependencies or accessing the filesystem.
///
/// Recoverable TypeScript diagnostics are retained until emission so callers can
/// first apply their existing export and capability validation rules.
///
/// # Errors
/// Returns a parser diagnostic with its module URL and one-based position.
pub fn parse_module(
    specifier: Url,
    source: &str,
    syntax: ModuleSyntax,
) -> Result<ParsedModule, String> {
    let source_map: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let file = source_map.new_source_file(FileName::Url(specifier).into(), source.to_owned());
    let comments = SingleThreadedComments::default();
    let syntax = match syntax {
        ModuleSyntax::TypeScript => Syntax::Typescript(TsSyntax {
            decorators: true,
            disallow_ambiguous_jsx_like: false,
            dts: false,
            tsx: false,
            no_early_errors: false,
        }),
        ModuleSyntax::JavaScript => Syntax::Es(EsSyntax {
            allow_return_outside_function: true,
            allow_super_outside_method: true,
            auto_accessors: true,
            decorators: true,
            decorators_before_export: false,
            export_default_from: true,
            fn_bind: false,
            import_attributes: true,
            jsx: false,
            explicit_resource_management: true,
        }),
    };
    let lexer = Lexer::new(
        syntax,
        ES_VERSION,
        StringInput::from(&*file),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser
        .parse_module()
        .map_err(|error| parser_error(&source_map, &error))?;
    let recoverable_errors = parser.take_errors();
    Ok(ParsedModule {
        module,
        comments,
        source_map,
        recoverable_errors,
    })
}

impl ParsedModule {
    /// Borrows the module before any transforms modify its executable syntax.
    #[must_use]
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Removes TypeScript using the existing traffic-script emission settings.
    ///
    /// This performs no semantic type checking and emits no runtime imports.
    ///
    /// # Errors
    /// Returns fatal syntax, transform or source-map emission diagnostics.
    pub fn transpile_typescript(self) -> Result<EmittedModule, String> {
        if let Some(error) = self
            .recoverable_errors
            .iter()
            .find(|error| fatal_syntax(error.kind()))
        {
            return Err(parser_error(&self.source_map, error));
        }
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let handler = Handler::with_emitter(
            true,
            false,
            Box::new(TransformDiagnostics(Arc::clone(&diagnostics))),
        );
        let globals = Globals::default();
        let program = GLOBALS.set(&globals, || {
            let unresolved = Mark::new();
            let top_level = Mark::new();
            HANDLER.set(&handler, || {
                helpers::HELPERS.set(&helpers::Helpers::new(false), || {
                    Program::Module(self.module).apply((
                        resolver(unresolved, top_level, true),
                        helpers::inject_helpers(top_level),
                        typescript(
                            TypeScriptConfig {
                                verbatim_module_syntax: false,
                                import_not_used_as_values: ImportsNotUsedAsValues::default(),
                                no_empty_export: true,
                                import_export_assign_config: TsImportExportAssignConfig::Preserve,
                                ts_enum_is_mutable: true,
                                native_class_properties: false,
                            },
                            unresolved,
                            top_level,
                        ),
                        fixer(Some(&self.comments)),
                        hygiene(),
                    ))
                })
            })
        });
        if let Some(diagnostic) = diagnostics
            .lock()
            .map_err(|_| "Compiler diagnostics unavailable")?
            .first()
        {
            return Err(diagnostic_message(&self.source_map, diagnostic));
        }

        let mut text = Vec::new();
        let mut mappings = Vec::new();
        {
            let mut writer = JsWriter::new(
                self.source_map.clone(),
                "\n",
                &mut text,
                Some(&mut mappings),
            );
            writer.set_indent_str("  ");
            let mut config = swc_ecma_codegen::Config::default();
            config.target = ES_VERSION;
            config.ascii_only = false;
            config.minify = false;
            config.omit_last_semi = false;
            config.emit_assert_for_import_attributes = false;
            config.inline_script = false;
            let mut emitter = Emitter {
                cfg: config,
                comments: Some(&self.comments),
                cm: self.source_map.clone(),
                wr: Box::new(writer),
            };
            program
                .emit_with(&mut emitter)
                .map_err(|error| error.to_string())?;
        }
        let mut source_map = Vec::new();
        self.source_map
            .build_source_map(&mappings, None, InlineOriginalSource)
            .to_writer(&mut source_map)
            .map_err(|error| error.to_string())?;
        Ok(EmittedModule {
            text: String::from_utf8(text).map_err(|error| error.to_string())?,
            source_map,
        })
    }
}

struct InlineOriginalSource;
impl SourceMapGenConfig for InlineOriginalSource {
    fn file_name_to_source(&self, file: &FileName) -> String {
        file.to_string()
    }
    fn inline_sources_content(&self, file: &FileName) -> bool {
        !matches!(file, FileName::Real(_) | FileName::Custom(_))
    }
}

struct TransformDiagnostics(Arc<Mutex<Vec<Diagnostic>>>);
impl DiagnosticEmitter for TransformDiagnostics {
    fn emit(&mut self, diagnostic: &mut DiagnosticBuilder<'_>) {
        if !matches!(diagnostic.level, Level::Help | Level::Note | Level::Warning)
            && let Ok(mut diagnostics) = self.0.lock()
        {
            diagnostics.push(diagnostic.take());
        }
    }
}

fn parser_error(source_map: &SourceMap, error: &swc_ecma_parser::error::Error) -> String {
    let location = source_map.lookup_byte_offset(error.span().lo);
    let source = location.sf.src.as_str();
    let offset = source.floor_char_boundary(location.pos.to_usize().min(source.len()));
    let prefix = &source[..offset];
    // Parser diagnostics historically count a tab as two columns and every
    // Unicode scalar as one. SWC's terminal display column uses different widths.
    // Keep this separate from transform diagnostics, whose SWC columns are public.
    let line_number = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |position| position + 1);
    let line_prefix = &source[line_start..offset];
    let column_number = line_prefix
        .chars()
        .map(|character| if character == '\t' { 2 } else { 1 })
        .sum::<usize>()
        + 1;
    let line = source[line_start..]
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r');
    let start_column = line_prefix.chars().count();
    let end_offset = error
        .span()
        .hi
        .to_usize()
        .saturating_sub(location.sf.start_pos.to_usize());
    let end_offset = source.floor_char_boundary(end_offset.min(line_start + line.len()));
    let end_column = source[line_start..end_offset]
        .chars()
        .count()
        .max(start_column);
    let snippet = parser_snippet(line, start_column, end_column, line_number);
    format!(
        "SyntaxError: {}\n{snippet}\n    at {}:{line_number}:{column_number}",
        error.kind().msg(),
        location.sf.name,
    )
}

fn parser_snippet(line: &str, start: usize, end: usize, line_number: usize) -> String {
    // Bound the snippet independently of source length. The public caller also
    // bounds the full diagnostic, but an invalid very long line must stay cheap.
    let length = line.chars().count();
    let crop_start = if length > 90 && start > 60 {
        (start - 20).min(length - 80)
    } else {
        0
    };
    let mut text = line
        .chars()
        .skip(crop_start)
        .take(if length > 90 { 80 } else { 90 })
        .collect::<String>();
    let kept = text.chars().count();
    let mut underline_start = start.saturating_sub(crop_start);
    let mut underline_end = end.saturating_sub(crop_start);
    if crop_start > 0 {
        text.insert_str(0, "...");
        underline_start += 3;
        underline_end += 3;
    }
    if length > crop_start + kept {
        underline_end = underline_end.min(text.chars().count());
        text.push_str("...");
    }
    let underline = format!(
        "{}{}",
        " ".repeat(underline_start),
        "~".repeat(underline_end.saturating_sub(underline_start).max(1))
    );
    let margin = " ".repeat(line_number.to_string().len());
    format!("{margin} |\n{line_number} | {text}\n{margin} | {underline}")
}

fn diagnostic_message(source_map: &SourceMap, diagnostic: &Diagnostic) -> String {
    if let Some(span) = diagnostic.span.primary_span() {
        let location = source_map.lookup_char_pos(span.lo);
        format!(
            "{} at {}:{}:{}",
            diagnostic.message(),
            location.file.name,
            location.line,
            location.col_display + 1
        )
    } else {
        diagnostic.message()
    }
}

fn fatal_syntax(error: &SyntaxError) -> bool {
    matches!(
        error,
        SyntaxError::TS1003
            | SyntaxError::TS1005
            | SyntaxError::TS1085
            | SyntaxError::LegacyOctal
            | SyntaxError::LegacyDecimal
            | SyntaxError::TS1109
            | SyntaxError::TS2406
            | SyntaxError::UnterminatedStrLit
            | SyntaxError::NullishCoalescingWithLogicalOp
            | SyntaxError::InitRequiredForUsingDecl
            | SyntaxError::Expected(_, _)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transpile(source: &str) -> EmittedModule {
        parse_module(
            Url::parse("file:///transmog-scripts/example/main.ts").unwrap(),
            source,
            ModuleSyntax::TypeScript,
        )
        .unwrap()
        .transpile_typescript()
        .unwrap()
    }

    #[test]
    fn type_imports_comments_and_original_source_maps_are_preserved() {
        let source = "import type { Action } from 'transmog:api/v1';\n// keep this comment\nexport function onRequestHead(): Action {\n  const answer: number = 42;\n  return { action: 'continue', answer };\n}\n";
        let output = transpile(source);
        assert!(!output.text.contains("transmog:api/v1"));
        assert!(!output.text.contains(": number"));
        assert!(output.text.contains("// keep this comment"));
        assert!(!output.text.contains("sourceMappingURL"));
        let map = sourcemap::SourceMap::from_slice(&output.source_map).unwrap();
        assert_eq!(
            map.get_source(0),
            Some("file:///transmog-scripts/example/main.ts")
        );
        assert_eq!(map.get_source_contents(0), Some(source));
        let generated_line = output
            .text
            .lines()
            .position(|line| line.contains("const answer"))
            .unwrap();
        let token = map
            .lookup_token(u32::try_from(generated_line).unwrap(), 2)
            .unwrap();
        assert_eq!(token.get_src_line(), 3);
    }

    #[test]
    fn enums_namespaces_parameter_properties_and_assertions_are_lowered() {
        let output = transpile(
            "enum Choice { First = 3, Second }\nnamespace Values { export const value = Choice.Second; }\nclass Selection { constructor(public value: number = Values.value) {} }\nexport function onRequestHead() { return new Selection().value satisfies number; }\n",
        );
        assert!(output.text.contains("Choice[\"First\"] = 3"));
        assert!(output.text.contains("Values.value = Choice.Second"));
        assert!(output.text.contains("this.value = value"));
        assert!(!output.text.contains("satisfies"));
        parse_module(
            Url::parse("file:///output.js").unwrap(),
            &output.text,
            ModuleSyntax::JavaScript,
        )
        .unwrap();
    }

    #[test]
    fn javascript_rejects_typescript_and_fatal_recovered_errors_block_emit() {
        assert!(
            parse_module(
                Url::parse("file:///main.js").unwrap(),
                "export function visualize(input: unknown) {}",
                ModuleSyntax::JavaScript
            )
            .is_err()
        );
        let source = "export function onRequestHead() { const invalid = 010; }";
        let parsed = parse_module(
            Url::parse("file:///main.ts").unwrap(),
            source,
            ModuleSyntax::TypeScript,
        )
        .unwrap();
        assert!(parsed.transpile_typescript().is_err());
    }

    #[test]
    fn independent_compilations_do_not_share_resolver_marks_or_source_files() {
        let source = "type Item = string; export function onRequestHead(Item: Item) { return { value: Item }; }";
        let first = transpile(source);
        let second = std::thread::spawn(move || transpile(source))
            .join()
            .unwrap();
        assert_eq!(first.text, second.text);
        assert_eq!(first.source_map, second.source_map);
    }

    #[test]
    fn parser_diagnostics_keep_scalar_columns_two_space_tabs_and_source_snippets() {
        for (source, position) in [
            ("\tconst = 1;", ":1:9"),
            ("\t\tconst = 1;", ":1:11"),
            ("const value = '界😀'; const = 1;", ":1:27"),
            ("const value = 1;\r\n\tconst = 1;", ":2:9"),
            ("\u{feff}\tconst = 1;", ":1:9"),
            ("const value = 1;\u{2028}\tconst = 1;", ":1:26"),
        ] {
            let error = parse_module(
                Url::parse("file:///main.ts").unwrap(),
                source,
                ModuleSyntax::TypeScript,
            )
            .err()
            .unwrap();
            assert!(error.starts_with("SyntaxError: "), "{error}");
            assert!(error.contains(" | "), "{error}");
            assert!(error.contains('~'), "{error}");
            assert!(error.ends_with(position), "{error}");
        }
        let source = format!("{}const = 1;", " ".repeat(10_000));
        let error = parse_module(
            Url::parse("file:///main.ts").unwrap(),
            &source,
            ModuleSyntax::TypeScript,
        )
        .err()
        .unwrap();
        assert!(error.contains("..."));
        assert!(error.len() < 300);
    }
}
