//! Isolated captured-page windows. The private proxy only denies traffic and
//! never contacts an origin, alters system routing or installs a certificate.
use super::{ARTIFACT_SEQUENCE, DesktopState};
static OPERATIONS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
> = std::sync::OnceLock::new();
fn operations() -> &'static std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>> {
    OPERATIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
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
        canceled.store(true, Ordering::Release);
    }
}
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::{Manager, WebviewWindowBuilder, utils::config::WebviewUrl};
use transmog_app::{AppError, CapturedPage, ErrorCategory};

const HARDENING: &str = r"(() => {
  const blocked = class { constructor() { throw new DOMException('Unavailable in a captured preview', 'NotSupportedError'); } };
  for (const name of ['RTCPeerConnection', 'webkitRTCPeerConnection', 'WebTransport', 'WebSocket']) {
    Object.defineProperty(globalThis, name, { value: blocked, writable: false, configurable: false });
  }
})();";

struct PreviewOwner {
    page: CapturedPage,
    profile: PreviewProfile,
    _firewall: DenyProxy,
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
    operation_id: String,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
) -> Result<String, AppError> {
    let application = state.window_application(&window)?;
    let (_operation, canceled) = register_operation(&window, &operation_id)?;
    let page = application
        .prepare_captured_page(id, canceled.clone())
        .await?;
    let app = window.app_handle().clone();
    let profile = tempfile::Builder::new()
        .prefix("transmog-captured-webview-")
        .tempdir()
        .map_err(|_| error("An isolated preview profile could not be created"))?;
    let firewall = DenyProxy::start()
        .map_err(|_| error("Captured preview network isolation could not start"))?;
    let address = firewall.address;
    let owner = Arc::new(PreviewOwner {
        page,
        profile: PreviewProfile(Some(profile)),
        _firewall: firewall,
    });
    let creation_owner = owner.clone();
    let creation_app = app.clone();
    let label = format!(
        "captured-page-{}",
        ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let creation_label = label.clone();
    let browser=tokio::task::spawn_blocking(move||{
        let args=format!("--proxy-server=http://{address} --proxy-bypass-list=<-loopback> --force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-background-networking --disable-quic");
        WebviewWindowBuilder::new(&creation_app,&creation_label,WebviewUrl::External("about:blank".parse().expect("fixed blank URL")))
            .title("Captured page preview — Transmog").inner_size(1100.0,800.0).visible(false)
            .data_directory(creation_owner.profile.path().to_owned()).additional_browser_args(&args)
            .initialization_script_for_all_frames(HARDENING).disable_javascript()
            .on_new_window(|_,_|tauri::webview::NewWindowResponse::Deny)
            .on_navigation(|url|url.as_str()=="about:blank"||matches!(url.scheme(),"http"|"https"))
            .build().map_err(|_|error("Captured preview window could not be created"))
    }).await.map_err(|_|error("Captured preview creation failed"))??;
    if canceled.load(Ordering::Acquire) {
        let _ = browser.destroy();
        return Err(error("Captured page preview canceled"));
    }
    let (ready, prepared) = tokio::sync::oneshot::channel();
    let original_url = owner.page.url.clone();
    let lookup_owner = owner.clone();
    if browser.with_webview(move|native|{
        let lookup:transmog_webview_preview_windows::Lookup=Arc::new(move|method,url|lookup_owner.page.resource(method,url).map(|resource|transmog_webview_preview_windows::CapturedResponse{status:resource.status,reason:resource.reason,headers:resource.headers,body_path:resource.body_path}));
        let result=transmog_webview_preview_windows::attach(&native.controller(),&native.environment(),lookup,enable_scripts,&original_url);
        let _=ready.send(result.map_err(|_|error("Captured preview needs a WebView2 runtime with complete resource interception. Update the runtime and try again.")));
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

fn register_operation(
    window: &tauri::WebviewWindow,
    operation_id: &str,
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
        if pending.len()
            + window
                .app_handle()
                .webview_windows()
                .keys()
                .filter(|label| label.starts_with("captured-page-"))
                .count()
            >= 3
        {
            return Err(error(
                "Close a captured page preview before opening another (maximum 3)",
            ));
        }
        pending.insert(key.clone(), canceled.clone());
    }
    let operation = Operation(key);
    if window
        .app_handle()
        .webview_windows()
        .keys()
        .filter(|label| label.starts_with("captured-page-"))
        .count()
        >= 3
    {
        return Err(error(
            "Close a captured page preview before opening another (maximum 3)",
        ));
    }
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
    let _ = socket
        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}
fn error(message: &str) -> AppError {
    AppError {
        category: ErrorCategory::Unavailable,
        message: message.into(),
        retryable: false,
    }
}
