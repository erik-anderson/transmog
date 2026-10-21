use deno_ast::{
    MediaType, ParseParams, parse_module,
    swc::{
        ast::{
            CallExpr, Callee, Decl, ExportSpecifier, Expr, Module, ModuleDecl, ModuleExportName,
            ModuleItem, NewExpr, Stmt,
        },
        ecma_visit::{Visit, VisitWith},
    },
};
use deno_core::{JsRuntime, ModuleSpecifier, RuntimeOptions, StaticModuleLoader, v8};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Arc,
};
use transmog_extensions::{
    Failure, FailureKind, Invocation, Snapshot, VisualizationStep, Visualizer,
};

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
    engine: JsRuntime,
    // V8 retains this executor's handle for delayed foreground work, including GC.
    // Drop the isolate first, then its executor. This does not expose JS timers.
    executor: tokio::runtime::Runtime,
    visualizer: Visualizer,
}
impl Runtime {
    pub(super) fn new(snapshot: &Snapshot, visualizer_id: &str) -> Result<Self, Failure> {
        let visualizer = snapshot
            .manifest
            .visualizers
            .iter()
            .find(|v| v.id == visualizer_id)
            .ok_or_else(|| fail(FailureKind::Validation, "Unknown visualizer contribution"))?
            .clone();
        let (entry, modules) = load_modules(snapshot, &visualizer)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|_| {
                fail(
                    FailureKind::Unavailable,
                    "The worker executor is unavailable",
                )
            })?;
        // deno_core captures Handle::try_current when registering the isolate.
        // Without this context, sufficiently large modules abort on delayed GC.
        let context = executor.enter();
        let mut runtime = JsRuntime::try_new(RuntimeOptions {
            create_params: Some(
                v8::CreateParams::default()
                    .heap_limits(8 * 1024 * 1024, visualizer.limits.heap_bytes),
            ),
            module_loader: Some(Rc::new(StaticModuleLoader::new(modules))),
            ..RuntimeOptions::default()
        })
        .map_err(|_| {
            fail(
                FailureKind::Unavailable,
                "The visualizer runtime could not initialize",
            )
        })?;
        // Install capability restrictions before executing any extension module initializer.
        runtime
            .execute_script("transmog:extension-bootstrap", BOOTSTRAP)
            .map_err(|_| {
                fail(
                    FailureKind::Unavailable,
                    "Runtime restrictions could not initialize",
                )
            })?;
        executor.block_on(async {
            let id = runtime
                .load_main_es_module(&entry)
                .await
                .map_err(|_| fail(FailureKind::Validation, "Package module loading failed"))?;
            let pending = runtime.mod_evaluate(id);
            runtime
                .run_event_loop(deno_core::PollEventLoopOptions::default())
                .await
                .map_err(|_| fail(FailureKind::Exception, "Extension initialization failed"))?;
            pending
                .await
                .map_err(|_| fail(FailureKind::Exception, "Extension initialization failed"))
        })?;
        drop(context);
        Ok(Self {
            engine: runtime,
            executor,
            visualizer,
        })
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
        // Drive ready foreground work once without waiting for future GC delays.
        // A hidden instance still has no executor or worker to poll.
        self.executor.block_on(async {
            tokio::task::yield_now().await;
            std::future::poll_fn(|context| {
                std::task::Poll::Ready(
                    match self
                        .engine
                        .poll_event_loop(context, deno_core::PollEventLoopOptions::default())
                    {
                        std::task::Poll::Ready(Err(_)) => Err(fail(
                            FailureKind::Exception,
                            "The visualizer operation failed",
                        )),
                        _ => Ok(()),
                    },
                )
            })
            .await
        })?;
        let _context = self.executor.enter();
        let result = self
            .engine
            .execute_script(
                "transmog:extension-invoke",
                format!("globalThis.__tmInvoke({literal})"),
            )
            .map_err(|_| fail(FailureKind::Exception, "The visualizer operation failed"))?;
        deno_core::scope!(scope, &mut self.engine);
        let result = v8::Local::new(scope, result);
        let output = result.to_string(scope).ok_or_else(|| {
            fail(
                FailureKind::Validation,
                "Visualizer output must be serializable",
            )
        })?;
        if output.length() > self.visualizer.limits.output_bytes {
            return Err(fail(
                FailureKind::Limit,
                "Visualizer output exceeds its limit",
            ));
        }
        let output = output.to_rust_string_lossy(scope);
        if output.len() > self.visualizer.limits.output_bytes {
            return Err(fail(
                FailureKind::Limit,
                "Visualizer output exceeds its limit",
            ));
        }
        serde_json::from_str(&output).map_err(|_| {
            fail(
                FailureKind::Validation,
                "Visualizer output violates the data contract",
            )
        })
    }
}

type Modules = BTreeMap<ModuleSpecifier, String>;
#[allow(clippy::too_many_lines)] // Keep the static module policy next to every dependency/export check.
fn load_modules(
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
        let parsed = parse_module(ParseParams {
            specifier: specifier.clone(),
            text: Arc::<str>::from(source),
            media_type: MediaType::JavaScript,
            capture_tokens: false,
            scope_analysis: false,
            maybe_syntax: None,
        })
        .map_err(|_| fail(FailureKind::Validation, "JavaScript parsing failed"))?;
        for item in &parsed.program_ref().unwrap_module().body {
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
        parsed.program_ref().visit_with(&mut forbidden);
        if forbidden.0 {
            return Err(fail(
                FailureKind::Validation,
                "Dynamic imports and string code generation are forbidden",
            ));
        }
        let mut javascript = source.to_string();
        if specifier == entry {
            javascript.push_str(&entry_handlers(
                parsed.program_ref().unwrap_module(),
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
