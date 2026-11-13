//! Native console snapshots never call authored accessors or string conversion.

use super::{Error, ErrorKind, state};
use transmog_script_diagnostics::{
    ConsoleEvent, LogLevel, LogRecord, MAX_LOG_BYTES, MAX_LOG_FRAMES, MAX_LOG_RECORD_BYTES,
    MAX_LOG_RECORDS, MAX_LOG_TOTAL_FRAMES,
};

type Sink = Box<dyn Fn(u64, ConsoleEvent) -> bool>;
#[derive(Default)]
pub(super) struct Console {
    pub(super) snapshots: bool,
    sink: Option<Sink>,
    invocation: u64,
    sequence: u64,
    accepted: usize,
    bytes: usize,
    dropped: u64,
    frames: usize,
}
impl Console {
    #[cfg(feature = "debugger")]
    pub(super) fn capture(sink: impl Fn(u64, ConsoleEvent) -> bool + 'static) -> Self {
        Self {
            snapshots: true,
            sink: Some(Box::new(sink)),
            ..Self::default()
        }
    }
    pub(super) fn begin(&mut self, invocation: u64) {
        self.invocation = invocation;
        self.sequence = 0;
        self.accepted = 0;
        self.bytes = 0;
        self.dropped = 0;
        self.frames = 0;
    }
    pub(super) fn finish(&self) {
        // An authenticated ordinary result is the completion barrier for a stream
        // with no console calls. Avoid a separate IPC frame on the common path.
        if self.sequence == 0 {
            return;
        }
        if let Some(sink) = &self.sink {
            sink(
                self.invocation,
                ConsoleEvent::End {
                    dropped: self.dropped,
                },
            );
        }
    }
}

pub(super) fn install(scope: &mut v8::PinScope<'_, '_>, sink: Sink) -> Result<(), Error> {
    let object = v8::Object::new(scope);
    for (name, level) in [
        ("log", 0),
        ("info", 0),
        ("debug", 1),
        ("warn", 2),
        ("error", 3),
        ("trace", 4),
    ] {
        let key = v8::String::new(scope, name).ok_or_else(unavailable)?;
        let data = v8::Integer::new(scope, level);
        let function = v8::Function::builder(log)
            .data(data.into())
            .build(scope)
            .ok_or_else(unavailable)?;
        if object.define_own_property(
            scope,
            key.into(),
            function.into(),
            v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_DELETE,
        ) != Some(true)
        {
            return Err(unavailable());
        }
    }
    let key = v8::String::new(scope, "console").ok_or_else(unavailable)?;
    let global = scope.get_current_context().global(scope);
    if global.define_own_property(
        scope,
        key.into(),
        object.into(),
        v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_DELETE,
    ) != Some(true)
    {
        return Err(unavailable());
    }
    state(scope).console.borrow_mut().sink = Some(sink);
    Ok(())
}
fn unavailable() -> Error {
    Error::new(
        ErrorKind::Initialization,
        "Native console could not initialize",
    )
}

#[allow(clippy::needless_pass_by_value)] // Native V8 callback ABI.
fn log<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    _result: v8::ReturnValue,
) {
    let state = state(scope);
    let mut console = state.console.borrow_mut();
    console.sequence = console.sequence.saturating_add(1);
    if console.sink.is_none()
        || console.accepted >= MAX_LOG_RECORDS
        || console.bytes >= MAX_LOG_BYTES
    {
        console.dropped = console.dropped.saturating_add(1);
        return;
    }
    let level = match args.data().int32_value(scope) {
        Some(1) => LogLevel::Debug,
        Some(2) => LogLevel::Warn,
        Some(3) => LogLevel::Error,
        Some(4) => LogLevel::Trace,
        _ => LogLevel::Info,
    };
    let limit = MAX_LOG_RECORD_BYTES.min(MAX_LOG_BYTES - console.bytes);
    let mut text = String::new();
    let mut truncated = args.length() > 16;
    for index in 0..args.length().min(16) {
        if text.len() >= limit {
            truncated = true;
            break;
        }
        if index > 0 {
            text.push(' ');
        }
        let value = args.get(index);
        if value.is_string() || value.is_big_int() {
            // Primitive-only conversion cannot invoke user methods. Objects,
            // functions, proxies and symbols never use Value::to_string.
            if let Some(value) = value.to_string(scope) {
                let mut buffer = [0; MAX_LOG_RECORD_BYTES];
                let count = value.write_utf8_v2(
                    scope,
                    &mut buffer[..limit - text.len()],
                    v8::WriteFlags::kReplaceInvalidUtf8,
                    None,
                );
                truncated |= count < value.utf8_length(scope);
                text.push_str(&String::from_utf8_lossy(&buffer[..count]));
            }
        } else {
            let label = inert_label(scope, value);
            let count = label.len().min(limit - text.len());
            text.push_str(&label[..count]);
            truncated |= count < label.len();
        }
    }
    let (frames, trace_truncated) =
        if level == LogLevel::Trace && console.frames < MAX_LOG_TOTAL_FRAMES {
            super::exception::current_frames(
                scope,
                limit - text.len(),
                MAX_LOG_FRAMES.min(MAX_LOG_TOTAL_FRAMES - console.frames),
            )
        } else if level == LogLevel::Trace {
            (vec![], true)
        } else {
            (vec![], false)
        };
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut record = LogRecord {
        values: vec![],
        sequence: console.sequence,
        time_unix_ms: u64::try_from(time).unwrap_or(u64::MAX),
        level,
        text,
        frames,
        truncated: truncated || trace_truncated,
    };
    if console.snapshots {
        record.values = super::console_snapshot::capture(
            scope,
            &args,
            limit.saturating_sub(record.text_bytes()),
        );
    }
    let bytes = record.text_bytes();
    let frames = record.frames.len();
    if (console.sink.as_ref().expect("checked sink"))(
        console.invocation,
        ConsoleEvent::Record(record),
    ) {
        console.accepted += 1;
        console.bytes += bytes;
        console.frames += frames;
    } else {
        console.dropped = console.dropped.saturating_add(1);
    }
}

pub(super) fn number_label(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<v8::Value>,
) -> String {
    if value
        .number_value(scope)
        .is_some_and(|number| number.to_bits() == (-0.0_f64).to_bits())
    {
        return "-0".into();
    }
    value
        .to_string(scope)
        .map_or_default(|text| text.to_rust_string_lossy(scope))
}

fn inert_label(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> String {
    if value.is_proxy() {
        "[Proxy: unavailable]".into()
    } else if value.is_null() {
        "null".into()
    } else if value.is_undefined() {
        "undefined".into()
    } else if value.is_boolean() {
        if value.is_true() {
            "true".into()
        } else {
            "false".into()
        }
    } else if value.is_number() {
        number_label(scope, value)
    } else if value.is_symbol() {
        "[Symbol]".into()
    } else if value.is_function() {
        "[Function]".into()
    } else if value.is_array() {
        "[Array]".into()
    } else if value.is_array_buffer_view() {
        "[TypedArray or DataView]".into()
    } else {
        "[Object]".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exception_metadata_keeps_primitive_messages_and_does_not_walk_proxy_prototypes() {
        let mut runtime = crate::Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        let error=runtime.execute_script("file:///message.js",r"globalThis.touches=0;const error=new Error();error.message=42;Object.defineProperty(error,'name',{get(){touches++;throw 0;}});throw error;").unwrap_err();
        let details = error.exception.unwrap();
        assert_eq!(details.exception.message, "42");
        assert!(details.exception.name.is_none());
        assert!(details.exception.unavailable);
        let error=runtime.execute_script("file:///prototype.js",r"const value=Object.create(new Proxy({}, {get(){touches++;throw 0;},getPrototypeOf(){touches++;throw 0;}}));value.message='safe';throw value;").unwrap_err();
        let details = error.exception.unwrap();
        assert_eq!(details.exception.message, "safe");
        assert!(details.exception.unavailable);
        assert_eq!(
            runtime
                .execute_string("transmog:assert", "String(touches)", 32)
                .unwrap(),
            "0"
        );
    }
    use std::{cell::RefCell, rc::Rc};
    #[test]
    fn primitive_output_trace_bounds_and_hostile_objects_are_inert() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let mut runtime = crate::Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        runtime
            .enable_console(move |id, event| {
                assert_eq!(id, 7);
                sink.borrow_mut().push(event);
                true
            })
            .unwrap();
        runtime.begin_console(7);
        runtime.execute_script("file:///console-test.js",r"
            let calls=0;
            const hostile=new Proxy({}, {ownKeys(){calls++;throw 0;},get(){calls++;throw 0;}});
            console.log('hello', 7, null, true, 10n, Symbol('s'), hostile, {toString(){calls++;throw 0;},toJSON(){calls++;throw 0;}});
            console.trace('trace');
            for(let index=0;index<40;index++) console.trace('bounded frames');
            for(let index=0;index<200;index++) console.warn('😀'.repeat(4000));
            if(calls) throw new Error('conversion invoked');
        ").unwrap();
        runtime.finish_console();
        let events = events.borrow();
        let records = events
            .iter()
            .filter_map(|event| {
                if let ConsoleEvent::Record(record) = event {
                    Some(record)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert!(records.iter().all(|record| record.is_bounded()));
        assert!(
            records
                .iter()
                .map(|record| record.text_bytes())
                .sum::<usize>()
                <= MAX_LOG_BYTES
        );
        assert!(
            records[0]
                .text
                .starts_with("hello 7 null true 10 [Symbol] [Proxy: unavailable] [Object]")
        );
        assert_eq!(
            records[1].frames[0].generated.as_ref().unwrap().source,
            "file:///console-test.js"
        );
        assert!(matches!(events.last(),Some(ConsoleEvent::End{dropped}) if *dropped>0));
    }
}
