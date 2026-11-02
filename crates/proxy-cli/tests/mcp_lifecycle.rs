//! Real OS pipes catch shutdown hangs that in-memory MCP transports cannot.
use std::{
    io::{BufRead as _, BufReader, Write as _},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct OwnedConnector(Child);
impl Drop for OwnedConnector {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

impl OwnedConnector {
    fn start(root: &std::path::Path, key: &str) -> Self {
        Self::with_log_filter(root, key, "transmog=info")
    }

    fn with_log_filter(root: &std::path::Path, key: &str, filter: &str) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_transmog-cli"))
                .args(["mcp", "serve"])
                .env("TRANSMOG_ACCESS_KEY", key)
                .env("RUST_LOG", filter)
                .env("LOCALAPPDATA", root)
                .env("XDG_STATE_HOME", root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn send(&mut self, message: &serde_json::Value) {
        let pipe = self.0.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *pipe, message).unwrap();
        pipe.write_all(b"\n").unwrap();
        pipe.flush().unwrap();
    }

    fn initialize(&mut self) -> BufReader<std::process::ChildStdout> {
        let mut reader = BufReader::new(self.0.stdout.take().unwrap());
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let _ = tx.send((reader, line));
        });
        self.send(
            &serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                "protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"lifecycle-test", "version":"1"}
            }}),
        );
        let (reader, line) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        thread.join().unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response.get("result").is_some(), "{response}");
        self.send(&serde_json::json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
        reader
    }

    fn assert_clean_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(Instant::now() < deadline, "MCP connector did not terminate");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn undrained_stderr_cannot_block_protocol_or_eof_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let mut connector = OwnedConnector::with_log_filter(root.path(), "", "trace");
    let mut output = connector.initialize();
    // Responses continue even after verbose logs fill the undrained stderr
    // pipe. Bound each read so the regression itself cannot hang the suite.
    for identity in 2..102 {
        connector.send(
            &serde_json::json!({"jsonrpc":"2.0", "id":identity, "method":"ping", "params":{}}),
        );
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut line = String::new();
            output.read_line(&mut line).unwrap();
            let _ = tx.send((output, line));
        });
        let (next, line) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        thread.join().unwrap();
        output = next;
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], identity);
    }
    drop(connector.0.stdin.take());
    connector.assert_clean_exit();
}

#[test]
fn stdin_eof_terminates_before_and_after_initialization() {
    let root = tempfile::tempdir().unwrap();
    for initialized in [false, true] {
        let mut connector = OwnedConnector::start(root.path(), "");
        let _output = initialized.then(|| connector.initialize());
        drop(connector.0.stdin.take());
        connector.assert_clean_exit();
    }
}

#[test]
fn failed_response_write_terminates_even_while_stdin_is_held_open() {
    let root = tempfile::tempdir().unwrap();
    let mut connector = OwnedConnector::start(root.path(), "");
    let output = connector.initialize();
    drop(output);
    connector
        .send(&serde_json::json!({"jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}}));
    // Keep stdin alive until after the process exits. This reproduced a hang
    // both in SDK response handling and runtime shutdown on Windows.
    connector.assert_clean_exit();
}

struct StalledOperation(std::sync::Arc<tokio::sync::Notify>);
impl transmog_mcp::Handler for StalledOperation {
    fn call(
        &self,
        _grant: transmog_mcp::Grant,
        _operation: String,
        _input: serde_json::Value,
    ) -> transmog_mcp::CallFuture<'_> {
        self.0.notify_one();
        Box::pin(std::future::pending())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_eof_bounds_shutdown_even_with_an_inflight_desktop_operation() {
    use std::sync::Arc;
    use transmog_mcp::{AccessManager, GrantInput, LocalServer, Permission, Scope};
    let root = tempfile::tempdir().unwrap();
    let access = AccessManager::open(None).unwrap();
    let (_, key) = access
        .create(GrantInput {
            name: "Owned lifecycle test".into(),
            permissions: [Permission::Inspect].into(),
            scope: Scope::All,
            expires_at: None,
            omit_sensitive_values: false,
        })
        .unwrap();
    access.set_enabled(true).unwrap();
    let admitted = Arc::new(tokio::sync::Notify::new());
    let _server = LocalServer::start(
        root.path().join("Transmog"),
        access,
        Arc::new(StalledOperation(admitted.clone())),
    )
    .await
    .unwrap();
    let mut connector = OwnedConnector::start(root.path(), key.expose());
    let _output = connector.initialize();
    connector.send(&serde_json::json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"transmog_status", "arguments":{}}}));
    tokio::time::timeout(Duration::from_secs(5), admitted.notified())
        .await
        .unwrap();
    drop(connector.0.stdin.take());
    connector.assert_clean_exit();
}
