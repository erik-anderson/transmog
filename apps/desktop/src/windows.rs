use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tauri::{
    State, WebviewWindowBuilder,
    http::{Request, Response, StatusCode, header},
    utils::config::WebviewUrl,
};
use transmog_app::{AppConfig, AppError, AppStatus, Application};
use transmog_app_webui::{AppRenderer, ShellView, UiError, UiResponse};

const UI_HOST: &str = "transmog-ui.localhost";

#[derive(Clone)]
struct DesktopState {
    application: Application,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn app_status(state: State<'_, DesktopState>) -> AppStatus {
    state.application.status()
}

#[tauri::command]
async fn stop_application(state: State<'_, DesktopState>) -> Result<AppStatus, AppError> {
    state.application.shutdown().await?;
    Ok(state.application.status())
}

/// Runs the Windows `WebView2` host with a fixed embedded origin.
///
/// # Panics
///
/// Panics if the embedded assets, application service, or Tauri host cannot initialize.
pub fn run() {
    let application = Application::new(AppConfig::default()).expect("application must initialize");
    let renderer = AppRenderer::new().expect("embedded WebUI assets must be valid");
    let protocol_application = application.clone();
    let close_application = application.clone();
    let close_started = Arc::new(AtomicBool::new(false));
    let close_guard = Arc::clone(&close_started);

    tauri::Builder::default()
        .manage(DesktopState {
            application: application.clone(),
        })
        .register_uri_scheme_protocol("transmog-ui", move |_context, request: Request<Vec<u8>>| {
            let view = ShellView::from(&protocol_application.status());
            into_tauri_response(renderer.respond(
                request.method().as_str(),
                request.uri().path(),
                &view,
            ))
        })
        .invoke_handler(tauri::generate_handler![app_status, stop_application])
        .setup(|app| {
            let url = tauri::Url::parse("transmog-ui://localhost/")
                .expect("fixed application URL must parse");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
                .title("Transmog")
                .inner_size(1180.0, 760.0)
                .min_inner_size(760.0, 520.0)
                .on_navigation(is_allowed_navigation)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && !close_guard.swap(true, Ordering::AcqRel)
            {
                api.prevent_close();
                let application = close_application.clone();
                let window = window.clone();
                let close_guard = Arc::clone(&close_guard);
                tauri::async_runtime::spawn(async move {
                    if application.shutdown().await.is_ok() {
                        let _ = window.destroy();
                    } else {
                        close_guard.store(false, Ordering::Release);
                        let _ = window.set_title("Transmog — shutdown needs attention");
                    }
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("Tauri desktop host failed");
}

fn is_allowed_navigation(url: &tauri::Url) -> bool {
    (url.scheme() == "transmog-ui" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some(UI_HOST))
}

fn into_tauri_response(result: Result<UiResponse, UiError>) -> Response<Vec<u8>> {
    let response = match result {
        Ok(response) => response,
        Err(_) => UiResponse {
            status: 500,
            content_type: "text/plain; charset=utf-8",
            cache_control: "no-store",
            content_security_policy: None,
            body: b"embedded UI unavailable".to_vec(),
        },
    };
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, response.content_type)
        .header(header::CACHE_CONTROL, response.cache_control)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header("cross-origin-opener-policy", "same-origin");
    if let Some(csp) = response.content_security_policy {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, csp);
    }
    builder
        .body(response.body)
        .expect("fixed response headers must be valid")
}

#[cfg(test)]
mod tests {
    use super::is_allowed_navigation;

    #[test]
    fn navigation_is_limited_to_the_embedded_origin() {
        for allowed in [
            "transmog-ui://localhost/",
            "transmog-ui://localhost/app.js",
            "http://transmog-ui.localhost/",
        ] {
            assert!(is_allowed_navigation(&tauri::Url::parse(allowed).unwrap()));
        }
        for denied in [
            "https://transmog-ui.localhost/",
            "http://localhost/",
            "https://example.com/",
            "transmog-ui://attacker.invalid/",
        ] {
            assert!(!is_allowed_navigation(&tauri::Url::parse(denied).unwrap()));
        }
    }
}
