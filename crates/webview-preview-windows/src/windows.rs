use std::{path::PathBuf, sync::Arc};
use webview2_com::{
    BasicAuthenticationRequestedEventHandler, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_STATE_DENY, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL, ICoreWebView2_4, ICoreWebView2_10,
        ICoreWebView2_22, ICoreWebView2Controller, ICoreWebView2Environment,
        ICoreWebView2Settings4, ICoreWebView2Settings5, ICoreWebView2WebResourceRequestedEventArgs,
        ICoreWebView2WebResourceResponse,
    },
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::{
            Com::{IStream, STGM_READ, STGM_SHARE_DENY_NONE},
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
pub type Lookup = Arc<dyn Fn(&str, &str) -> Option<CapturedResponse> + Send + Sync>;

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
        let Some(resource) = lookup(&method, &uri) else {
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
