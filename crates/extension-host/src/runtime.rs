use deno_ast::{
    MediaType, ParseParams, parse_module,
    swc::{
        ast::{CallExpr, Callee, Decl, Expr, ModuleDecl, ModuleItem, NewExpr},
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
    else if (call.kind === 'expand') result = handlers.expand(call.token);
    else if (call.kind === 'traffic') result = handlers.onTraffic(call.events);
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
    runtime: JsRuntime,
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
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|_| {
                fail(
                    FailureKind::Unavailable,
                    "The worker executor is unavailable",
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
        Ok(Self {
            runtime,
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
        let result = self
            .runtime
            .execute_script(
                "transmog:extension-invoke",
                format!("globalThis.__tmInvoke({literal})"),
            )
            .map_err(|_| fail(FailureKind::Exception, "The visualizer operation failed"))?;
        deno_core::scope!(scope, &mut self.runtime);
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
        let mut exports = BTreeMap::new();
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
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                    if let Decl::Fn(f) = &e.decl {
                        exports.insert(
                            f.ident.sym.to_string(),
                            f.function.is_async || f.function.is_generator,
                        );
                    }
                    None
                }
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
            if exports.get("visualize") != Some(&false)
                || (visualizer.live_updates.is_some() && exports.get("onTraffic") != Some(&false))
                || ["resume", "expand"]
                    .iter()
                    .any(|name| exports.get(*name) == Some(&true))
            {
                return Err(fail(
                    FailureKind::Validation,
                    "Declare synchronous visualize and optional live handlers",
                ));
            }
            javascript.push_str("\nObject.defineProperty(globalThis,'__tmHandlers',{value:Object.freeze({visualize, resume:typeof resume==='function'?resume:undefined, expand:typeof expand==='function'?expand:undefined, onTraffic:typeof onTraffic==='function'?onTraffic:undefined})});");
        }
        modules.insert(specifier, javascript);
    }
    Ok((entry, modules))
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
