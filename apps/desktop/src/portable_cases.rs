//! Window-owned portable transfer review. Opening never enables automation.
use super::DesktopState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{State, WebviewWindow};
use transmog_app::{
    AppError, CaseExportRequest, CaseNoteInput, ErrorCategory, ExperimentSelection,
    PreparedRuleImport, RuleExportRequest,
};
use transmog_capture::CapturePassword;
use transmog_portable::{Package, PackageFormat, PackageManifest, PreparedCase};

#[derive(Default)]
struct WindowCase {
    operation: Option<Arc<AtomicBool>>,
    chosen: Option<PathBuf>,
    export: Option<Arc<PreparedCase>>,
    package: Option<Arc<Package>>,
    import: Option<PreparedRuleImport>,
}
fn cases() -> &'static Mutex<HashMap<String, WindowCase>> {
    static CASES: OnceLock<Mutex<HashMap<String, WindowCase>>> = OnceLock::new();
    CASES.get_or_init(Mutex::default)
}
fn lock() -> std::sync::MutexGuard<'static, HashMap<String, WindowCase>> {
    cases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::InvalidInput,
        message: message.into(),
        retryable: false,
    }
}
fn portable(error: transmog_portable::Error) -> AppError {
    let category = match &error {
        transmog_portable::Error::PasswordRequired => ErrorCategory::PasswordRequired,
        transmog_portable::Error::InvalidPassword => ErrorCategory::InvalidPassword,
        transmog_portable::Error::Canceled => ErrorCategory::Canceled,
        transmog_portable::Error::Unsupported(_) => ErrorCategory::Unsupported,
        transmog_portable::Error::Io(_) => ErrorCategory::Unavailable,
        transmog_portable::Error::Invalid(_) => ErrorCategory::InvalidInput,
    };
    AppError {
        category,
        message: error.to_string(),
        retryable: false,
    }
}
struct Operation {
    window: String,
    flag: Arc<AtomicBool>,
}
impl Operation {
    fn begin(window: &str) -> Result<Self, AppError> {
        let mut all = lock();
        if all.len() >= 16 && !all.contains_key(window) {
            return Err(error(
                "Close a case in another window before opening another",
            ));
        }
        let owner = all.entry(window.into()).or_default();
        if owner.operation.is_some() {
            return Err(error("A case operation is still running in this window"));
        }
        let flag = Arc::new(AtomicBool::new(false));
        owner.operation = Some(flag.clone());
        Ok(Self {
            window: window.into(),
            flag,
        })
    }
    fn update<T>(&self, action: impl FnOnce(&mut WindowCase) -> T) -> Result<T, AppError> {
        if self.flag.load(Ordering::Acquire) {
            return Err(AppError {
                category: ErrorCategory::Canceled,
                message: "Case operation canceled".into(),
                retryable: false,
            });
        }
        let mut all = lock();
        let owner = all
            .get_mut(&self.window)
            .filter(|owner| {
                owner
                    .operation
                    .as_ref()
                    .is_some_and(|flag| Arc::ptr_eq(flag, &self.flag))
            })
            .ok_or_else(|| error("The case window has closed"))?;
        Ok(action(owner))
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if let Some(owner) = lock().get_mut(&self.window)
            && owner
                .operation
                .as_ref()
                .is_some_and(|f| Arc::ptr_eq(f, &self.flag))
        {
            owner.operation = None;
        }
    }
}
pub(super) fn release(window: &str) {
    let owner = { lock().remove(window) };
    if let Some(owner) = owner {
        if let Some(flag) = &owner.operation {
            flag.store(true, Ordering::Release);
        }
        tauri::async_runtime::spawn_blocking(move || drop(owner));
    }
}
// This fixture adapter is compiled out of product builds. It selects only an
// owned file inside this isolated test instance's export root.
#[cfg(feature = "desktop-test-fixtures")]
fn fixture_path(state: &DesktopState) -> Option<PathBuf> {
    std::env::var_os("TRANSMOG_DESKTOP_CASE_TEST")
        .map(|_| state.export_root.join("qualification.tmcase.tmcap"))
}
#[cfg(not(feature = "desktop-test-fixtures"))]
fn fixture_path(_: &DesktopState) -> Option<PathBuf> {
    None
}
async fn pick_case(
    window: &WebviewWindow,
    state: &DesktopState,
    suffix: Option<&str>,
) -> Option<PathBuf> {
    if let Some(path) = fixture_path(state) {
        return Some(path);
    }
    let picker = rfd::AsyncFileDialog::new().set_parent(window);
    let file = if let Some(suffix) = suffix {
        picker
            .set_title("Save portable case")
            .set_file_name(format!("Investigation.{suffix}"))
            .add_filter("Portable case", &[suffix])
            .save_file()
            .await
    } else {
        picker
            .set_title("Open portable case or rules")
            .add_filter("Portable investigations", &["tmcap", "tmcase", "tmrules"])
            .pick_file()
            .await
    };
    file.map(|file| file.path().to_owned())
}
fn package(window: &str) -> Result<Arc<Package>, AppError> {
    lock()
        .get(window)
        .and_then(|o| o.package.clone())
        .ok_or_else(|| error("Open a case first"))
}
fn main_window(window: &WebviewWindow) -> Result<(), AppError> {
    if window.label() != "main" {
        return Err(error("Importing automation belongs to the main window"));
    }
    Ok(())
}
fn review(manifest: PackageManifest) -> Result<Value, AppError> {
    let (value, inventory) = match manifest {
        PackageManifest::Case(m) => {
            let inventory = m.inventory().map_err(portable)?;
            (serde_json::to_value(&m), serde_json::to_value(inventory))
        }
        PackageManifest::Rules(m) => (
            serde_json::to_value(&m),
            Ok(
                json!({"sources":0,"contexts":0,"notes":0,"runs":0,"missingDependencies":0,"incompleteSources":0,"experiments":m.experiments.iter().map(|e|json!({"id":e.id,"state":"ready-for-explicit-import","reason":null})).collect::<Vec<_>>()}),
            ),
        ),
    };
    Ok(
        json!({"manifest":value.map_err(|_| error("Case inventory failed"))?,"inventory":inventory.map_err(|_|error("Case inventory failed"))?,
        "excluded":["Only declared objects are present. Earlier export omissions are unknown unless recorded in provenance."],"activation":"Opening and importing do not activate rules or scripts."}),
    )
}

#[tauri::command]
pub(super) fn case_export_choices(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Value, AppError> {
    let app = state.window_application(&window)?;
    if window.label() != "main" {
        return Ok(json!({"rules":[],"variants":[]}));
    }
    let status = app.automation_status();
    let owned = status
        .experiments
        .iter()
        .flat_map(|e| &e.variants)
        .flat_map(|v| &v.revisions)
        .flat_map(|r| &r.rules)
        .map(|r| r.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut variants = vec![];
    for experiment in status.experiments {
        for variant in experiment.variants {
            for revision in variant.revisions {
                if !revision.rules.is_empty() {
                    variants.push(json!({"label":format!("{} / {} / revision {}",experiment.name,variant.name,revision.revision),"selection":ExperimentSelection{experiment_id:experiment.id.clone(),variant_id:variant.id.clone(),revision:revision.revision}}));
                }
            }
        }
    }
    Ok(
        json!({"rules":status.rules.iter().filter(|r|!owned.contains(&r.id)).map(|r| json!({"id":r.id,"label":r.display_name.as_deref().unwrap_or(&r.id)})).collect::<Vec<_>>(),"variants":variants}),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExportInput {
    title: String,
    scope: Option<transmog_app::comparison::ComparisonScope>,
    rule_ids: Vec<String>,
    selections: Vec<ExperimentSelection>,
    notes: String,
    include_context: bool,
}
#[tauri::command]
pub(super) async fn prepare_case_export(
    input: ExportInput,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Value, AppError> {
    let app = state.window_application(&window)?;
    if window.label() != "main" && (!input.rule_ids.is_empty() || !input.selections.is_empty()) {
        return Err(error("Saved automation belongs to the main window"));
    }
    let operation = Operation::begin(window.label())?;
    let flag = operation.flag.clone();
    let staging = state.export_root.clone();
    let mut identity = [0; 16];
    getrandom::fill(&mut identity).map_err(|_| error("Cannot create a portable case identity"))?;
    let id = format!("case-{:032x}", u128::from_le_bytes(identity));
    let include_context = input.include_context;
    let (prepared, traffic) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging).map_err(|e| portable(e.into()))?;
        let evidence = input
            .scope
            .map(|scope| app.review_traffic_export(Some(scope)))
            .transpose()?;
        let request = CaseExportRequest {
            rules: RuleExportRequest {
                id,
                revision: "1".into(),
                title: input.title,
                rule_ids: input.rule_ids,
                selections: input.selections,
            },
            evidence_review_id: evidence.as_ref().map(|r| r.id.clone()),
            derived_plaintext: true,
            notes: if input.notes.is_empty() {
                vec![]
            } else {
                vec![CaseNoteInput {
                    id: "notes".into(),
                    text: input.notes,
                    exchange_id: None,
                }]
            },
        };
        let prepared = app.prepare_investigation(&request, &staging, &flag)?;
        if input.include_context {
            return Ok((prepared, evidence));
        }
        let mut manifest = prepared.manifest().clone();
        manifest.contexts.clear();
        manifest.objects.retain(|o| o.role != "context");
        PreparedCase::prepare(
            &manifest.encode().map_err(portable)?,
            &staging,
            |sha| Ok(Box::new(prepared.open_object(sha)?)),
            &flag,
        )
        .map(|prepared| (prepared, evidence))
        .map_err(portable)
    })
    .await
    .map_err(|_| error("Case preparation failed"))??;
    let mut result = review(PackageManifest::Case(prepared.manifest().clone()))?;
    result["traffic"] =
        serde_json::to_value(traffic).map_err(|_| error("Traffic inventory failed"))?;
    result["excluded"] = json!([
        "Working manifest history and recovery files",
        "Unselected traffic, rules and experiment revisions",
        "Script sources, compiled state and application credential/private-key stores",
        "Historical run results (not reconstructed from current files)",
        if include_context {
            "New configuration commands are not run"
        } else {
            "Original source context and configuration output"
        }
    ]);
    drop(operation.update(|owner| owner.export.replace(Arc::new(prepared)))?);
    Ok(result)
}
#[tauri::command]
pub(super) async fn save_case_export(
    format: PackageFormat,
    password: Option<CapturePassword>,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, AppError> {
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let prepared = operation
        .update(|owner| owner.export.clone())?
        .ok_or_else(|| error("Review the selected case before saving"))?;
    let suffix = if matches!(format, PackageFormat::Native) {
        "tmcase.tmcap"
    } else {
        "tmcase"
    };
    let Some(path) = pick_case(&window, &state, Some(suffix)).await else {
        return Ok(None);
    };
    let label = path.to_string_lossy().into_owned();
    let flag = operation.flag.clone();
    tokio::task::spawn_blocking(move || {
        transmog_portable::write_case_package(&prepared, &path, format, password, true, &flag)
            .map_err(portable)
    })
    .await
    .map_err(|_| error("Case save failed"))??;
    Ok(Some(label))
}
#[tauri::command]
pub(super) async fn choose_case(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, AppError> {
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let Some(path) = pick_case(&window, &state, None).await else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    operation.update(|o| o.chosen = Some(path))?;
    Ok(Some(name))
}

#[tauri::command]
pub(super) async fn save_open_case(
    format: PackageFormat,
    password: Option<CapturePassword>,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, AppError> {
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let package = package(window.label())?;
    let suffix = match (package.manifest(), format) {
        (PackageManifest::Case(_), PackageFormat::Native) => "tmcase.tmcap",
        (PackageManifest::Case(_), PackageFormat::BareZip) => "tmcase",
        (PackageManifest::Rules(_), PackageFormat::Native) => "tmrules.tmcap",
        (PackageManifest::Rules(_), PackageFormat::BareZip) => "tmrules",
    };
    let Some(path) = pick_case(&window, &state, Some(suffix)).await else {
        return Ok(None);
    };
    let label = path.to_string_lossy().into_owned();
    let flag = operation.flag.clone();
    tokio::task::spawn_blocking(move || {
        package
            .write_transfer(&path, format, password.as_ref(), true, &flag)
            .map_err(portable)
    })
    .await
    .map_err(|_| error("Transfer save failed"))??;
    Ok(Some(label))
}
#[tauri::command]
pub(super) async fn open_case(
    password: Option<CapturePassword>,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Value, AppError> {
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let path = operation
        .update(|o| o.chosen.clone())?
        .ok_or_else(|| error("Choose a package first"))?;
    let flag = operation.flag.clone();
    let package = tokio::task::spawn_blocking(move || {
        Package::open(&path, password.as_ref(), &flag).map_err(portable)
    })
    .await
    .map_err(|_| error("Case opening failed"))??;
    let mut result = review(package.manifest().clone())?;
    result["encrypted"] = json!(package.encrypted());
    drop(operation.update(|o| (o.package.replace(Arc::new(package)), o.import.take()))?);
    Ok(result)
}
#[tauri::command]
pub(super) async fn inspect_case_source(
    source_id: String,
    password: Option<CapturePassword>,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::OpenedCaseEvidence, AppError> {
    let app = state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let package = package(window.label())?;
    app.open_package_evidence(&package, &source_id, password, operation.flag.clone())
        .await
}

#[tauri::command]
pub(super) async fn read_case_context(
    id: String,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<String, AppError> {
    use std::io::Read as _;
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let package = package(window.label())?;
    let flag = operation.flag.clone();
    tokio::task::spawn_blocking(move || {
        let PackageManifest::Case(manifest) = package.manifest() else {
            return Err(error("This package has no context objects"));
        };
        let context = manifest
            .contexts
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| error("Context is not included"))?;
        if context.content.state != "present" {
            return Err(error("Context is unavailable"));
        }
        let sha = context
            .content
            .sha256
            .as_deref()
            .ok_or_else(|| error("Context has no object"))?;
        let object = manifest
            .objects
            .iter()
            .find(|o| o.sha256 == sha)
            .ok_or_else(|| error("Context object is missing"))?;
        if object.media_type != "application/json" && !object.media_type.starts_with("text/") {
            return Err(error("This context type has no text inspector"));
        }
        let mut reader = package.open_object(sha).map_err(portable)?;
        let mut bytes = vec![];
        let mut buffer = vec![0; 128 * 1024];
        loop {
            if flag.load(Ordering::Acquire) {
                return Err(portable(transmog_portable::Error::Canceled));
            }
            let count = reader
                .read(&mut buffer)
                .map_err(|_| error("Context read failed"))?;
            if count == 0 {
                break;
            }
            if bytes.len() + count > 8 * 1024 * 1024 {
                return Err(error("Context exceeds eight MiB"));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).map_err(|_| error("Context text is not UTF-8"))
    })
    .await
    .map_err(|_| error("Context inspection failed"))?
}
#[tauri::command]
pub(super) async fn save_case_working_copy(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, AppError> {
    state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let package = package(window.label())?;
    let destination = if fixture_path(&state).is_some() {
        Some(state.export_root.join("Investigation-working-copy"))
    } else {
        rfd::AsyncFileDialog::new()
            .set_parent(&window)
            .set_title("Create a new plaintext working folder")
            .set_file_name("Investigation-working-copy")
            .save_file()
            .await
            .map(|file| file.path().to_owned())
    };
    let Some(path) = destination else {
        return Ok(None);
    };
    let PackageManifest::Case(_) = package.manifest() else {
        return Err(error("A rule collection has no working case"));
    };
    let label = path.to_string_lossy().into_owned();
    let staging = state.export_root.clone();
    let flag = operation.flag.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging).map_err(|e| portable(e.into()))?;
        package
            .save_working_copy(&path, &staging, &flag)
            .map(drop)
            .map_err(portable)
    })
    .await
    .map_err(|_| error("Working copy failed"))??;
    Ok(Some(label))
}
#[tauri::command]
pub(super) async fn review_case_rules(
    experiment_ids: Vec<String>,
    ordinary: bool,
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::RuleImportReview, AppError> {
    main_window(&window)?;
    let app = state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let package = package(window.label())?;
    let flag = operation.flag.clone();
    let staging = state.export_root.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging).map_err(|e| portable(e.into()))?;
        let collection = match package.manifest() {
            PackageManifest::Case(m) => m
                .rules_for_import(&experiment_ids, ordinary)
                .map_err(portable)?,
            PackageManifest::Rules(m) => m.clone(),
        };
        if collection.rules.is_empty() && collection.experiments.is_empty() {
            return Err(error(
                "Select ordinary rules or a supported experiment to import",
            ));
        }
        app.prepare_rule_import(
            &collection.encode().map_err(portable)?,
            &staging,
            |sha| Ok(Box::new(package.open_object(sha)?)),
            &flag,
        )
    })
    .await
    .map_err(|_| error("Rule import review failed"))??;
    let result = prepared.review().clone();
    drop(operation.update(|o| o.import.replace(prepared))?);
    Ok(result)
}
#[tauri::command]
pub(super) async fn import_case_rules(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<transmog_app::AutomationStatus, AppError> {
    main_window(&window)?;
    let app = state.window_application(&window)?;
    let operation = Operation::begin(window.label())?;
    let prepared = operation
        .update(|o| o.import.take())?
        .ok_or_else(|| error("Review the selected rule import first"))?;
    let flag = operation.flag.clone();
    tokio::task::spawn_blocking(move || app.import_rule_collection(prepared, &flag))
        .await
        .map_err(|_| error("Rule import failed"))?
}
#[tauri::command]
pub(super) fn cancel_case_operation(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.window_application(&window)?;
    if let Some(flag) = lock().get(window.label()).and_then(|o| o.operation.clone()) {
        flag.store(true, Ordering::Release);
    }
    Ok(())
}
#[tauri::command]
pub(super) fn close_case(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<(), AppError> {
    state.window_application(&window)?;
    release(window.label());
    Ok(())
}
