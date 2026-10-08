//! Full-fidelity request actions. Generators return text and never execute it.

use std::{
    fmt::Write as _,
    io::{Read, Write},
    path::PathBuf,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use transmog_core::{
    HeaderBlock, HttpLegVersion, RequestHead, intercept::ExchangeId, observe::ExchangeBoundary,
};
use transmog_session::{ApplicationSessionService, SessionSnapshot, SessionTerminal};

use crate::{
    AppError, BodyAvailability, BodyStore, ComposerHeader, ErrorCategory,
    inspector::parse_session_id,
};

const INLINE_BYTES: u64 = 16 * 1024;
const COMPOSER_BYTES: u64 = 4 * 1024 * 1024;

/// Shell targeted by a clipboard-only request command.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestCommandFormat {
    /// cURL with POSIX shell quoting.
    Curl,
    /// cURL configuration piped by PowerShell 5.1 or 7 on Windows.
    CurlWindows,
    /// .NET HTTP commands compatible with PowerShell 5.1 and 7 on Windows.
    Powershell,
}

/// A generated command and explicit fidelity information for its clipboard UI.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCommand {
    /// Clipboard text. This application never runs it.
    pub text: String,
    /// Reasons a shell/HTTP stack cannot exactly reproduce the capture.
    pub notices: Vec<String>,
    /// Whether a body path must be supplied before executing the command.
    pub body_file_required: bool,
    /// Whether a complete retained body can currently be saved.
    pub body_file_available: bool,
}

/// Complete original request data suitable for loading an editable Composer.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposerSource {
    /// Original method without display truncation.
    pub method: String,
    /// Original absolute target without display truncation.
    pub url: String,
    /// Retained ordered headers, with transport framing removed.
    pub headers: Vec<ComposerHeader>,
    /// Complete encoded request bytes rendered as hex for lossless editing.
    pub body: String,
    /// Whether the complete request body is available.
    pub body_available: bool,
    /// Redaction or missing-body information.
    pub notices: Vec<String>,
}

/// A complete original request protected from cache eviction until save/cancel.
pub struct RequestFile {
    head: RequestHead,
    reader: Box<dyn Read + Send>,
    bytes: u64,
    length_known: bool,
}

impl RequestFile {
    /// Generates text referencing the selected path before consuming the lease.
    ///
    /// # Errors
    /// Returns a non-text header or unsupported platform error.
    pub fn command(
        &self,
        format: RequestCommandFormat,
        path: &str,
    ) -> Result<RequestCommand, AppError> {
        generate(&self.head, format, CommandBody::File(path), true)
    }

    /// Saves exact original bytes, including any content coding, atomically.
    ///
    /// # Errors
    /// Returns an incomplete-read or filesystem error; preserves the destination
    /// until the new file is complete. Overwrite consent belongs to the dialog.
    pub async fn save_to(mut self, destination: PathBuf) -> Result<(), AppError> {
        tokio::task::spawn_blocking(move || {
            if !destination.is_absolute() || destination.file_name().is_none() {
                return Err(invalid("Select an absolute request-body destination"));
            }
            let parent = destination
                .parent()
                .ok_or_else(|| invalid("Select an absolute request-body destination"))?;
            let mut file = tempfile::NamedTempFile::new_in(parent)
                .map_err(|_| unavailable("Request body file could not be created"))?;
            let copied = std::io::copy(
                &mut self.reader.by_ref().take(self.bytes.saturating_add(1)),
                &mut file,
            )
            .map_err(|_| unavailable("Request body bytes could not be saved"))?;
            if self.length_known && copied != self.bytes || copied > self.bytes {
                return Err(unavailable("Retained request body is incomplete"));
            }
            file.flush()
                .and_then(|()| file.as_file().sync_all())
                .map_err(|_| unavailable("Request body file could not be flushed"))?;
            file.persist(destination)
                .map_err(|_| unavailable("Request body file could not be saved"))?;
            Ok(())
        })
        .await
        .map_err(|_| unavailable("Request body save worker failed"))?
    }
}

pub(crate) fn prepare_file(
    service: &ApplicationSessionService,
    store: Option<&BodyStore>,
    id: &str,
) -> Result<RequestFile, AppError> {
    let snapshot = snapshot(service, id)?;
    let head = request_head(&snapshot)?.clone();
    let body = original_body(&snapshot, store)?;
    Ok(RequestFile {
        head,
        reader: body.reader,
        bytes: body.bytes,
        length_known: body.length_known,
    })
}

pub(crate) fn command(
    service: &ApplicationSessionService,
    store: Option<&BodyStore>,
    id: &str,
    format: RequestCommandFormat,
) -> Result<RequestCommand, AppError> {
    let snapshot = snapshot(service, id)?;
    let head = request_head(&snapshot)?;
    let Ok(body) = original_body(&snapshot, store) else {
        let mut command = generate(head, format, CommandBody::File("request-body.bin"), false)?;
        command.notices.push("The complete request body is unavailable. Supply your own file before running this command.".into());
        return Ok(command);
    };
    if body.bytes == 0 {
        return generate(head, format, CommandBody::Empty, true);
    }
    if body.bytes <= INLINE_BYTES || !body.length_known {
        let mut bytes = Vec::new();
        body.reader
            .take(INLINE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable("Request body could not be read"))?;
        if body.length_known && bytes.len() as u64 != body.bytes {
            return Err(unavailable("Retained request body is incomplete"));
        }
        if bytes.len() as u64 > INLINE_BYTES {
            let mut command = generate(head, format, CommandBody::File("request-body.bin"), true)?;
            command.notices.push("This body is too large for an inline command. Save its complete bytes and supply the file path.".into());
            return Ok(command);
        }
        if matches!(format, RequestCommandFormat::Powershell) {
            return generate(head, format, CommandBody::Bytes(&bytes), true);
        }
        if let Ok(text) = std::str::from_utf8(&bytes)
            && !text.contains('\0')
        {
            return generate(head, format, CommandBody::Text(text), true);
        }
    }
    let mut command = generate(head, format, CommandBody::File("request-body.bin"), true)?;
    command.notices.push("Save the complete request body, then replace request-body.bin with its path. Save as… also copies an updated command.".into());
    Ok(command)
}

pub(crate) fn composer_source(
    service: &ApplicationSessionService,
    store: Option<&BodyStore>,
    id: &str,
) -> Result<ComposerSource, AppError> {
    let snapshot = snapshot(service, id)?;
    let head = request_head(&snapshot)?;
    let mut notices = Vec::new();
    let mut headers = Vec::new();
    for field in head.headers.iter() {
        if field.is_redacted() {
            notices.push(format!(
                "{} was redacted; supply its value if needed.",
                text(field.name())?
            ));
            continue;
        }
        if framing(field) {
            continue;
        }
        headers.push(ComposerHeader {
            name: text(field.name())?.into(),
            value: text(field.value())?.into(),
        });
    }
    let mut available = false;
    let mut bytes = Vec::new();
    match original_body(&snapshot, store) {
        Ok(body) if body.bytes <= COMPOSER_BYTES || !body.length_known => {
            body.reader
                .take(COMPOSER_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| unavailable("Request body could not be read"))?;
            available = bytes.len() as u64 <= COMPOSER_BYTES
                && (!body.length_known || bytes.len() as u64 == body.bytes);
            if !available {
                notices.push("Request body exceeds the Composer's four-MiB input limit; supply a smaller body.".into());
            }
        }
        Ok(_) => notices.push(
            "Request body exceeds the Composer's four-MiB input limit; supply a smaller body."
                .into(),
        ),
        Err(_) => notices
            .push("The complete request body is unavailable. Supply it before sending.".into()),
    }
    if !available {
        bytes.clear();
    }
    let body = bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    });
    Ok(ComposerSource {
        method: head.method.clone(),
        url: url(head),
        headers,
        body,
        body_available: available,
        notices,
    })
}

pub(crate) fn copy_all_headers(
    service: &ApplicationSessionService,
    id: &str,
) -> Result<String, AppError> {
    let snapshot = snapshot(service, id)?;
    let head = request_head(&snapshot)?;
    let mut out = format!(
        "{} {} {}",
        head.method,
        url(head),
        snapshot
            .performance
            .protocols
            .iter()
            .find(|item| item.boundary == "client-request")
            .map_or(protocol(head.source_version), |item| item.version.as_str())
    );
    append_headers(&mut out, &head.headers)?;
    let response = snapshot
        .response_heads
        .iter()
        .find(|item| item.boundary == ExchangeBoundary::ClientResponse)
        .or_else(|| snapshot.response_heads.last());
    if let Some(response) = response {
        let observed = snapshot
            .performance
            .protocols
            .iter()
            .find(|item| item.boundary == crate::inspector::boundary(response.boundary));
        let reason = observed
            .and_then(|item| item.reason.as_deref())
            .or_else(|| {
                http::StatusCode::from_u16(response.head.status)
                    .ok()
                    .and_then(|status| status.canonical_reason())
            })
            .unwrap_or("");
        // Three CRLFs terminate the request line/block and leave two blank lines.
        write!(
            out,
            "\r\n\r\n\r\n{} {}",
            observed.map_or(protocol(response.head.source_version), |item| item
                .version
                .as_str()),
            response.head.status
        )
        .expect("string write");
        if !reason.is_empty() {
            write!(out, " {reason}").expect("string write");
        }
        append_headers(&mut out, &response.head.headers)?;
    }
    Ok(out)
}

fn snapshot(service: &ApplicationSessionService, id: &str) -> Result<SessionSnapshot, AppError> {
    service
        .catalog()
        .get(ExchangeId(parse_session_id(id)?))
        .ok_or_else(|| unavailable("The selected request is no longer retained"))
}

fn request_head(snapshot: &SessionSnapshot) -> Result<&RequestHead, AppError> {
    snapshot
        .request_heads
        .iter()
        .find(|item| item.boundary == ExchangeBoundary::ClientRequest)
        .or_else(|| snapshot.request_heads.first())
        .map(|item| &item.head)
        .ok_or_else(|| unavailable("Request headers are unavailable"))
}

struct OriginalBody {
    reader: Box<dyn Read + Send>,
    bytes: u64,
    length_known: bool,
}

fn original_body(
    snapshot: &SessionSnapshot,
    store: Option<&BodyStore>,
) -> Result<OriginalBody, AppError> {
    let head = request_head(snapshot)?;
    let boundary = snapshot
        .request_heads
        .iter()
        .find(|item| item.boundary == ExchangeBoundary::ClientRequest)
        .map_or(ExchangeBoundary::UpstreamRequest, |item| item.boundary);
    let observed = snapshot
        .bodies
        .iter()
        .find(|body| body.boundary == boundary);
    let known_empty = snapshot.sequence_loss == 0
        && matches!(snapshot.terminal, Some(SessionTerminal::Completed(_)))
        && observed.is_none_or(|body| body.observed_bytes == 0 && !body.truncated)
        && head
            .headers
            .values("content-length")
            .all(|value| value == b"0")
        && head.headers.values("transfer-encoding").next().is_none();
    if let Some(store) = store {
        store
            .flush()
            .map_err(|_| unavailable("Request body metadata is unavailable"))?;
        if let Some(metadata) = store
            .metadata(snapshot.exchange_id)
            .into_iter()
            .find(|body| body.boundary == crate::inspector::boundary(boundary))
        {
            if metadata.availability != BodyAvailability::Complete {
                if known_empty && !snapshot.imported {
                    return Ok(empty_body());
                }
                return Err(unavailable("Complete request bytes are unavailable"));
            }
            let reader: Box<dyn Read + Send> = if metadata.retained_bytes == 0 {
                Box::new(std::io::Cursor::new(Vec::<u8>::new()))
            } else {
                Box::new(
                    store
                        .open_complete(snapshot.exchange_id, boundary)
                        .map_err(|_| unavailable("Complete request bytes are unavailable"))?,
                )
            };
            return Ok(OriginalBody {
                reader,
                bytes: metadata.retained_bytes,
                length_known: metadata.length_known,
            });
        }
    }
    if known_empty {
        return Ok(empty_body());
    }
    Err(unavailable("Complete request bytes are unavailable"))
}

fn empty_body() -> OriginalBody {
    OriginalBody {
        reader: Box::new(std::io::Cursor::new(Vec::<u8>::new())),
        bytes: 0,
        length_known: true,
    }
}

#[derive(Clone, Copy)]
enum CommandBody<'a> {
    Empty,
    Text(&'a str),
    Bytes(&'a [u8]),
    File(&'a str),
}

fn generate(
    head: &RequestHead,
    format: RequestCommandFormat,
    body: CommandBody<'_>,
    available: bool,
) -> Result<RequestCommand, AppError> {
    if matches!(
        format,
        RequestCommandFormat::Powershell | RequestCommandFormat::CurlWindows
    ) && !cfg!(windows)
    {
        return Err(invalid("PowerShell generation is available on Windows"));
    }
    let mut notices = Vec::new();
    if head
        .headers
        .iter()
        .any(transmog_core::HeaderField::is_redacted)
    {
        notices.push("Redacted values are marked REPLACE_REDACTED_VALUE. Replace them before running the command.".into());
    }
    if head.headers.iter().any(framing) {
        notices.push("Content-Length and Transfer-Encoding are regenerated by the HTTP client for the supplied body.".into());
    }
    let headers = command_headers(&head.headers)?;
    let text = match format {
        RequestCommandFormat::Curl => curl(head, &headers, &body)?,
        RequestCommandFormat::CurlWindows => curl_windows(head, &headers, &body)?,
        RequestCommandFormat::Powershell => {
            notices.push("PowerShell 5.1 and 7 use .NET HTTP transport; it may normalize header formatting and uses HTTP/1.1. Redirects and automatic cookies are disabled.".into());
            powershell(head, &headers, &body)
        }
    };
    Ok(RequestCommand {
        text,
        notices,
        body_file_required: matches!(body, CommandBody::File(_)),
        body_file_available: available,
    })
}

fn command_headers(block: &HeaderBlock) -> Result<Vec<(String, String)>, AppError> {
    block
        .iter()
        .filter(|field| !framing(field))
        .map(|field| {
            Ok((
                text(field.name())?.into(),
                if field.is_redacted() {
                    "REPLACE_REDACTED_VALUE".into()
                } else {
                    text(field.value())?.into()
                },
            ))
        })
        .collect()
}

fn framing(field: &transmog_core::HeaderField) -> bool {
    field.name_eq("content-length") || field.name_eq("transfer-encoding")
}
fn protocol(version: HttpLegVersion) -> &'static str {
    match version {
        HttpLegVersion::Http1 => "HTTP/1.1",
        HttpLegVersion::Http2 => "HTTP/2",
        HttpLegVersion::Http3 => "HTTP/3",
    }
}
fn url(head: &RequestHead) -> String {
    format!(
        "{}://{}{}{}",
        head.target.scheme,
        head.target.authority,
        head.target.path,
        head.target
            .query
            .as_ref()
            .map_or_else(String::new, |query| format!("?{query}"))
    )
}
fn text(bytes: &[u8]) -> Result<&str, AppError> {
    std::str::from_utf8(bytes).map_err(|_| unavailable("This header contains non-text bytes and cannot be represented faithfully as clipboard text"))
}
fn invalid(message: &str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}
fn unavailable(message: &str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, false)
}
fn append_headers(out: &mut String, block: &HeaderBlock) -> Result<(), AppError> {
    for field in block.iter() {
        write!(
            out,
            "\r\n{}: {}",
            text(field.name())?,
            if field.is_redacted() {
                "[redacted]"
            } else {
                text(field.value())?
            }
        )
        .expect("string write");
    }
    Ok(())
}
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn curl(
    head: &RequestHead,
    headers: &[(String, String)],
    body: &CommandBody<'_>,
) -> Result<String, AppError> {
    let mut parts = vec![
        "curl --disable --globoff --path-as-is".into(),
        format!("--request {}", sh_quote(&head.method)),
    ];
    for (name, value) in curl_headers(headers) {
        parts.push(format!(
            "--header {}",
            sh_quote(&curl_header(&name, &value, headers))
        ));
    }
    match body {
        CommandBody::Bytes(_) => return Err(invalid("Binary cURL bodies require a file")),
        CommandBody::Empty => {}
        CommandBody::Text(value) => parts.push(format!(
            "{} {}",
            if value.starts_with('@') {
                "--data-raw"
            } else {
                "--data-binary"
            },
            sh_quote(value)
        )),
        CommandBody::File(path) => {
            parts.push(format!("--data-binary {}", sh_quote(&format!("@{path}"))));
        }
    }
    parts.push(format!("--url {}", sh_quote(&url(head))));
    Ok(parts.join(" \\\n  "))
}

fn curl_header(name: &str, value: &str, headers: &[(String, String)]) -> String {
    if value.is_empty()
        && headers
            .iter()
            .any(|(field, _)| field.eq_ignore_ascii_case(name))
    {
        format!("{name};")
    } else {
        format!("{name}: {value}")
    }
}

fn curl_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut result = headers.to_vec();
    for name in ["User-Agent", "Accept", "Content-Type"] {
        if !headers
            .iter()
            .any(|(field, _)| field.eq_ignore_ascii_case(name))
        {
            result.push((name.into(), String::new()));
        }
    }
    result
}

fn config_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn curl_windows(
    head: &RequestHead,
    headers: &[(String, String)],
    body: &CommandBody<'_>,
) -> Result<String, AppError> {
    let mut config = format!(
        "globoff\npath-as-is\nrequest = {}\nurl = {}\n",
        config_quote(&head.method),
        config_quote(&url(head))
    );
    for (name, value) in curl_headers(headers) {
        writeln!(
            config,
            "header = {}",
            config_quote(&curl_header(&name, &value, headers))
        )
        .expect("string write");
    }
    match body {
        CommandBody::Bytes(_) => return Err(invalid("Binary cURL bodies require a file")),
        CommandBody::Empty => {}
        CommandBody::Text(value) => {
            writeln!(
                config,
                "{} = {}",
                if value.starts_with('@') {
                    "data-raw"
                } else {
                    "data-binary"
                },
                config_quote(value)
            )
            .expect("string write");
        }
        CommandBody::File(path) => {
            writeln!(
                config,
                "data-binary = {}",
                config_quote(&format!("@{path}"))
            )
            .expect("string write");
        }
    }
    // Base64 data is decoded into a PowerShell string; hostile traffic can never
    // terminate a literal or become shell syntax. OutputEncoding handles 5.1.
    Ok(format!(
        "$curlConfig = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))\r\n$previousOutputEncoding = $OutputEncoding\r\ntry {{\r\n  $OutputEncoding = New-Object Text.UTF8Encoding($false)\r\n  $curlConfig | & curl.exe --disable --config -\r\n}} finally {{ $OutputEncoding = $previousOutputEncoding }}",
        STANDARD.encode(config)
    ))
}

fn powershell(head: &RequestHead, headers: &[(String, String)], body: &CommandBody<'_>) -> String {
    let mut out = format!(
        "Add-Type -AssemblyName System.Net.Http\r\n$handler = New-Object System.Net.Http.HttpClientHandler\r\n$handler.AllowAutoRedirect = $false\r\n$handler.UseCookies = $false\r\n$handler.UseDefaultCredentials = $false\r\n$handler.AutomaticDecompression = [Net.DecompressionMethods]::None\r\n$client = New-Object System.Net.Http.HttpClient($handler)\r\n$request = New-Object System.Net.Http.HttpRequestMessage\r\n$request.Method = New-Object System.Net.Http.HttpMethod({})\r\n$request.RequestUri = {}\r\n$request.Version = [Version]'1.1'\r\ntry {{\r\n",
        ps_quote(&head.method),
        ps_quote(&url(head))
    );
    let expression = match body {
        CommandBody::Empty => "[byte[]]@()".into(),
        CommandBody::Bytes(value) => {
            format!("[Convert]::FromBase64String('{}')", STANDARD.encode(value))
        }
        CommandBody::Text(value) => format!(
            "[Convert]::FromBase64String('{}')",
            STANDARD.encode(value.as_bytes())
        ),
        CommandBody::File(path) => format!("[IO.File]::ReadAllBytes({})", ps_quote(path)),
    };
    // A content object also accommodates Content-Type on an empty request.
    writeln!(out, "  $bodyBytes = {expression}\r\n  $request.Content = New-Object System.Net.Http.ByteArrayContent(,$bodyBytes)").expect("string write");
    for (name, value) in headers {
        let (name, value) = (ps_quote(name), ps_quote(value));
        writeln!(out, "  if (!$request.Headers.TryAddWithoutValidation({name}, {value})) {{\r\n    if (!$request.Content.Headers.TryAddWithoutValidation({name}, {value})) {{ throw 'A captured header could not be added' }}\r\n  }}").expect("string write");
    }
    out.push_str("  $response = $client.SendAsync($request).GetAwaiter().GetResult()\r\n  $response\r\n  $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()\r\n} finally {\r\n  if ($response) { $response.Dispose() }\r\n  $request.Dispose()\r\n  $client.Dispose()\r\n  $handler.Dispose()\r\n}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use transmog_core::{HeaderField, Target};

    fn head() -> RequestHead {
        RequestHead {
            method: "POST".into(),
            target: Target {
                scheme: "https".into(),
                authority: "example.invalid".into(),
                host: "example.invalid".into(),
                port: 443,
                path: "/path".into(),
                query: Some("q=$('hostile')&x=[1]".into()),
            },
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("X-Test", "one").unwrap(),
                HeaderField::try_new("X-Test", "two'$(never)").unwrap(),
            ]),
            source_version: HttpLegVersion::Http1,
        }
    }

    #[test]
    fn shell_literals_keep_hostile_text_and_duplicates_inert() {
        let head = head();
        let result = generate(
            &head,
            RequestCommandFormat::Curl,
            CommandBody::Text("a'$(never)\nnext"),
            true,
        )
        .unwrap();
        assert!(result.text.contains("'a'\"'\"'$(never)\nnext'"));
        assert_eq!(result.text.matches("--header 'X-Test:").count(), 2);
        assert!(result.text.contains("--globoff --path-as-is"));
        assert!(!result.text.contains("--location"));
    }

    #[test]
    fn redaction_has_explicit_placeholders_and_never_leaks_values() {
        let mut head = head();
        head.headers
            .push(HeaderField::try_new("Authorization", "Bearer secret").unwrap());
        head.headers.redact_sensitive();
        let result = generate(
            &head,
            RequestCommandFormat::Curl,
            CommandBody::File("request-body.bin"),
            false,
        )
        .unwrap();
        assert!(result.body_file_required);
        assert!(!result.body_file_available);
        assert!(result.text.contains("REPLACE_REDACTED_VALUE"));
        assert!(!result.text.contains("secret"));
        assert!(!result.notices.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_config_transports_utf8_and_quotes_without_native_argument_round_trips() {
        let head = head();
        let result = generate(
            &head,
            RequestCommandFormat::CurlWindows,
            CommandBody::Text("é\n\"test\""),
            true,
        )
        .unwrap();
        let encoded = result
            .text
            .split("FromBase64String('")
            .nth(1)
            .unwrap()
            .split('\'')
            .next()
            .unwrap();
        let config = String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(config.contains("data-binary = \"é\\n\\\"test\\\"\""));
        assert!(config.contains("X-Test: two'$(never)"));
        assert!(result.text.contains("curl.exe --disable --config -"));
        assert!(!result.text.contains("$(never)"));
    }

    #[cfg(windows)]
    #[test]
    fn powershell_works_without_version_seven_syntax_or_ambient_credentials() {
        let result = generate(
            &head(),
            RequestCommandFormat::Powershell,
            CommandBody::File("C:\\captured body's\\é.bin"),
            true,
        )
        .unwrap();
        assert!(
            result
                .text
                .contains("[IO.File]::ReadAllBytes('C:\\captured body''s\\é.bin')")
        );
        assert!(result.text.contains("$handler.UseCookies = $false"));
        assert!(result.text.contains("$handler.AllowAutoRedirect = $false"));
        assert!(!result.text.contains("SkipCertificateCheck"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One complete exchange checks shared original evidence end to end.
    async fn full_headers_binary_body_and_composer_share_original_evidence() {
        use std::sync::Arc;
        use transmog_core::{
            ConnectionId, ResponseHead, SessionId, SessionMetadata, StreamId,
            intercept::{CompletedExchange, ExchangeMetadata},
            observe::{ObservedBodyChunk, Observer, ObserverEvent, ObserverEventKind},
        };
        let root = tempfile::tempdir().unwrap();
        let service =
            ApplicationSessionService::new(transmog_session::ServiceConfig::default()).unwrap();
        let store = BodyStore::new(crate::BodyStoreConfig::product_default(
            root.path().join("bodies"),
        ))
        .unwrap();
        let mut head = head();
        head.headers
            .push(HeaderField::try_new("Cookie", vec![b'a'; 5000]).unwrap());
        head.headers
            .push(HeaderField::try_new("Content-Encoding", "gzip").unwrap());
        let response = ResponseHead {
            status: 404,
            source_version: HttpLegVersion::Http1,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("X-Response", "one").unwrap(),
                HeaderField::try_new("X-Response", "two").unwrap(),
            ]),
        };
        let metadata = Arc::new(ExchangeMetadata::from_session(
            &SessionMetadata {
                session_id: SessionId(1),
                downstream_connection_id: ConnectionId(1),
                stream_id: StreamId(1),
                client_addr: "192.0.2.1:1000".parse().unwrap(),
                client_identity: transmog_core::ClientIdentity::default(),
                proxy_addr: "127.0.0.1:8888".parse().unwrap(),
                ingress_version: HttpLegVersion::Http1,
                egress_version: Some(HttpLegVersion::Http1),
            },
            head.target.clone(),
        ));
        let bytes = b"\x1f\x8b\x00\xffraw\r\n";
        let kinds = vec![
            ObserverEventKind::ExchangeStarted {
                metadata: metadata.clone(),
            },
            ObserverEventKind::RequestHeadObserved {
                boundary: ExchangeBoundary::ClientRequest,
                head: head.clone(),
            },
            ObserverEventKind::ResponseHeadObserved {
                boundary: ExchangeBoundary::ClientResponse,
                head: response.clone(),
            },
            ObserverEventKind::BodyChunk(ObservedBodyChunk {
                boundary: ExchangeBoundary::ClientRequest,
                byte_count: bytes.len(),
                sample: Some(bytes::Bytes::copy_from_slice(bytes)),
                truncated: false,
            }),
            ObserverEventKind::Completed(CompletedExchange {
                metadata,
                request_head: head,
                response_head: response,
            }),
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let event = ObserverEvent {
                exchange_id: ExchangeId(1),
                sequence: index as u64 + 1,
                kind,
            };
            service.catalog().apply(event.clone());
            store.on_event(event).await.unwrap();
        }
        let id = "00000000000000000000000000000001";
        let headers = copy_all_headers(&service, id).unwrap();
        assert!(
            headers
                .starts_with("POST https://example.invalid/path?q=$('hostile')&x=[1] HTTP/1.1\r\n")
        );
        assert!(headers.contains(&format!("Cookie: {}", "a".repeat(5000))));
        assert!(headers.contains("Content-Encoding: gzip\r\n\r\n\r\nHTTP/1.1 404 Not Found"));
        assert!(headers.ends_with("X-Response: one\r\nX-Response: two"));
        let generated = command(&service, Some(&store), id, RequestCommandFormat::Curl).unwrap();
        assert!(generated.body_file_required && generated.body_file_available);
        #[cfg(windows)]
        {
            let generated =
                command(&service, Some(&store), id, RequestCommandFormat::Powershell).unwrap();
            assert!(!generated.body_file_required);
            assert!(generated.text.contains(&STANDARD.encode(bytes)));
        }
        let prepared = prepare_file(&service, Some(&store), id).unwrap();
        let path = root.path().join("captured body.bin");
        prepared.save_to(path.clone()).await.unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        let source = composer_source(&service, Some(&store), id).unwrap();
        assert!(source.body_available);
        assert_eq!(source.body, "1f8b00ff7261770d0a");
        assert!(
            source
                .headers
                .iter()
                .any(|header| header.name == "Content-Encoding" && header.value == "gzip")
        );
        assert_eq!(
            store
                .metadata(ExchangeId(1))
                .into_iter()
                .find(|body| body.boundary == "client-request")
                .unwrap()
                .content_codings,
            ["gzip"]
        );
        drop(store);
    }

    #[cfg(windows)]
    #[test]
    fn generated_windows_scripts_parse_in_both_powershell_versions_without_execution() {
        let directory = tempfile::tempdir().unwrap();
        for format in [
            RequestCommandFormat::CurlWindows,
            RequestCommandFormat::Powershell,
        ] {
            let generated =
                generate(&head(), format, CommandBody::File("C:\\body's é.bin"), true).unwrap();
            let path = directory.path().join("parse-only.ps1");
            // A BOM lets Windows PowerShell read the UTF-8 fixture correctly.
            std::fs::write(&path, format!("\u{feff}{}", generated.text)).unwrap();
            for shell in ["powershell.exe", "pwsh.exe"] {
                let output = std::process::Command::new(shell)
                    .args(["-NoProfile", "-NonInteractive", "-Command", "$tokens=$null; $parseErrors=$null; $null=[System.Management.Automation.Language.Parser]::ParseFile($env:TRANSMOG_PARSE_SCRIPT,[ref]$tokens,[ref]$parseErrors); if($parseErrors.Count) { $parseErrors | ForEach-Object { $_.Message }; exit 1 }"])
                    .env("TRANSMOG_PARSE_SCRIPT", &path).output().unwrap();
                assert!(
                    output.status.success(),
                    "{shell} rejected generated syntax: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
        }
    }
}
