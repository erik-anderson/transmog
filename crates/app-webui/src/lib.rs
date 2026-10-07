#![deny(missing_docs)]

//! Bounded `WebUI` presentation mapping and embedded-origin renderer.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use thiserror::Error;
use transmog_app::{AppLifecycle, AppStatus};
use webui::{Protocol, RenderOptions, ResponseWriter, WebUIHandler};
use webui_handler::plugin::webui::WebUIHydrationPlugin;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

const DOCUMENT_STYLES: &[u8] = include_bytes!("../../../apps/desktop/ui/dist/document.css");
const APP_ICON: &[u8] = include_bytes!("../../../apps/desktop/icons/icon.ico");
const APP_LOGO: &[u8] = include_bytes!("../../../apps/desktop/icons/icon.svg");
const DOCUMENT_CSP_PREFIX: &str = "default-src 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'; script-src 'self' 'nonce-";
const DOCUMENT_CSP_SUFFIX: &str = "'; worker-src 'self'; style-src 'self' 'unsafe-inline'; font-src data:; img-src 'self' data: transmog-preview: http://transmog-preview.localhost; connect-src 'self' ipc: http://ipc.localhost; require-trusted-types-for 'script'; trusted-types webui monaco";

/// Complete bounded state used to render the application shell.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellView {
    /// HTML document language.
    pub language: &'static str,
    /// Window and document title.
    pub page_title: &'static str,
    /// Product heading.
    pub heading: &'static str,
    /// Short product description.
    pub summary: &'static str,
    /// Human-readable lifecycle label.
    pub lifecycle_label: &'static str,
    /// Stable lifecycle value for styling.
    pub lifecycle_kind: &'static str,
    /// Current listener or a safe inactive placeholder.
    pub listener: String,
}

impl From<&AppStatus> for ShellView {
    fn from(status: &AppStatus) -> Self {
        let (lifecycle_label, lifecycle_kind) = match status.lifecycle {
            AppLifecycle::Stopped => ("Stopped", "stopped"),
            AppLifecycle::Running => ("Running", "running"),
            AppLifecycle::Stopping => ("Stopping", "stopping"),
            AppLifecycle::Failed => ("Needs attention", "failed"),
        };
        Self {
            language: "en",
            page_title: "Transmog",
            heading: "Inspect traffic without losing the thread",
            summary: "A native, bounded workspace over the Transmog proxy core.",
            lifecycle_label,
            lifecycle_kind,
            listener: status
                .listener
                .clone()
                .unwrap_or_else(|| "Not listening".to_owned()),
        }
    }
}

/// A custom-protocol response safe to hand to the desktop host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiResponse {
    /// HTTP-compatible status.
    pub status: u16,
    /// MIME type with charset for text resources.
    pub content_type: &'static str,
    /// Cache policy.
    pub cache_control: &'static str,
    /// Document CSP, when applicable.
    pub content_security_policy: Option<String>,
    /// Fully bounded response body.
    pub body: Vec<u8>,
}

impl UiResponse {
    fn asset(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            cache_control: "no-store",
            content_security_policy: None,
            body: body.into(),
        }
    }
}

/// Embedded renderer construction or rendering error.
#[derive(Debug, Error)]
pub enum UiError {
    /// The compiled `WebUI` protocol is invalid.
    #[error("embedded WebUI protocol is invalid: {0}")]
    InvalidProtocol(String),
    /// The document could not be rendered.
    #[error("WebUI document render failed: {0}")]
    Render(String),
    /// A CSP nonce could not be generated.
    #[error("CSP nonce generation failed: {0}")]
    Nonce(String),
    /// A bounded presentation state could not be serialized.
    #[error("WebUI presentation state serialization failed: {0}")]
    State(String),
}

/// Immutable renderer that loads the `WebUI` protocol exactly once.
#[derive(Clone)]
pub struct AppRenderer {
    protocol: Arc<Protocol>,
}

impl AppRenderer {
    /// Loads and validates the embedded protocol.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::InvalidProtocol`] if build output is malformed.
    pub fn new() -> Result<Self, UiError> {
        let protocol = Protocol::from_protobuf(PROTOCOL_BYTES)
            .map_err(|error| UiError::InvalidProtocol(error.to_string()))?;
        Ok(Self {
            protocol: Arc::new(protocol),
        })
    }

    /// Serves only allowlisted embedded-origin paths.
    ///
    /// # Errors
    ///
    /// Returns a renderer or nonce failure for the document path.
    pub fn respond(
        &self,
        method: &str,
        path: &str,
        view: &ShellView,
    ) -> Result<UiResponse, UiError> {
        if !matches!(method, "GET" | "HEAD") {
            return Ok(UiResponse::asset(
                405,
                "text/plain; charset=utf-8",
                b"method not allowed".to_vec(),
            ));
        }
        let path = path.split_once('?').map_or(path, |(path, _)| path);
        let mut response = match path {
            "/" | "/index.html" => self.render_document(view)?,
            "/document.css" => {
                UiResponse::asset(200, "text/css; charset=utf-8", DOCUMENT_STYLES.to_vec())
            }
            "/favicon.ico" => UiResponse::asset(200, "image/x-icon", APP_ICON.to_vec()),
            "/transmog-icon.svg" => {
                UiResponse::asset(200, "image/svg+xml; charset=utf-8", APP_LOGO.to_vec())
            }
            asset_path => {
                if let Some((_, content_type, bytes)) = CLIENT_ASSETS
                    .iter()
                    .find(|(path, _, _)| *path == asset_path)
                {
                    UiResponse::asset(200, content_type, bytes.to_vec())
                } else if let Some((_, bytes)) =
                    CSS_ASSETS.iter().find(|(path, _)| *path == asset_path)
                {
                    UiResponse::asset(200, "text/css; charset=utf-8", bytes.to_vec())
                } else {
                    UiResponse::asset(404, "text/plain; charset=utf-8", b"not found".to_vec())
                }
            }
        };
        if method == "HEAD" {
            response.body.clear();
        }
        Ok(response)
    }

    fn render_document(&self, view: &ShellView) -> Result<UiResponse, UiError> {
        let nonce = generate_nonce()?;
        let mut state: serde_json::Value = serde_json::from_str(include_str!(
            "../../../apps/desktop/ui/src/initial-state.json"
        ))
        .map_err(|error| UiError::State(error.to_string()))?;
        state
            .as_object_mut()
            .expect("initial client state is an object")
            .extend(
                serde_json::to_value(view)
                    .map_err(|error| UiError::State(error.to_string()))?
                    .as_object()
                    .expect("shell view is an object")
                    .clone(),
            );
        let handler = WebUIHandler::with_plugin(|| Box::new(WebUIHydrationPlugin::new()));
        let mut writer = StringWriter::default();
        handler
            .render(
                &self.protocol,
                &state,
                &RenderOptions::new("index.html", "/").with_nonce(&nonce),
                &mut writer,
            )
            .map_err(|error| UiError::Render(error.to_string()))?;
        Ok(UiResponse {
            status: 200,
            content_type: "text/html; charset=utf-8",
            cache_control: "no-store",
            content_security_policy: Some(format!(
                "{DOCUMENT_CSP_PREFIX}{nonce}{DOCUMENT_CSP_SUFFIX}"
            )),
            body: writer.output.into_bytes(),
        })
    }
}

#[derive(Default)]
struct StringWriter {
    output: String,
}

impl ResponseWriter for StringWriter {
    fn write(&mut self, content: &str) -> webui_handler::Result<()> {
        self.output.push_str(content);
        Ok(())
    }

    fn end(&mut self) -> webui_handler::Result<()> {
        Ok(())
    }
}

fn generate_nonce() -> Result<String, UiError> {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes).map_err(|error| UiError::Nonce(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> AppStatus {
        AppStatus {
            lifecycle: AppLifecycle::Stopped,
            listener: None,
            summary: "Proxy stopped".to_owned(),
            host_restore_pending: false,
        }
    }

    #[test]
    fn renderer_is_tauri_free_bounded_and_secure() {
        let renderer = AppRenderer::new().unwrap();
        let response = renderer
            .respond("GET", "/", &ShellView::from(&status()))
            .unwrap();
        let html = String::from_utf8(response.body).unwrap();
        let csp = response.content_security_policy.unwrap();
        assert!(html.contains("Inspect traffic without losing the thread"));
        assert!(html.contains("app-shell"));
        assert!(html.contains("id=\"webui-data\""));
        assert!(csp.contains("require-trusted-types-for 'script'"));
        assert!(csp.contains("trusted-types webui monaco"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(csp.contains("font-src data:"));
        assert!(csp.contains("transmog-preview:"));
        assert!(csp.contains("http://transmog-preview.localhost"));
        assert!(!csp.contains("unsafe-eval"));
        assert!(csp.contains("worker-src 'self'"));
        for path in [
            "/document.css",
            "/monaco.css",
            "/monaco-editor.worker.js",
            "/monaco-ts.worker.js",
            "/transmog-icon.svg",
        ] {
            assert_eq!(
                renderer
                    .respond("GET", path, &ShellView::from(&status()))
                    .unwrap()
                    .status,
                200
            );
        }
    }

    #[test]
    fn origin_does_not_expose_filesystem_paths() {
        let renderer = AppRenderer::new().unwrap();
        let view = ShellView::from(&status());
        assert_eq!(
            renderer
                .respond("GET", "/../../Cargo.toml", &view)
                .unwrap()
                .status,
            404
        );
        assert_eq!(renderer.respond("POST", "/", &view).unwrap().status, 405);
        assert!(
            renderer
                .respond("HEAD", "/", &view)
                .unwrap()
                .body
                .is_empty()
        );
    }

    #[test]
    fn split_assets_are_bounded_and_keep_their_mime_types() {
        let renderer = AppRenderer::new().unwrap();
        let view = ShellView::from(&status());
        assert!(
            CLIENT_ASSETS
                .iter()
                .any(|(path, _, _)| path.starts_with("/chunks/"))
        );
        for (path, mime, bytes) in CLIENT_ASSETS {
            let get = renderer.respond("GET", path, &view).unwrap();
            assert_eq!(get.status, 200);
            assert_eq!(get.content_type, *mime);
            assert_eq!(get.body, *bytes);
            assert!(
                renderer
                    .respond("HEAD", path, &view)
                    .unwrap()
                    .body
                    .is_empty()
            );
        }
        for path in [
            "/chunks/missing.js",
            "/chunks/../app.js",
            "/client-assets.json",
            "/client-metafile.json",
        ] {
            assert_eq!(renderer.respond("GET", path, &view).unwrap().status, 404);
        }
    }

    #[test]
    fn initial_document_has_reactive_defaults_without_loading_the_editor() {
        let response = AppRenderer::new()
            .unwrap()
            .respond("GET", "/", &ShellView::from(&status()))
            .unwrap();
        let html = String::from_utf8(response.body).unwrap();
        assert!(html.contains("No matching sessions."));
        assert!(html.contains("Application facade ready."));
        assert!(html.contains("aria-current=\"page\""));
        assert!(!html.contains("<link rel=\"stylesheet\" href=\"/monaco.css\""));
        assert!(!html.contains("<script type=\"module\" src=\"/monaco.js\""));
    }
}
