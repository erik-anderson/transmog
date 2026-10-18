//! Isolated captured-page windows. The private proxy only denies traffic and
//! never contacts an origin, alters system routing or installs a certificate.
use super::{ARTIFACT_SEQUENCE, DesktopState};
static OPERATIONS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, PreparingPreview>>,
> = std::sync::OnceLock::new();
fn operations() -> &'static std::sync::Mutex<std::collections::HashMap<String, PreparingPreview>> {
    OPERATIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
struct PreparingPreview {
    canceled: Arc<AtomicBool>,
    label: String,
}
struct Operation(String);
impl Drop for Operation {
    fn drop(&mut self) {
        operations()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.0);
    }
}
#[tauri::command]
pub(super) fn cancel_captured_page(operation_id: String, window: tauri::WebviewWindow) {
    if let Some(canceled) = operations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&format!("{}:{operation_id}", window.label()))
    {
        canceled.canceled.store(true, Ordering::Release);
    }
}
use std::{
    io::Read,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::{Manager, WebviewWindowBuilder, utils::config::WebviewUrl};
use transmog_app::{
    AppError, CapturedPage, CapturedPageDiagnostics, CapturedPageOptions, CapturedPageReport,
    ErrorCategory,
};
static REPORTS: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<RetainedPreview>>> =
    std::sync::OnceLock::new();
struct RetainedPreview {
    owner: String,
    label: String,
    diagnostics: CapturedPageDiagnostics,
    page: Option<Arc<CapturedPage>>,
}
fn reports() -> &'static std::sync::Mutex<std::collections::VecDeque<RetainedPreview>> {
    REPORTS.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}
pub(super) fn comparison_evidence(
    owner: &str,
    subject: &transmog_app::comparison::ComparisonSubject,
) -> Result<
    (
        Option<Arc<CapturedPage>>,
        Option<transmog_app::comparison::PreviewComparisonEvidence>,
    ),
    AppError,
> {
    let transmog_app::comparison::ComparisonSubject::Preview { run, observation } = subject else {
        return Ok((None, None));
    };
    let reports = reports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let report = reports
        .iter()
        .find(|r| r.owner == owner && &r.label == run)
        .ok_or_else(|| error("Preview report expired or belongs to another window"))?;
    let Some(page) = report.page.clone() else {
        return Ok((None, None));
    };
    let snapshot = report.diagnostics.snapshot();
    let request = snapshot.requests.iter().find(|r| r.id == *observation);
    let evidence = report
        .diagnostics
        .supplied_response(*observation)
        .map(
            |resource| transmog_app::comparison::PreviewComparisonEvidence {
                resource,
                source_id: request.and_then(|r|r.entry_id.clone()),
                notes:{let mut notes=vec![format!("Frozen preview {} · observation {} · browser-safe headers include preparation framing/content-coding repairs",run,observation)];
                    if let Some(rule)=request.and_then(|r|r.rule_id.as_ref()).and_then(|id|snapshot.rules.iter().find(|rule|&rule.id==id)){notes.push(format!("Winning saved rule: {} · {}@{} · response asset {}",rule.name,rule.id,rule.revision,rule.asset));}
                    notes
                },
                label: {
                    format!(
                        "{} · {} · {} · scripts {} · response {}",
                        if snapshot.rules.is_empty() {
                            "Baseline"
                        } else {
                            "Modified"
                        },
                        snapshot.source,
                        snapshot.runtime,
                        match snapshot.scripts_enabled {
                            Some(true) => "on",
                            Some(false) => "off",
                            None => "unrecorded",
                        },
                        observation
                    )
                },
            },
        );
    Ok((Some(page), evidence))
}
#[tauri::command]
pub(super) fn captured_page_report(
    label: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<CapturedPageReport, AppError> {
    let application = state.window_application(&window)?;
    let report = reports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|run| run.owner == window.label() && run.label == label)
        .map(|run| (run.diagnostics.clone(), run.page.is_some()))
        .ok_or_else(|| error("This preview report is unavailable or belongs to another window"))?;
    let retained = application
        .session_service()
        .catalog()
        .project_retained(|snapshot| format!("{:032x}", snapshot.exchange_id.0))
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let mut report = {
        let mut snapshot = report.0.snapshot();
        snapshot.bytes_available = report.1;
        snapshot
    };
    for resource in &mut report.resources {
        resource.source_available = retained.contains(&resource.entry_id);
    }
    Ok(report)
}

#[tauri::command]
pub(super) async fn captured_page_choices(
    source_id: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<transmog_app::CapturedPageChoices, AppError> {
    let application = state.window_application(&window)?;
    tokio::task::spawn_blocking(move || application.captured_page_choices(&source_id))
        .await
        .map_err(|_| error("Preview document choices are unavailable"))?
}

#[tauri::command]
pub(super) async fn experiment_preview_pages(
    id: String,
    selected_id: Option<String>,
    include_pages: Option<bool>,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<transmog_app::ExperimentPreviewPages, AppError> {
    let application = state.window_application(&window)?;
    let owner = window.label().to_owned();
    tokio::task::spawn_blocking(move || {
        let mut choices = application.experiment_preview_pages(
            &id,
            selected_id.as_deref(),
            include_pages.unwrap_or(true),
        )?;
        if !choices.available
            && let Some(page) = &choices.page
            && application.experiment_page_capture_current(page)
        {
            // Exact identity only, and only existing retained scenes in this window.
            choices.source_label = reports()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .rev()
                .find(|run| {
                    run.owner == owner
                        && run.page.as_ref().is_some_and(|scene| {
                            let report = scene.diagnostics().snapshot();
                            report.selected_entry_id == page.entry_id
                                && report.scope == transmog_app::CapturedPageScope::OriginalTrace
                        })
                })
                .map(|run| run.label.clone());
            choices.available |= choices.source_label.is_some();
        }
        Ok(choices)
    })
    .await
    .map_err(|_| error("Captured page choices could not be read"))?
}

#[tauri::command]
pub(super) async fn set_experiment_preview_page(
    id: String,
    revision: u64,
    page: transmog_app::ExperimentPreviewPage,
    state: tauri::State<'_, DesktopState>,
) -> Result<transmog_app::AutomationStatus, AppError> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || {
        application.set_experiment_preview_page(&id, revision, page)
    })
    .await
    .map_err(|_| error("The page association could not be saved"))?
}

const HARDENING: &str = r"(() => {
  const blocked = class { constructor() { throw new DOMException('Unavailable in a captured preview', 'NotSupportedError'); } };
  for (const name of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'WebTransport', 'WebSocket']) {
    Object.defineProperty(globalThis, name, { value: blocked, writable: false, configurable: false });
  }
})();";

struct PreviewOwner {
    page: Arc<CapturedPage>,
    profile: PreviewProfile,
    firewall: DenyProxy,
}
impl PreviewOwner {
    fn new(page: CapturedPage, enable_scripts: bool) -> Result<Self, AppError> {
        let profile = tempfile::Builder::new()
            .prefix("transmog-captured-webview-")
            .tempdir()
            .map_err(|_| error("An isolated preview profile could not be created"))?;
        let firewall = DenyProxy::start()
            .map_err(|_| error("Captured preview network isolation could not start"))?;
        let owner = Self {
            page: Arc::new(page),
            profile: PreviewProfile(Some(profile)),
            firewall,
        };
        owner.page.diagnostics().script_choice(enable_scripts);
        owner.page.diagnostics().runtime(format!(
            "WebView2 {}",
            tauri::webview_version().unwrap_or_else(|_| "version unavailable".into())
        ));
        Ok(owner)
    }
}
struct PreviewProfile(Option<tempfile::TempDir>);
impl PreviewProfile {
    fn path(&self) -> &std::path::Path {
        self.0.as_ref().expect("owned preview profile").path()
    }
}
impl Drop for PreviewProfile {
    fn drop(&mut self) {
        if let Some(profile) = self.0.take() {
            // WebView2 releases profile locks after its window closes. Retry
            // removal away from the UI thread, retaining only our owned TempDir.
            let _ = thread::Builder::new()
                .name("captured-preview-profile-cleanup".into())
                .spawn(move || {
                    let path = profile.path().to_owned();
                    if profile.close().is_ok() {
                        return;
                    }
                    for _ in 0..100 {
                        thread::sleep(Duration::from_millis(100));
                        match std::fs::remove_dir_all(&path) {
                            Ok(()) => return,
                            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                                return;
                            }
                            Err(_) => {}
                        }
                    }
                });
        }
    }
}
/// Open only after the trusted warning has collected the script choice.
#[tauri::command]
pub(super) async fn open_captured_page(
    id: String,
    enable_scripts: bool,
    options: Option<CapturedPageOptions>,
    source_label: Option<String>,
    operation_id: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<String, AppError> {
    let application = state.window_application(&window)?;
    let options = options.unwrap_or_default();
    if (options.rule_ids.is_some() || options.variant.is_some()) && window.label() != "main" {
        return Err(error(
            "Open this capture in the main window to use saved auto-responses",
        ));
    }
    let label = format!(
        "captured-page-{}",
        ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let (_operation, canceled) = register_operation(&window, &operation_id, &label)?;
    let page = prepare_scene(
        &application,
        window.label(),
        id,
        options,
        source_label,
        canceled.clone(),
    )
    .await?;
    if canceled.load(Ordering::Acquire)
        || window
            .app_handle()
            .get_webview_window(window.label())
            .is_none()
    {
        return Err(error("Captured page preview canceled"));
    }
    let app = window.app_handle().clone();
    let owner = Arc::new(PreviewOwner::new(page, enable_scripts)?);
    let address = owner.firewall.address;
    let title = preview_title(&owner.page, enable_scripts);
    let creation_owner = owner.clone();
    let creation_app = app.clone();
    let creation_label = label.clone();
    let browser=tokio::task::spawn_blocking(move||{
        let args=format!("--proxy-server=http://{address} --proxy-bypass-list=<-loopback> --force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-background-networking --disable-quic");
        WebviewWindowBuilder::new(&creation_app,&creation_label,WebviewUrl::External("about:blank".parse().expect("fixed blank URL")))
            .title(title).inner_size(1100.0,800.0).visible(false)
            .data_directory(creation_owner.profile.path().to_owned()).incognito(true).additional_browser_args(&args)
            .initialization_script_for_all_frames(HARDENING).disable_javascript()
            .on_new_window(|_,_|tauri::webview::NewWindowResponse::Deny)
            .on_navigation(|url|url.as_str()=="about:blank"||matches!(url.scheme(),"http"|"https"))
            .build().map_err(|_|error("Captured preview window could not be created"))
    }).await.map_err(|_|error("Captured preview creation failed"))??;
    let owner_browser = browser.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = owner_browser.destroy();
        }
    });
    if canceled.load(Ordering::Acquire) {
        let _ = browser.destroy();
        return Err(error("Captured page preview canceled"));
    }
    let (ready, prepared) = tokio::sync::oneshot::channel();
    let original_url = owner.page.url.clone();
    let user_agent = owner.page.user_agent.clone();
    let lookup_owner = owner.clone();
    let failed_window = browser.clone();
    if browser.with_webview(move|native|{
        let lookup:transmog_webview_preview_windows::Lookup=Arc::new(move|request|lookup_owner.page.resolve(&request.method,&request.url,request.headers.as_deref(),request.body_sha256.as_deref(),request.document).map(|resource|transmog_webview_preview_windows::CapturedResponse{status:resource.status,reason:resource.reason,headers:resource.headers,body_path:resource.body_path}));
        let navigation_failed:Arc<dyn Fn()+Send+Sync>=Arc::new(move||{let window=failed_window.clone();tauri::async_runtime::spawn(async move{let _=window.destroy();});});
        let result=transmog_webview_preview_windows::attach(&native.controller(),&native.environment(),lookup,enable_scripts,&original_url,user_agent.as_deref(),navigation_failed);
        let _=ready.send(result.map_err(|_|error("Captured preview needs an InPrivate WebView2 session with complete resource interception. Update the runtime and try again.")));
    }).is_err(){let _=browser.destroy();return Err(error("Captured preview initialization failed"));}
    match tokio::time::timeout(Duration::from_secs(10), prepared).await {
        Ok(Ok(Ok(()))) => {
            if canceled.load(Ordering::Acquire) {
                let _ = browser.destroy();
                return Err(error("Captured page preview canceled"));
            }
            if browser.show().is_err() {
                let _ = browser.destroy();
                return Err(error("Captured preview could not be shown"));
            }
            if !retain_preview(window.label(), &label, owner.page.clone(), &canceled) {
                let _ = browser.destroy();
                return Err(error("Captured page preview canceled"));
            }
            Ok(label)
        }
        result => {
            let _ = browser.destroy();
            match result {
                Ok(Ok(Err(error))) => Err(error),
                _ => Err(error("Captured preview initialization timed out")),
            }
        }
    }
}

async fn prepare_scene(
    application: &transmog_app::Application,
    owner: &str,
    id: String,
    mut options: CapturedPageOptions,
    source_label: Option<String>,
    canceled: Arc<AtomicBool>,
) -> Result<CapturedPage, AppError> {
    if let Some(input) = options.case_preview.take() {
        if options.rule_ids.is_some() || options.variant.is_some() || source_label.is_some() {
            return Err(error("Choose a case preview or saved workbench rules"));
        }
        return super::portable_cases::prepare_preview(application, owner, id, input, canceled)
            .await;
    }
    if let Some(label) = source_label {
        let page = reports().lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter()
            .find(|run| run.owner == owner && run.label == label).and_then(|run| run.page.clone())
            .ok_or_else(|| error("These preview bytes have expired; select the captured HTML and open a new preview"))?;
        if let Some(reference) = &options.page_reference
            && (!application.experiment_page_capture_current(reference)
                || reference.entry_id != page.diagnostics().snapshot().selected_entry_id)
        {
            return Err(error(
                "The chosen captured page is unavailable. Choose a replacement in Response sets.",
            ));
        }
        application
            .prepare_related_captured_page(page, options, canceled)
            .await
    } else {
        if let Some(reference) = &options.page_reference
            && (reference.entry_id != id || !application.experiment_page_available(reference))
        {
            return Err(error(
                "The chosen captured page is unavailable. Choose a replacement in Response sets.",
            ));
        }
        application
            .prepare_captured_page_with_options(id, options, canceled)
            .await
    }
}

fn preview_title(page: &CapturedPage, enable_scripts: bool) -> String {
    let report = page.diagnostics().snapshot();
    format!(
        "{} preview · Scripts {} — Transmog",
        if report.rules.is_empty() {
            "Baseline"
        } else {
            "Modified"
        },
        if enable_scripts { "on" } else { "off" }
    )
}
fn retain_preview(
    owner: &str,
    label: &str,
    page: Arc<CapturedPage>,
    canceled: &AtomicBool,
) -> bool {
    let mut reports = reports()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if canceled.load(Ordering::Acquire) {
        return false;
    }
    if reports.len() == 8 {
        reports.pop_front();
    }
    // Reports are cheap; retain at most three scenes (256 MiB/run) for iteration.
    while reports.iter().filter(|run| run.page.is_some()).count() >= 3 {
        if let Some(run) = reports.iter_mut().find(|run| run.page.is_some()) {
            run.page = None;
        }
    }
    reports.push_back(RetainedPreview {
        owner: owner.into(),
        label: label.into(),
        diagnostics: page.diagnostics(),
        page: Some(page),
    });
    true
}

fn register_operation(
    window: &tauri::WebviewWindow,
    operation_id: &str,
    label: &str,
) -> Result<(Operation, Arc<AtomicBool>), AppError> {
    if operation_id.is_empty() || operation_id.len() > 128 {
        return Err(error("Invalid captured preview operation"));
    }
    let key = format!("{}:{operation_id}", window.label());
    let canceled = Arc::new(AtomicBool::new(false));
    {
        let mut pending = operations()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.contains_key(&key) {
            return Err(error("Captured preview is already preparing"));
        }
        let admitted = window
            .app_handle()
            .webview_windows()
            .keys()
            .filter(|label| label.starts_with("captured-page-"))
            .cloned()
            .chain(pending.values().map(|pending| pending.label.clone()))
            .collect::<std::collections::HashSet<_>>();
        if admitted.len() >= 3 {
            return Err(error(
                "Close a captured page preview before opening another (maximum 3)",
            ));
        }
        pending.insert(
            key.clone(),
            PreparingPreview {
                canceled: canceled.clone(),
                label: label.into(),
            },
        );
    }
    let operation = Operation(key);
    let weak = Arc::downgrade(&canceled);
    let owner = window.label().to_owned();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(canceled) = weak.upgrade() {
                canceled.store(true, Ordering::Release);
            }
            reports()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|run| run.owner != owner);
        }
    });
    Ok((operation, canceled))
}

struct DenyProxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl DenyProxy {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let worker = thread::Builder::new()
            .name("captured-preview-deny".into())
            .spawn(move || {
                while !done.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            if active.fetch_add(1, Ordering::AcqRel) >= 16 {
                                active.fetch_sub(1, Ordering::AcqRel);
                                continue;
                            }
                            let client_active = active.clone();
                            if thread::Builder::new()
                                .name("captured-preview-deny-client".into())
                                .spawn(move || {
                                    deny(socket);
                                    client_active.fetch_sub(1, Ordering::AcqRel);
                                })
                                .is_err()
                            {
                                active.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            address,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for DenyProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn deny(mut socket: TcpStream) {
    let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = socket.set_write_timeout(Some(Duration::from_millis(500)));
    let mut buffer = [0u8; 2048];
    let mut bytes = 0;
    while bytes < 16 * 1024 {
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                bytes += count;
                if buffer[..count].windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    // A missing capture is a network failure, never a fabricated server response.
    let _ = socket.shutdown(std::net::Shutdown::Both);
}
fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deny_endpoint_closes_without_fabricating_an_http_response() {
        use std::io::Write;
        let proxy = DenyProxy::start().unwrap();
        for request in [
            b"GET http://origin.invalid/missing HTTP/1.1\r\nHost: origin.invalid\r\n\r\n"
                .as_slice(),
            b"CONNECT origin.invalid:443 HTTP/1.1\r\nHost: origin.invalid:443\r\n\r\n".as_slice(),
        ] {
            let mut client = TcpStream::connect(proxy.address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            client.write_all(request).unwrap();
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).unwrap();
            assert!(bytes.is_empty());
        }
    }
}
