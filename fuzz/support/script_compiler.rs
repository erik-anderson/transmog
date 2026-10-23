//! Runtime-free script compiler cases shared by fuzzing and ordinary seed replay.

use std::collections::BTreeSet;

use transmog_script::{
    SCRIPT_API_REVISION, ScriptCapabilities, ScriptHandler, ScriptLimits, ScriptManifest,
    ScriptPrefilter, compile_typescript, source_hash,
};
use transmog_script_compiler::{ModuleSyntax, ast::ModuleDecl, ast::ModuleItem, parse_module};
use url::Url;

const MAX_RAW_BYTES: usize = 16 * 1024;

/// Exercises malformed sources and compiler invariants without executing JavaScript.
pub fn exercise(data: &[u8]) {
    let data = &data[..data.len().min(MAX_RAW_BYTES)];
    match data.split_first() {
        Some((b'S', data)) => structured(data),
        Some((b'F', data)) => forbidden(data),
        Some((b'D', data)) => raw_source(&deep_source(data)),
        Some((b'J' | b'T', data)) => raw_source(&String::from_utf8_lossy(data)),
        _ => raw_source(&String::from_utf8_lossy(data)),
    }
}

fn structured(data: &[u8]) {
    let selector = data.first().copied().unwrap_or_default();
    let mut expression = format!("{}", data.get(1).copied().unwrap_or_default());
    for choice in data.iter().copied().skip(2).take(24) {
        expression = match choice % 8 {
            0 => format!("[{expression}]"),
            1 => format!("({{value:{expression}}})"),
            2 => format!("({expression} as unknown)"),
            3 => format!("({expression} satisfies unknown)"),
            4 => format!("((value:unknown)=>value)({expression})"),
            5 => format!("`value:${{{expression}}}`"),
            6 => format!("(true ? {expression} : null)"),
            _ => format!("({expression} ?? 0)"),
        };
    }
    let declarations = match selector % 8 {
        0 => "enum Choice { First=3, Second }",
        1 => "namespace Values { export const value:number=42; }",
        2 => "class Item { constructor(public value:number=7){} }",
        3 => "const identity = <T>(value:T):T=>value;",
        4 => "type Lookup<T> = {[K in keyof T]?:T[K]}; interface Item {value:string}",
        5 => "const pattern = /[a-z\\]{}]+/gu; const text = '界😀\\u0000';",
        6 => "const enum Bits { First=1, Second=2 }",
        _ => "class Item { #value=1; get value():number{return this.#value;} }",
    };
    let bom = if selector & 128 == 0 { "" } else { "\u{feff}" };
    let source = format!(
        "{bom}import type {{ScriptAction}} from 'transmog:api/v1';\r\n// retained authoring comment\r\n{declarations}\r\nexport function onRequestHead(_context:unknown):ScriptAction {{ const value:unknown={expression}; return {{action:'continue'}}; }}"
    );
    let compiled = compile_typescript(manifest(&source), &source)
        .expect("Structured valid script must reach TypeScript emission");
    let url = Url::parse("file:///transmog-scripts/fuzz/main.ts").unwrap();
    emitted(
        &compiled.javascript,
        &compiled.source_map,
        &source,
        &url,
        &BTreeSet::new(),
    );
    parse_module(url, &compiled.javascript, ModuleSyntax::JavaScript)
        .expect("Structured emission must be JavaScript");
    raw_source(&source);
}

fn forbidden(data: &[u8]) {
    let fragment = match data.first().copied().unwrap_or_default() % 12 {
        0 => "import value from './outside.js'; export function onRequestHead(){return value;}",
        1 => "export * from 'node:fs'; export function onRequestHead(){}",
        2 => {
            "export {value} from 'https://example.invalid/code.js'; export function onRequestHead(){}"
        }
        3 => "export function onRequestHead(){return eval('1');}",
        4 => "export function onRequestHead(){return new Function('return 1');}",
        5 => "export function onRequestHead(){return import('./outside.js');}",
        6 => "export async function onRequestHead(){return {action:'continue'};}",
        7 => "export function* onRequestHead(){yield {action:'continue'};}",
        8 => "export function onRequestHead(){} export function onResponseHead(){}",
        9 => "export default function onRequestHead(){}",
        10 => {
            "import type {Action} from './outside.ts'; export function onRequestHead():Action{return {action:'continue'};}"
        }
        _ => "import value = require('node:fs'); export function onRequestHead(){return value;}",
    };
    assert!(
        compile_typescript(manifest(fragment), fragment).is_err(),
        "Forbidden source was accepted"
    );
    raw_source(fragment);
}

fn deep_source(data: &[u8]) -> String {
    let kind = data.first().copied().unwrap_or_default() % 12;
    let depth = [16, 64, 128, 256, 1024, 4096, 2000]
        [usize::from(data.get(1).copied().unwrap_or_default()) % 7];
    let malformed = data.get(2).is_some_and(|selector| selector & 1 != 0);
    if kind == 9 || kind == 10 || kind == 11 {
        let declaration = match kind {
            9 => format!("type Value = A{};", ".B".repeat(depth)),
            10 => format!("type Value = number{};", "[]".repeat(depth)),
            _ => format!(
                "namespace A{} {{ export const value=1; }}",
                ".B".repeat(depth)
            ),
        };
        return format!(
            "{declaration} export function onRequestHead(){{return {{action:'continue'}};}}"
        );
    }
    let expression = match kind {
        0 => format!("{}1{}", "[".repeat(depth), "]".repeat(depth)),
        1 => format!("{}0", "!".repeat(depth)),
        2 => format!("0{}", "+1".repeat(depth)),
        3 => format!("value{}", ".item".repeat(depth)),
        4 => format!("value{}", "()".repeat(depth)),
        5 => format!("{}0", "true?0:".repeat(depth)),
        6 => format!("{}0", "()=>".repeat(depth)),
        7 => format!("{}1{}", "({value:".repeat(depth), "})".repeat(depth)),
        _ => format!(
            "{}value{}",
            "(".repeat(depth),
            format!("){}", ".item".repeat(100)).repeat(depth)
        ),
    };
    let tail = if malformed { "+ ;" } else { ";" };
    format!(
        "export function onRequestHead(){{const value={expression}{tail}return {{action:'continue'}};}}"
    )
}

fn raw_source(source: &str) {
    let url = Url::parse("file:///transmog-scripts/fuzz/main.ts").unwrap();
    for syntax in [ModuleSyntax::JavaScript, ModuleSyntax::TypeScript] {
        let first = parse_module(url.clone(), source, syntax);
        let second = parse_module(url.clone(), source, syntax);
        match (first, second) {
            (Ok(first), Ok(second)) => {
                assert_eq!(dependencies(first.module()), dependencies(second.module()));
                let original_dependencies = dependencies(first.module());
                match (first.transpile_typescript(), second.transpile_typescript()) {
                    (Ok(first), Ok(second)) => {
                        assert_eq!(first.text, second.text);
                        assert_eq!(first.source_map, second.source_map);
                        emitted(
                            &first.text,
                            &first.source_map,
                            source,
                            &url,
                            &original_dependencies,
                        );
                    }
                    (Err(first), Err(second)) => assert_eq!(first, second),
                    _ => panic!("TypeScript emission is nondeterministic"),
                }
            }
            (Err(first), Err(second)) => assert_eq!(first, second),
            _ => panic!("Source parsing is nondeterministic"),
        }
    }
    let manifest = manifest(source);
    match (
        compile_typescript(manifest.clone(), source),
        compile_typescript(manifest, source),
    ) {
        (Ok(first), Ok(second)) => {
            assert_eq!(first, second);
            emitted(
                &first.javascript,
                &first.source_map,
                source,
                &url,
                &BTreeSet::new(),
            );
            parse_module(url, &first.javascript, ModuleSyntax::JavaScript)
                .expect("Authoritative compilation must emit JavaScript");
        }
        (Err(first), Err(second)) => assert_eq!(first, second),
        _ => panic!("Authoritative script compilation is nondeterministic"),
    }
}

fn manifest(source: &str) -> ScriptManifest {
    ScriptManifest {
        id: "fuzz".into(),
        revision: 1,
        api_revision: SCRIPT_API_REVISION,
        source_hash: source_hash(source.as_bytes()),
        handlers: BTreeSet::from([ScriptHandler::RequestHead]),
        prefilter: ScriptPrefilter::default(),
        capabilities: ScriptCapabilities::default(),
        limits: ScriptLimits::default(),
        priority: 0,
    }
}

fn dependencies(module: &transmog_script_compiler::ast::Module) -> BTreeSet<String> {
    module
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) if !import.type_only => {
                Some(import.src.value.to_string_lossy().into_owned())
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportAll(export)) if !export.type_only => {
                Some(export.src.value.to_string_lossy().into_owned())
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if !export.type_only => export
                .src
                .as_ref()
                .map(|source| source.value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn emitted(
    text: &str,
    map: &[u8],
    source: &str,
    url: &Url,
    original_dependencies: &BTreeSet<String>,
) {
    // This is a harness resource bound, not a production output-size promise.
    let budget = source.len().saturating_mul(256).saturating_add(128 * 1024);
    assert!(text.len() <= budget && map.len() <= budget);
    let parsed_map =
        sourcemap::SourceMap::from_slice(map).expect("Compiler emitted an invalid source map");
    for index in 0..parsed_map.get_source_count() {
        assert_eq!(parsed_map.get_source(index), Some(url.as_str()));
        assert_eq!(
            parsed_map.get_source_contents(index),
            Some(source.strip_prefix('\u{feff}').unwrap_or(source))
        );
    }
    if let Ok(parsed) = parse_module(url.clone(), text, ModuleSyntax::TypeScript) {
        assert!(
            dependencies(parsed.module()).is_subset(original_dependencies),
            "Compiler introduced a runtime dependency"
        );
    }
}
