//! `WebUI` rendering and bounded product-shell contracts for the desktop spike.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use webui::{Protocol, RenderOptions, ResponseWriter, WebUIHandler};
use webui_handler::plugin::webui::WebUIHydrationPlugin;

include!(concat!(env!("OUT_DIR"), "/webui/assets.rs"));

const CLIENT_BUNDLE: &[u8] = include_bytes!("../ui/dist/app.js");
const APP_ICON: &[u8] = include_bytes!("../icons/icon.ico");
const MAX_PROBE_LABEL_BYTES: usize = 64;
const DOCUMENT_CSP_PREFIX: &str = "default-src 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'; script-src 'self' 'nonce-";
const DOCUMENT_CSP_SUFFIX: &str = "'; style-src 'self'; img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; require-trusted-types-for 'script'; trusted-types webui";

/// A custom-protocol response whose bytes and headers are safe to hand to Tauri.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiResponse {
    /// HTTP-compatible status returned by the custom protocol.
    pub status: u16,
    /// MIME type including an explicit UTF-8 charset for textual content.
    pub content_type: &'static str,
    /// Cache policy for the response.
    pub cache_control: &'static str,
    /// Optional document Content Security Policy.
    pub content_security_policy: Option<String>,
    /// Fully buffered response bytes.
    pub body: Vec<u8>,
}

impl UiResponse {
    fn text(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            cache_control: "no-store",
            content_security_policy: None,
            body: body.into(),
        }
    }
}

/// Errors produced while constructing or rendering the embedded UI.
#[derive(Debug, Error)]
pub enum UiError {
    /// The embedded build-time protocol was invalid.
    #[error("embedded WebUI protocol is invalid: {0}")]
    InvalidProtocol(String),
    /// A document could not be rendered.
    #[error("WebUI document render failed: {0}")]
    Render(String),
    /// Secure random nonce generation failed.
    #[error("CSP nonce generation failed: {0}")]
    Nonce(String),
}

/// Immutable, shareable renderer for the embedded `WebUI` protocol.
#[derive(Clone)]
pub struct UiRenderer {
    protocol: Arc<Protocol>,
}

impl UiRenderer {
    /// Loads and validates the checked-in application's compiled protocol.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::InvalidProtocol`] if the build-time protocol is invalid.
    pub fn new() -> Result<Self, UiError> {
        let protocol = Protocol::from_protobuf(PROTOCOL_BYTES)
            .map_err(|error| UiError::InvalidProtocol(error.to_string()))?;
        Ok(Self {
            protocol: Arc::new(protocol),
        })
    }

    /// Serves the fixed application origin. Unknown paths never reach the filesystem.
    ///
    /// # Errors
    ///
    /// Returns a [`UiError`] if the requested document cannot be rendered securely.
    pub fn respond(&self, method: &str, path: &str) -> Result<UiResponse, UiError> {
        if !matches!(method, "GET" | "HEAD") {
            return Ok(UiResponse::text(
                405,
                "text/plain; charset=utf-8",
                b"method not allowed".to_vec(),
            ));
        }

        let path = path.split_once('?').map_or(path, |(path, _)| path);
        let mut response = match path {
            "/" | "/index.html" => self.render_document()?,
            "/app.js" => UiResponse::text(
                200,
                "text/javascript; charset=utf-8",
                CLIENT_BUNDLE.to_vec(),
            ),
            "/favicon.ico" => UiResponse::text(200, "image/x-icon", APP_ICON.to_vec()),
            "/probe.json" => UiResponse::text(
                200,
                "application/json; charset=utf-8",
                br#"{"transport":"custom-protocol","bounded":true}"#.to_vec(),
            ),
            css_path => CSS_ASSETS
                .iter()
                .find(|(asset_path, _)| *asset_path == css_path)
                .map_or_else(
                    || UiResponse::text(404, "text/plain; charset=utf-8", b"not found".to_vec()),
                    |(_, bytes)| UiResponse::text(200, "text/css; charset=utf-8", bytes.to_vec()),
                ),
        };

        if method == "HEAD" {
            response.body.clear();
        }
        Ok(response)
    }

    fn render_document(&self) -> Result<UiResponse, UiError> {
        let nonce = generate_nonce()?;
        let state = json!({
            "language": "en",
            "pageTitle": "rustymiddle delivery spike",
            "introHeading": "Proxy inspection, from the core outward",
            "introSummary": "This server-rendered shell has no HTTP listener and no runtime Node process.",
            "probeHeading": "Delivery-path proof",
            "probeSummary": "The probe crosses the custom protocol, a typed Tauri command, and a bounded channel notification."
        });
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

/// The only payload accepted by the Phase 0 native command.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProbeInput {
    /// Human-readable label used only in the returned result.
    pub label: String,
    /// Browser-observed custom-protocol proof.
    pub custom_protocol_ok: bool,
}

/// A single bounded notification sent before the command completes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeHint {
    /// Monotonic revision for this self-contained spike.
    pub revision: u64,
    /// Stable category interpreted by the island.
    pub kind: &'static str,
}

/// Result of the narrowly scoped native probe.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// Validated label echoed by Rust.
    pub accepted_label: String,
    /// Confirms the command executed in Rust.
    pub rust_command_ok: bool,
}

/// Stable, presentation-safe failure returned across the Tauri boundary.
#[derive(Debug, Clone, Error, Serialize, PartialEq, Eq)]
#[serde(tag = "category", content = "message", rename_all = "kebab-case")]
pub enum ProbeError {
    /// The caller supplied invalid or excessive input.
    #[error("invalid probe input: {0}")]
    InvalidInput(&'static str),
    /// The bounded notification could not be delivered.
    #[error("probe notification could not be delivered")]
    NotificationUnavailable,
}

/// Validates the browser payload without depending on Tauri.
///
/// # Errors
///
/// Returns [`ProbeError::InvalidInput`] when the label is empty or too long, or when
/// the browser did not first confirm the custom-protocol delivery path.
pub fn run_probe(input: &ProbeInput) -> Result<(ProbeHint, ProbeResult), ProbeError> {
    let trimmed = input.label.trim();
    if trimmed.is_empty() {
        return Err(ProbeError::InvalidInput("label must not be empty"));
    }
    if trimmed.len() > MAX_PROBE_LABEL_BYTES {
        return Err(ProbeError::InvalidInput("label is too long"));
    }
    if !input.custom_protocol_ok {
        return Err(ProbeError::InvalidInput(
            "custom protocol must be verified first",
        ));
    }

    Ok((
        ProbeHint {
            revision: 1,
            kind: "refresh-available",
        },
        ProbeResult {
            accepted_label: trimmed.to_owned(),
            rust_command_ok: true,
        },
    ))
}

#[cfg(windows)]
mod windows;

/// Starts the Windows desktop shell.
#[cfg(windows)]
pub use windows::run;

/// Reports the intentionally unsupported platform when built elsewhere.
#[cfg(not(windows))]
pub fn run() {
    eprintln!("rustymiddle-desktop currently supports Windows with WebView2 only");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_server_state_and_security_headers() {
        let renderer = UiRenderer::new().expect("embedded protocol should load");
        let response = renderer
            .respond("GET", "/")
            .expect("document should render");
        let html = String::from_utf8(response.body).expect("document should be UTF-8");
        let csp = response
            .content_security_policy
            .expect("document should carry a CSP");

        assert_eq!(response.status, 200);
        assert!(html.contains("Proxy inspection, from the core outward"));
        assert!(html.contains("phase-zero-probe"));
        assert!(html.contains("id=\"webui-data\""));
        assert!(csp.contains("require-trusted-types-for 'script'"));
        assert!(csp.contains("trusted-types webui"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(!csp.contains("unsafe-eval"));
    }

    #[test]
    fn document_nonce_is_fresh() {
        let renderer = UiRenderer::new().expect("embedded protocol should load");
        let first = renderer.respond("GET", "/").expect("first render");
        let second = renderer.respond("GET", "/").expect("second render");
        assert_ne!(
            first.content_security_policy,
            second.content_security_policy
        );
    }

    #[test]
    fn serves_only_embedded_assets_and_probe() {
        let renderer = UiRenderer::new().expect("embedded protocol should load");
        let script = renderer.respond("GET", "/app.js").expect("script response");
        let probe = renderer
            .respond("GET", "/probe.json?cache-bust=1")
            .expect("probe response");
        let missing = renderer
            .respond("GET", "/../../Cargo.toml")
            .expect("missing response");

        assert_eq!(script.status, 200);
        assert_eq!(script.content_type, "text/javascript; charset=utf-8");
        assert!(String::from_utf8_lossy(&script.body).contains("custom-protocol"));
        assert_eq!(probe.status, 200);
        assert_eq!(missing.status, 404);
    }

    #[test]
    fn head_and_disallowed_methods_are_bounded() {
        let renderer = UiRenderer::new().expect("embedded protocol should load");
        let head = renderer.respond("HEAD", "/").expect("HEAD response");
        let post = renderer.respond("POST", "/").expect("POST response");

        assert_eq!(head.status, 200);
        assert!(head.body.is_empty());
        assert_eq!(post.status, 405);
    }

    #[test]
    fn probe_trims_and_validates_input() {
        let (hint, result) = run_probe(&ProbeInput {
            label: "  WebView2  ".to_owned(),
            custom_protocol_ok: true,
        })
        .expect("valid probe");

        assert_eq!(hint.revision, 1);
        assert_eq!(result.accepted_label, "WebView2");
        assert!(result.rust_command_ok);
        assert_eq!(
            run_probe(&ProbeInput {
                label: "ignored".to_owned(),
                custom_protocol_ok: false,
            }),
            Err(ProbeError::InvalidInput(
                "custom protocol must be verified first"
            ))
        );
    }
}
