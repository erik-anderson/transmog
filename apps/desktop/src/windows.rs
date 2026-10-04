use tauri::{
    WebviewWindowBuilder,
    http::{Request, Response, StatusCode, header},
    ipc::Channel,
    utils::config::WebviewUrl,
};

use crate::{ProbeError, ProbeHint, ProbeInput, ProbeResult, UiRenderer, UiResponse, run_probe};

const UI_HOST: &str = "rustymiddle-ui.localhost";

#[tauri::command]
async fn phase_zero_probe(
    input: ProbeInput,
    on_event: Channel<ProbeHint>,
) -> Result<ProbeResult, ProbeError> {
    let (hint, result) = run_probe(&input)?;
    on_event
        .send(hint)
        .map_err(|_| ProbeError::NotificationUnavailable)?;
    Ok(result)
}

/// Runs the Windows `WebView2` host with a fixed embedded origin.
///
/// # Panics
///
/// Panics if the build embedded invalid assets or configuration, or if the Tauri host
/// cannot create or run the application window.
pub fn run() {
    let renderer = UiRenderer::new().expect("embedded WebUI assets must be valid");

    tauri::Builder::default()
        .register_uri_scheme_protocol(
            "rustymiddle-ui",
            move |_context, request: Request<Vec<u8>>| {
                into_tauri_response(
                    renderer.respond(request.method().as_str(), request.uri().path()),
                )
            },
        )
        .invoke_handler(tauri::generate_handler![phase_zero_probe])
        .setup(|app| {
            let url = tauri::Url::parse("rustymiddle-ui://localhost/")
                .expect("fixed application URL must parse");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
                .title("rustymiddle")
                .inner_size(960.0, 680.0)
                .min_inner_size(680.0, 480.0)
                .on_navigation(is_allowed_navigation)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri desktop host failed");
}

fn is_allowed_navigation(url: &tauri::Url) -> bool {
    (url.scheme() == "rustymiddle-ui" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some(UI_HOST))
}

fn into_tauri_response(result: Result<UiResponse, crate::UiError>) -> Response<Vec<u8>> {
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
            "rustymiddle-ui://localhost/",
            "rustymiddle-ui://localhost/app.js",
            "http://rustymiddle-ui.localhost/",
        ] {
            let url = tauri::Url::parse(allowed).expect("allowed URL should parse");
            assert!(
                is_allowed_navigation(&url),
                "expected {allowed} to be allowed"
            );
        }

        for denied in [
            "https://rustymiddle-ui.localhost/",
            "http://localhost/",
            "https://example.com/",
            "rustymiddle-ui://attacker.invalid/",
        ] {
            let url = tauri::Url::parse(denied).expect("denied URL should parse");
            assert!(
                !is_allowed_navigation(&url),
                "expected {denied} to be denied"
            );
        }
    }
}
