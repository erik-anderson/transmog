use std::{path::PathBuf, sync::Arc};
use webview2_com::{
    BasicAuthenticationRequestedEventHandler, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL, ICoreWebView2_4, ICoreWebView2_10,
        ICoreWebView2_22, ICoreWebView2Controller, ICoreWebView2Environment,
        ICoreWebView2Settings4, ICoreWebView2Settings5, ICoreWebView2WebResourceRequest,
        ICoreWebView2WebResourceRequestedEventArgs, ICoreWebView2WebResourceResponse,
    },
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::{
    Win32::{
        Foundation::{E_POINTER, HWND},
        System::{
            Com::{IStream, STGM_READ, STGM_SHARE_DENY_NONE, STREAM_SEEK_SET},
            Threading::GetCurrentThreadId,
        },
        UI::{Shell::SHCreateStreamOnFileEx, WindowsAndMessaging::GetWindowThreadProcessId},
    },
    core::{HSTRING, Interface, PWSTR, Result},
};

/// A frozen browser-compatible response produced by the trusted capture facade.
#[derive(Clone)]
pub struct CapturedResponse {
    /// Captured status code.
    pub status: u16,
    /// Valid HTTP reason text.
    pub reason: String,
    /// Repaired browser headers, including the render-only policy.
    pub headers: String,
    /// Absolute owned immutable body path, materialized before browser creation.
    pub body_path: PathBuf,
}
/// Pure lookup. It must not perform I/O, access live app state or execute traffic.
pub type Lookup = Arc<dyn Fn(&PreviewRequest) -> Option<CapturedResponse> + Send + Sync>;
/// Bounded request facts obtained before any network request is permitted.
#[derive(Clone, Debug)]
pub struct PreviewRequest {
    /// Requested method.
    pub method: String,
    /// Original browser URL.
    pub url: String,
    /// Available request headers; none means native inspection failed or exceeded budget.
    pub headers: Option<Vec<(String, String)>>,
    /// SHA-256 of complete encoded body bytes; none means unavailable/over budget.
    pub body_sha256: Option<String>,
    /// True for a document request, including frame navigation.
    pub document: bool,
}
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// Installs fail-closed resource interception before navigating from about:blank.
///
/// Call from Tauri's `with_webview` UI-thread callback for a fresh isolated profile.
/// All event handlers live with the COM webview. Unsupported interception fails
/// before navigation; the host must destroy the hidden window on any error.
///
/// # Errors
/// Returns wrong UI-thread, unavailable runtime, policy or interception failures.
pub fn attach(
    controller: &ICoreWebView2Controller,
    environment: &ICoreWebView2Environment,
    lookup: Lookup,
    scripts: bool,
    url: &str,
) -> Result<()> {
    // SAFETY: The caller owns these COM references. ParentWindow writes into an
    // initialized HWND slot; the Win32 query only reads that valid window handle.
    // No interface is used beyond this check if its owning thread differs.
    unsafe {
        let mut parent = HWND::default();
        controller.ParentWindow(&raw mut parent)?;
        if GetWindowThreadProcessId(parent, None) != GetCurrentThreadId() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                -2_147_417_842,
            )));
        }
    }
    // SAFETY: All following COM operations run on the verified owning UI thread.
    // Interfaces are owned clones, HSTRING values stay alive for each call, and
    // COM retains callback interfaces until the webview is closed and released.
    unsafe {
        let core = controller.CoreWebView2()?;
        let settings = core.Settings()?;
        settings.SetIsScriptEnabled(scripts)?;
        settings.SetIsWebMessageEnabled(false)?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetAreDefaultScriptDialogsEnabled(false)?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetAreDevToolsEnabled(cfg!(debug_assertions))?;
        settings
            .cast::<ICoreWebView2Settings4>()?
            .SetIsPasswordAutosaveEnabled(false)?;
        settings
            .cast::<ICoreWebView2Settings5>()?
            .SetIsGeneralAutofillEnabled(false)?;
        let mut token = 0;
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            })),
            &raw mut token,
        )?;
        core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetCancel(true)?;
                }
                Ok(())
            })),
            &raw mut token,
        )?;
        core.cast::<ICoreWebView2_10>()?
            .add_BasicAuthenticationRequested(
                &BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &raw mut token,
            )?;
        core.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &raw mut token,
        )?;
        core.cast::<ICoreWebView2_22>()?
            .AddWebResourceRequestedFilterWithRequestSourceKinds(
                &HSTRING::from("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?;
        let response_environment = environment.clone();
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
            let Some(args) = args else {
                if let Some(sender) = sender {
                    sender.Stop()?;
                }
                return Ok(());
            };
            let response = resource_response(&response_environment, &args, &lookup)
                .or_else(|_| empty_response(&response_environment));
            match response {
                Ok(response) => {
                    if args.SetResponse(&response).is_err()
                        && let Some(sender) = sender
                    {
                        sender.Stop()?;
                    }
                }
                Err(_) => {
                    if let Some(sender) = sender {
                        sender.Stop()?;
                    }
                }
            }
            Ok(())
        }));
        core.add_WebResourceRequested(&handler, &raw mut token)?;
        core.Navigate(&HSTRING::from(url))?;
    }
    Ok(())
}
fn resource_response(
    environment: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    lookup: &Lookup,
) -> Result<ICoreWebView2WebResourceResponse> {
    // SAFETY: Called only by WebView2 on the owning UI thread. Request getters
    // allocate CoTaskMem strings into initialized slots; take_pwstr copies and
    // frees each allocation once. Stream creation opens a trusted owned path in
    // read-only mode. COM owns the stream reference after creating the response.
    unsafe {
        let request = args.Request()?;
        let mut uri = PWSTR::null();
        let mut method = PWSTR::null();
        request.Uri(&raw mut uri)?;
        let uri = take_pwstr(uri);
        request.Method(&raw mut method)?;
        let method = take_pwstr(method);
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
        args.ResourceContext(&raw mut context)?;
        let facts = PreviewRequest {
            method: method.clone(),
            url: uri,
            headers: request_headers(&request).ok(),
            body_sha256: request_body_hash(&request, &method).ok().flatten(),
            document: context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        };
        let Some(resource) = lookup(&facts) else {
            return empty_response(environment);
        };
        if !(200..=599).contains(&resource.status)
            || resource.reason.contains(['\r', '\n', '\0'])
            || resource.headers.contains('\0')
            || !resource.body_path.is_absolute()
        {
            return empty_response(environment);
        }
        let path = HSTRING::from(resource.body_path.to_string_lossy().as_ref());
        let body = SHCreateStreamOnFileEx(
            &path,
            STGM_READ.0 | STGM_SHARE_DENY_NONE.0,
            0,
            false,
            None::<&IStream>,
        )?;
        environment.CreateWebResourceResponse(
            &body,
            i32::from(resource.status),
            &HSTRING::from(resource.reason),
            &HSTRING::from(resource.headers),
        )
    }
}
fn empty_response(
    environment: &ICoreWebView2Environment,
) -> Result<ICoreWebView2WebResourceResponse> {
    // SAFETY: The owned environment is used on its UI thread. None supplies a
    // null optional stream; static owned HSTRING buffers live through the call.
    unsafe {
        environment.CreateWebResourceResponse(
            None::<&IStream>,
            404,
            &HSTRING::from("Not Found"),
            &HSTRING::from("Content-Length: 0\r\nCache-Control: no-store\r\n"),
        )
    }
}

fn request_headers(request: &ICoreWebView2WebResourceRequest) -> Result<Vec<(String, String)>> {
    // SAFETY: Request and iterator belong to this UI-thread callback. Output
    // slots are initialized; each CoTaskMem string is copied/freed once, even
    // when an iterator getter fails. No COM interface crosses threads.
    unsafe {
        let iterator = request.Headers()?.GetIterator()?;
        let mut has = windows::core::BOOL::default();
        iterator.HasCurrentHeader(&raw mut has)?;
        let mut result = Vec::new();
        let mut bytes = 0;
        while has.as_bool() {
            let mut name = PWSTR::null();
            let mut value = PWSTR::null();
            let status = iterator.GetCurrentHeader(&raw mut name, &raw mut value);
            let name = take_pwstr(name);
            let value = take_pwstr(value);
            status?;
            bytes += name.len() + value.len();
            if bytes > 64 * 1024 || result.len() >= 512 {
                return Err(windows::core::Error::from_hresult(E_POINTER));
            }
            result.push((name, value));
            iterator.MoveNext(&raw mut has)?;
        }
        Ok(result)
    }
}
fn request_body_hash(
    request: &ICoreWebView2WebResourceRequest,
    method: &str,
) -> Result<Option<String>> {
    use sha2::{Digest, Sha256};
    // SAFETY: The request is used only on its owning UI thread. A cloned stream
    // has its own seek position. If cloning is unsupported we may consume the
    // original: every path supplies a synthetic response or stops navigation;
    // the body is never forwarded to a network endpoint.
    // Read receives initialized, correctly sized writable storage and a live
    // byte-count slot. The returned count is checked before slicing. Inspection
    // is bounded, and failures produce no signature (therefore an empty 404).
    unsafe {
        let content = match request.Content() {
            Ok(content) => content,
            Err(error) if error.code() == E_POINTER || matches!(method, "GET" | "HEAD") => {
                return Ok(Some(format!("{:x}", Sha256::digest([]))));
            }
            Err(error) => return Err(error),
        };
        let stream = content.Clone().unwrap_or(content);
        let _ = stream.Seek(0, STREAM_SEEK_SET, None);
        let mut digest = Sha256::new();
        let mut bytes = 0;
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let mut count = 0_u32;
            stream
                .Read(
                    buffer.as_mut_ptr().cast(),
                    u32::try_from(buffer.len()).expect("bounded buffer"),
                    Some(&raw mut count),
                )
                .ok()?;
            let count = usize::try_from(count).expect("bounded native count");
            if count > buffer.len() {
                return Ok(None);
            }
            if count == 0 {
                break;
            }
            bytes += count;
            if bytes > MAX_REQUEST_BYTES {
                return Ok(None);
            }
            digest.update(&buffer[..count]);
        }
        Ok(Some(format!("{:x}", digest.finalize())))
    }
}
