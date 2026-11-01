//! Development proof only. The narrow shipping debugger is a later phase.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::{Read, Write},
    rc::Rc,
};

use serde_json::{Value, json};
use v8::inspector::{
    Channel, ChannelImpl, StringBuffer, StringView, V8Inspector, V8InspectorClient,
    V8InspectorClientImpl, V8InspectorClientTrustLevel, V8InspectorSession,
};

use crate::Runtime;

const FRAME_BYTES: usize = 64 * 1024;
const FIXTURE: &str = "export function inner(input) {\n  const local = input + 4;\n  const object = {get secret() {globalThis.getterCalls++; throw new Error('getter ran');}};\n  const result = local * 2;\n  return result;\n}\nexport function outer(value) {\n  return inner(value);\n}\nglobalThis.entry = outer;\nglobalThis.getterCalls = 0;\n";

struct Client {
    session: RefCell<Option<Rc<V8InspectorSession>>>,
    paused: Cell<bool>,
    stopped: Cell<bool>,
    isolate: v8::IsolateHandle,
}

impl Client {
    fn stop(&self) {
        self.stopped.set(true);
        self.paused.set(false);
        self.isolate.terminate_execution();
    }

    fn dispatch(&self, command: &Value) -> Result<(), String> {
        let method = command["method"].as_str().ok_or("Missing method")?;
        if command["params"].get("condition").is_some()
            || command["params"]["generatePreview"] == true
        {
            return Err("Probe evaluation and custom previews are unsupported".to_owned());
        }
        // Even the spike does not accept expression evaluation, mutation, live
        // editing, or an arbitrary command protocol from its controller.
        if !matches!(
            method,
            "Debugger.enable"
                | "Debugger.setBreakpointByUrl"
                | "Debugger.removeBreakpoint"
                | "Debugger.setPauseOnExceptions"
                | "Debugger.resume"
                | "Debugger.stepInto"
                | "Debugger.stepOver"
                | "Debugger.stepOut"
                | "Runtime.getProperties"
                | "Runtime.releaseObjectGroup"
        ) {
            return Err("Unsupported probe command".to_owned());
        }
        let session = self.session.borrow().clone().ok_or("No session")?;
        let message = serde_json::to_vec(command).map_err(|error| error.to_string())?;
        session.dispatch_protocol_message(StringView::from(message.as_slice()));
        Ok(())
    }
}

struct InspectorClient(Rc<Client>);

impl V8InspectorClientImpl for InspectorClient {
    fn run_message_loop_on_pause(&self, _context_group_id: i32) {
        let client = &self.0;
        client.paused.set(true);
        while client.paused.get() {
            let Ok(command) = read() else {
                client.stop();
                break;
            };
            if command["probe"] == "stop" {
                client.stop();
                break;
            }
            if let Err(error) = client.dispatch(&command)
                && send(&json!({"id":command["id"], "error":{"message":error}})).is_err()
            {
                client.stop();
            }
        }
    }

    fn quit_message_loop_on_pause(&self) {
        self.0.paused.set(false);
    }
}

struct Output;

struct Connection {
    inspector: V8Inspector,
    client: Rc<Client>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // The session points into Inspector, including on early error returns.
        self.client.session.borrow_mut().take();
    }
}

impl ChannelImpl for Output {
    fn send_response(&self, _call_id: i32, message: v8::UniquePtr<StringBuffer>) {
        output(message);
    }
    fn send_notification(&self, message: v8::UniquePtr<StringBuffer>) {
        output(message);
    }
    fn flush_protocol_notifications(&self) {}
}

fn output(message: v8::UniquePtr<StringBuffer>) {
    let message = message.unwrap();
    if message.string().len() > FRAME_BYTES / 3 {
        std::process::exit(74);
    }
    let text = message.string().to_string();
    // A failed controller pipe ends this owned development worker immediately.
    if send_bytes(text.as_bytes()).is_err() {
        std::process::exit(74);
    }
}

fn read() -> Result<Value, String> {
    let mut input = std::io::stdin().lock();
    let mut length = [0; 4];
    input
        .read_exact(&mut length)
        .map_err(|error| error.to_string())?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > FRAME_BYTES {
        return Err("Invalid probe frame size".to_owned());
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn send(value: &Value) -> Result<(), String> {
    send_bytes(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
}

fn send_bytes(bytes: &[u8]) -> Result<(), String> {
    let length = u32::try_from(bytes.len()).map_err(|error| error.to_string())?;
    if bytes.len() > FRAME_BYTES {
        return Err("Probe output exceeds its limit".to_owned());
    }
    let mut output = std::io::stdout().lock();
    output
        .write_all(&length.to_le_bytes())
        .map_err(|error| error.to_string())?;
    output.write_all(bytes).map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}

fn sample(runtime: &mut Runtime) -> Result<u128, String> {
    for _ in 0..100 {
        runtime
            .execute_string("transmog:benchmark", "String(1 + 2)", 32)
            .map_err(|error| error.to_string())?;
    }
    let mut batches = Vec::new();
    for _ in 0..11 {
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            runtime
                .execute_string("transmog:benchmark", "String(1 + 2)", 32)
                .map_err(|error| error.to_string())?;
        }
        batches.push(start.elapsed().as_nanos() / 1000);
    }
    batches.sort_unstable();
    Ok(batches[5])
}

/// Serves the fixed fixture proof over inherited bounded native pipes.
///
/// # Errors
/// Returns framing, engine, or fixture failures. The parent owns a hard deadline.
pub fn run() -> Result<(), String> {
    let mut runtime =
        Runtime::new(1024 * 1024, 32 * 1024 * 1024).map_err(|error| error.to_string())?;
    let client = Rc::new(Client {
        session: RefCell::new(None),
        paused: Cell::new(false),
        stopped: Cell::new(false),
        isolate: runtime.isolate.thread_safe_handle(),
    });
    let connection = Connection {
        inspector: V8Inspector::create(
            &mut runtime.isolate,
            V8InspectorClient::new(Box::new(InspectorClient(Rc::clone(&client)))),
        ),
        client: Rc::clone(&client),
    };
    let inspector = &connection.inspector;
    {
        v8::scope!(let scope, &mut runtime.isolate);
        let context = v8::Local::new(scope, &runtime.context);
        inspector.context_created(
            context,
            1,
            StringView::from(&b"Transmog probe"[..]),
            StringView::from(&b"{}"[..]),
        );
    }
    *client.session.borrow_mut() = Some(Rc::new(inspector.connect(
        1,
        Channel::new(Box::new(Output)),
        StringView::from(&b"{}"[..]),
        V8InspectorClientTrustLevel::Untrusted,
    )));
    send(&json!({"event":"ready", "sandboxed":true}))?;
    while !client.stopped.get() {
        let Ok(command) = read() else { break };
        match command["probe"].as_str() {
            Some("benchmark") => {
                let disabled_ns = {
                    let mut baseline = Runtime::new(1024 * 1024, 32 * 1024 * 1024)
                        .map_err(|error| error.to_string())?;
                    sample(&mut baseline)?
                };
                let attached_ns = sample(&mut runtime)?;
                send(
                    &json!({"event":"benchmark", "disabledNs":disabled_ns, "attachedNs":attached_ns}),
                )?;
            }
            Some("load") => {
                let entry = url::Url::parse("file:///transmog-script/main.js").unwrap();
                runtime
                    .evaluate_modules(&entry, BTreeMap::from([(entry.clone(), FIXTURE.into())]))
                    .map_err(|error| error.to_string())?;
                send(&json!({"event":"loaded"}))?;
            }
            Some("invoke") => {
                let result = runtime.execute_string(
                    "transmog:probe-invoke",
                    "`${entry(3)}:${getterCalls}`",
                    100,
                );
                if !client.stopped.get() {
                    send(
                        &json!({"event":"completed", "value":result.map_err(|error|error.to_string())}),
                    )?;
                }
            }
            Some("stop") => break,
            _ => {
                if let Err(error) = client.dispatch(&command) {
                    send(&json!({"id":command["id"], "error":{"message":error}}))?;
                }
            }
        }
    }
    // Break the client/session cycle and release Inspector handles while the
    // production runtime's context and isolate are still alive.
    client.session.borrow_mut().take();
    {
        v8::scope!(let scope, &mut runtime.isolate);
        inspector.context_destroyed(v8::Local::new(scope, &runtime.context));
    }
    drop(connection);
    Ok(())
}
