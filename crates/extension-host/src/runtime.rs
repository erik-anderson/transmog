use std::collections::{BTreeMap, BTreeSet};
use transmog_extensions::{
    Failure, FailureKind, Invocation, Snapshot, VisualizationStep, Visualizer,
};
use transmog_js_runtime::{ErrorKind, Runtime as Engine};
use transmog_script_compiler::{
    ModuleSyntax,
    ast::{
        CallExpr, Callee, Decl, ExportSpecifier, Expr, Module, ModuleDecl, ModuleExportName,
        ModuleItem, NewExpr, Stmt,
    },
    parse_module_detailed,
    visit::{Visit, VisitWith},
};
use url::Url as ModuleSpecifier;

const BOOTSTRAP: &str = r"
(() => {
  const parse = JSON.parse, stringify = JSON.stringify;
  const freeze = (v, seen = new WeakSet()) => {
    if (v === null || (typeof v !== 'object' && typeof v !== 'function') || seen.has(v)) return v;
    seen.add(v); for (const key of Reflect.ownKeys(v)) freeze(v[key], seen); return Object.freeze(v);
  };
  Object.defineProperty(globalThis, '__tmInvoke', {value: (inputJson) => {
    const call = freeze(parse(inputJson));
    const handlers = globalThis.__tmHandlers;
    let result;
    if (call.kind === 'visualize') result = handlers.visualize(call.input);
    else if (call.kind === 'resume') result = handlers.resume(call.token, call.pages);
    else if (call.kind === 'resumeContext') result = handlers.resumeContext(call.token, call.page);
    else if (call.kind === 'resumeDecoded') result = handlers.resumeDecoded(call.token, call.result);
    else if (call.kind === 'expand') result = handlers.expand(call.token);
    else if (call.kind === 'traffic') result = handlers.onTraffic(call.events, freeze({missedHints:call.missedHints}));
    else throw new Error('Unknown operation');
    if (result && typeof result.then === 'function') throw new Error('Synchronous output required');
    const output = stringify(result);
    if (typeof output !== 'string') throw new Error('Serializable output required');
    return output;
  }});
  for (const key of ['Deno','fetch','WebSocket','XMLHttpRequest','Worker','SharedWorker','WebAssembly',
      'eval','Function','setTimeout','setInterval','queueMicrotask','process','require']) {
    delete globalThis[key];
  }
})();
";

pub(super) struct Runtime {
    pub(super) engine: Engine,
    visualizer: Visualizer,
}
impl Runtime {
    pub(super) fn new(
        snapshot: &Snapshot,
        visualizer_id: &str,
        sink: impl Fn(u64, transmog_extensions::ConsoleEvent) -> bool + 'static,
        attachment: Option<transmog_js_runtime::debugger::DebugAttachment>,
    ) -> Result<Self, Failure> {
        let visualizer = snapshot
            .manifest
            .visualizers
            .iter()
            .find(|v| v.id == visualizer_id)
            .ok_or_else(|| fail(FailureKind::Validation, "Unknown visualizer contribution"))?
            .clone();
        let mut runtime =
            Engine::new(8 * 1024 * 1024, visualizer.limits.heap_bytes).map_err(|_| {
                fail(
                    FailureKind::Unavailable,
                    "The visualizer runtime could not initialize",
                )
            })?;
        runtime.enable_console(sink).map_err(|_| {
            fail(
                FailureKind::Unavailable,
                "Runtime console could not initialize",
            )
        })?;
        runtime.begin_console(0);
        if let Some(attachment) = attachment {
            let (options, transport, _) = attachment.into_parts();
            runtime
                .enable_debugger(options, transport)
                .map_err(|_| fail(FailureKind::Unavailable, "Debugger unavailable"))?;
        }
        runtime.begin_debug_operation(0);
        let initialized = (|| -> Result<(), Failure> {
            let (entry, modules) = load_modules(snapshot, &visualizer)?;
            // Install capability restrictions before executing any extension module initializer.
            runtime
                .execute_script("transmog:extension-bootstrap", BOOTSTRAP)
                .map_err(|_| {
                    fail(
                        FailureKind::Unavailable,
                        "Runtime restrictions could not initialize",
                    )
                })?;
            runtime.evaluate_modules(&entry, modules).map_err(|error| {
                if error.kind == ErrorKind::Module {
                    engine_failure(
                        FailureKind::Validation,
                        "Package module loading failed",
                        error,
                    )
                } else {
                    engine_failure(
                        FailureKind::Exception,
                        "Extension initialization failed",
                        error,
                    )
                }
            })?;
            Ok(())
        })();
        runtime.finish_debug_operation();
        runtime.finish_console();
        initialized?;
        Ok(Self {
            engine: runtime,
            visualizer,
        })
    }
    pub(super) fn begin_console(&mut self, invocation: u64) {
        self.engine.begin_console(invocation);
        self.engine.begin_debug_operation(invocation);
    }
    pub(super) fn finish_console(&mut self) {
        self.engine.finish_debug_operation();
        self.engine.finish_console();
    }
    pub(super) fn invoke(&mut self, invocation: &Invocation) -> Result<VisualizationStep, Failure> {
        if matches!(invocation, Invocation::Traffic { .. })
            && self.visualizer.live_updates.is_none()
        {
            return Err(fail(
                FailureKind::Protocol,
                "Live updates were not declared",
            ));
        }
        let input = serde_json::to_string(invocation)
            .map_err(|_| fail(FailureKind::Protocol, "Invocation serialization failed"))?;
        let literal = serde_json::to_string(&input)
            .map_err(|_| fail(FailureKind::Protocol, "Invocation encoding failed"))?;
        self.engine.poll_tasks().map_err(|error| {
            engine_failure(
                FailureKind::Exception,
                "The visualizer operation failed",
                error,
            )
        })?;
        let output = self
            .engine
            .execute_string(
                "transmog:extension-invoke",
                &format!("globalThis.__tmInvoke({literal})"),
                self.visualizer.limits.output_bytes,
            )
            .map_err(|error| {
                let (kind, message) = match error.kind {
                    ErrorKind::OutputLimit => {
                        (FailureKind::Limit, "Visualizer output exceeds its limit")
                    }
                    ErrorKind::NotString => (
                        FailureKind::Validation,
                        "Visualizer output must be serializable",
                    ),
                    _ => (FailureKind::Exception, "The visualizer operation failed"),
                };
                engine_failure(kind, message, error)
            })?;
        serde_json::from_str(&output).map_err(|_| {
            fail(
                FailureKind::Validation,
                "Visualizer output violates the data contract",
            )
        })
    }
}

fn engine_failure(kind: FailureKind, message: &str, error: transmog_js_runtime::Error) -> Failure {
    let mut failure = fail(kind, message);
    failure.exception = error.exception;
    failure
}

type Modules = BTreeMap<ModuleSpecifier, String>;
fn load_modules(
    snapshot: &Snapshot,
    visualizer: &Visualizer,
) -> Result<(ModuleSpecifier, Modules), Failure> {
    transmog_script_compiler::with_compiler_stack(|| load_modules_inner(snapshot, visualizer))
}

#[allow(clippy::too_many_lines)] // Keep the static module policy next to every dependency/export check.
fn load_modules_inner(
    snapshot: &Snapshot,
    visualizer: &Visualizer,
) -> Result<(ModuleSpecifier, Modules), Failure> {
    let root = ModuleSpecifier::parse("file:///transmog-extension/").expect("Constant module root");
    let entry = root
        .join(&visualizer.entry)
        .map_err(|_| fail(FailureKind::Validation, "Invalid entry module"))?;
    let mut pending = BTreeSet::from([entry.clone()]);
    let mut modules = Modules::new();
    while let Some(specifier) = pending.pop_first() {
        if modules.contains_key(&specifier) {
            continue;
        }
        let path = specifier
            .path()
            .strip_prefix(root.path())
            .ok_or_else(|| fail(FailureKind::Validation, "Module escaped package root"))?;
        if !std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext == "js" || ext == "mjs")
        {
            return Err(fail(
                FailureKind::Validation,
                "Executable modules must be JavaScript .js or .mjs files",
            ));
        }
        let source = snapshot
            .files
            .get(path)
            .and_then(|b| std::str::from_utf8(b).ok())
            .ok_or_else(|| {
                fail(
                    FailureKind::Validation,
                    "Package module is missing or not UTF-8",
                )
            })?;
        let parsed = parse_module_detailed(specifier.clone(), source, ModuleSyntax::JavaScript)
            .map_err(|diagnostic| {
                let mut failure = fail(FailureKind::Validation, "JavaScript parsing failed");
                let message = diagnostic.message.chars().take(2048).collect::<String>();
                let source_excess = diagnostic
                    .location
                    .as_ref()
                    .is_some_and(|location| location.source.len() > 2048);
                let unavailable = diagnostic.location.is_none() || source_excess;
                let name = diagnostic
                    .location
                    .as_ref()
                    .map(|_| "SyntaxError".to_owned());
                let truncated = message.len() < diagnostic.message.len() || source_excess;
                let frames = diagnostic
                    .location
                    .filter(|location| location.source.len() <= 2048)
                    .map(|generated| transmog_extensions::StackFrame {
                        function: String::new(),
                        generated: Some(generated),
                        original: None,
                        mapping_issue: None,
                    })
                    .into_iter()
                    .collect();
                failure.exception = Some(Box::new(transmog_extensions::ExceptionDiagnostic {
                    artifact: None,
                    exception: transmog_extensions::ExceptionValue {
                        name,
                        message,
                        frames,
                        unavailable,
                    },
                    causes: vec![],
                    truncated,
                }));
                failure
            })?;
        for item in &parsed.module().body {
            let dependency = match item {
                ModuleItem::ModuleDecl(ModuleDecl::Import(i)) if i.type_only => None,
                ModuleItem::ModuleDecl(ModuleDecl::Import(i)) => {
                    Some(i.src.value.to_string_lossy().into_owned())
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportAll(e)) if !e.type_only => {
                    Some(e.src.value.to_string_lossy().into_owned())
                }
                ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(e)) if !e.type_only => e
                    .src
                    .as_ref()
                    .map(|s| s.value.to_string_lossy().into_owned()),
                _ => None,
            };
            if let Some(dependency) = dependency {
                if !(dependency.starts_with("./") || dependency.starts_with("../"))
                    || dependency.contains(['\\', '?', '#', '%'])
                {
                    return Err(fail(
                        FailureKind::Validation,
                        "Only package-relative static imports are supported",
                    ));
                }
                let resolved = specifier
                    .join(&dependency)
                    .map_err(|_| fail(FailureKind::Validation, "Invalid package import"))?;
                if resolved.scheme() != root.scheme()
                    || resolved.host_str() != root.host_str()
                    || !resolved.path().starts_with(root.path())
                {
                    return Err(fail(
                        FailureKind::Validation,
                        "Import escaped the extension package",
                    ));
                }
                pending.insert(resolved);
            }
        }
        let mut forbidden = Forbidden::default();
        parsed.module().visit_with(&mut forbidden);
        if forbidden.0 {
            return Err(fail(
                FailureKind::Validation,
                "Dynamic imports and string code generation are forbidden",
            ));
        }
        let mut javascript = source.to_string();
        if specifier == entry {
            javascript.push_str(&entry_handlers(
                parsed.module(),
                visualizer.live_updates.is_some(),
            )?);
        }
        modules.insert(specifier, javascript);
    }
    Ok((entry, modules))
}

fn entry_handlers(module: &Module, live_updates: bool) -> Result<String, Failure> {
    let mut functions = BTreeMap::new();
    let mut exports = BTreeMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
                functions.insert(function.ident.sym.to_string(), &function.function);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Fn(function) = &export.decl {
                    let name = function.ident.sym.to_string();
                    functions.insert(name.clone(), &function.function);
                    exports.insert(name.clone(), name);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export))
                if export.src.is_none() && !export.type_only =>
            {
                for specifier in &export.specifiers {
                    if let ExportSpecifier::Named(specifier) = specifier
                        && !specifier.is_type_only
                        && let ModuleExportName::Ident(local) = &specifier.orig
                    {
                        let name = specifier.exported.as_ref().unwrap_or(&specifier.orig);
                        exports.insert(name.atom().to_string(), local.sym.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let mut handlers = Vec::new();
    for name in [
        "visualize",
        "resume",
        "resumeContext",
        "resumeDecoded",
        "expand",
        "onTraffic",
    ] {
        if let Some(local) = exports.get(name) {
            if !functions
                .get(local)
                .is_some_and(|function| !function.is_async && !function.is_generator)
            {
                return Err(fail(
                    FailureKind::Validation,
                    "Export handlers as synchronous local function declarations",
                ));
            }
            // Resolve the public export to its local binding, including bundler aliases.
            handlers.push(format!("{name}:{local}"));
        } else if name == "visualize" || (name == "onTraffic" && live_updates) {
            return Err(fail(
                FailureKind::Validation,
                "Declare synchronous visualize and optional live handlers",
            ));
        }
    }
    Ok(format!(
        "\nObject.defineProperty(globalThis,'__tmHandlers',{{value:Object.freeze({{{}}})}});",
        handlers.join(",")
    ))
}

#[derive(Default)]
struct Forbidden(bool);
impl Visit for Forbidden {
    fn visit_call_expr(&mut self, e: &CallExpr) {
        if matches!(&e.callee, Callee::Import(_))
            || matches!(&e.callee,Callee::Expr(c) if matches!(c.as_ref(),Expr::Ident(i) if i.sym=="eval"||i.sym=="Function"))
        {
            self.0 = true;
        }
        e.visit_children_with(self);
    }
    fn visit_new_expr(&mut self, e: &NewExpr) {
        if matches!(e.callee.as_ref(),Expr::Ident(i) if i.sym=="Function") {
            self.0 = true;
        }
        e.visit_children_with(self);
    }
}
fn fail(kind: FailureKind, message: &str) -> Failure {
    Failure::new(kind, message)
}
