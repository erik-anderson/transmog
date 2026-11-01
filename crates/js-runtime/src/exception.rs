//! Capture engine-owned evidence without invoking script accessors or conversion.

use transmog_script_diagnostics::{
    ExceptionDiagnostic, ExceptionValue, MAX_EXCEPTION_CAUSES, MAX_EXCEPTION_FRAMES,
    MAX_EXCEPTION_TEXT_BYTES, SourceLocation, StackFrame,
};

struct Budget {
    bytes: usize,
    frames: usize,
    truncated: bool,
    unavailable: bool,
}

impl Budget {
    fn text(&mut self, scope: &v8::Isolate, value: v8::Local<v8::String>) -> String {
        let limit = self.bytes.min(2048);
        let mut buffer = [0; 2048];
        let written = value.write_utf8_v2(
            scope,
            &mut buffer[..limit],
            v8::WriteFlags::kReplaceInvalidUtf8,
            None,
        );
        self.truncated |= value.utf8_length(scope) > written;
        let text = String::from_utf8_lossy(&buffer[..written]).into_owned();
        self.bytes = self.bytes.saturating_sub(text.len());
        text
    }
}

/// Reads an own data descriptor only. Descriptors returned by V8 can inherit an
/// authored Object.prototype, so check for an own `value` before reading it.
fn data<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    key: &str,
) -> Result<Option<v8::Local<'s, v8::Value>>, ()> {
    if value.is_proxy() {
        return Err(());
    }
    let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
        return Ok(None);
    };
    let key = v8::String::new(scope, key).ok_or(())?;
    let descriptor = object
        .get_own_property_descriptor(scope, key.into())
        .ok_or(())?;
    if descriptor.is_undefined() {
        return Ok(None);
    }
    let descriptor = v8::Local::<v8::Object>::try_from(descriptor).map_err(|_| ())?;
    let key = v8::String::new(scope, "value").ok_or(())?;
    if descriptor.has_own_property(scope, key.into()) != Some(true) {
        return Err(());
    }
    descriptor.get(scope, key.into()).map(Some).ok_or(())
}

fn read_data<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    key: &str,
    budget: &mut Budget,
) -> Option<v8::Local<'s, v8::Value>> {
    if let Ok(value) = data(scope, value, key) {
        value
    } else {
        budget.unavailable = true;
        None
    }
}

fn name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    budget: &mut Budget,
) -> Option<String> {
    let mut current = value;
    for _ in 0..4 {
        if current.is_proxy() {
            budget.unavailable = true;
            return None;
        }
        let Ok(property) = data(scope, current, "name") else {
            budget.unavailable = true;
            return None;
        };
        if let Some(value) = property {
            if let Ok(name) = v8::Local::<v8::String>::try_from(value) {
                return Some(budget.text(scope, name));
            }
            budget.unavailable = true;
            return None;
        }
        let object = v8::Local::<v8::Object>::try_from(current).ok()?;
        current = object.get_prototype(scope)?;
    }
    budget.unavailable = true;
    None
}

fn trace_frames<'s>(
    scope: &v8::PinScope<'s, '_>,
    trace: v8::Local<'s, v8::StackTrace>,
    budget: &mut Budget,
) -> Vec<StackFrame> {
    let mut frames = Vec::new();
    let count = trace.get_frame_count().min(budget.frames);
    budget.truncated |=
        trace.get_frame_count() > count || trace.get_frame_count() >= MAX_EXCEPTION_FRAMES;
    budget.frames -= count;
    for index in 0..count {
        let Some(frame) = trace.get_frame(scope, index) else {
            continue;
        };
        // get_script_name ignores authored //# sourceURL spoofing.
        let generated = frame.get_script_name(scope).and_then(|source| {
            if source.utf8_length(scope) > budget.bytes.min(2048) {
                // Truncating an identity could make it name another module.
                budget.truncated = true;
                return None;
            }
            let line = u32::try_from(frame.get_line_number())
                .ok()
                .filter(|n| *n > 0)?;
            let column = u32::try_from(frame.get_column()).ok().filter(|n| *n > 0)?;
            Some(SourceLocation {
                source: budget.text(scope, source),
                line,
                column,
            })
        });
        let function = frame
            .get_function_name(scope)
            .map(|name| budget.text(scope, name))
            .unwrap_or_default();
        frames.push(StackFrame {
            function,
            generated,
            original: None,
            mapping_issue: None,
        });
    }
    frames
}

pub(super) fn current_frames(
    scope: &v8::PinScope<'_, '_>,
    bytes: usize,
    frames: usize,
) -> (Vec<StackFrame>, bool) {
    let mut budget = Budget {
        bytes,
        frames,
        truncated: false,
        unavailable: false,
    };
    let Some(trace) = v8::StackTrace::current_stack_trace(scope, frames) else {
        return (vec![], true);
    };
    let full = trace.get_frame_count() >= frames;
    let frames = trace_frames(scope, trace, &mut budget);
    (frames, budget.truncated || full)
}

fn snapshot<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    budget: &mut Budget,
) -> ExceptionValue {
    let mut output = ExceptionValue {
        name: None,
        message: String::new(),
        frames: vec![],
        unavailable: false,
    };
    if let Ok(text) = v8::Local::<v8::String>::try_from(value) {
        output.message = budget.text(scope, text);
    } else if value.is_null() {
        output.message = "null".into();
    } else if value.is_undefined() {
        output.message = "undefined".into();
    } else if value.is_boolean() {
        output.message = if value.is_true() { "true" } else { "false" }.into();
    } else if value.is_number() {
        output.message = value.number_value(scope).unwrap_or_default().to_string();
    } else {
        output.name = name(scope, value, budget);
        let message = read_data(scope, value, "message", budget);
        if let Some(message) = message.and_then(|v| v8::Local::<v8::String>::try_from(v).ok()) {
            output.message = budget.text(scope, message);
        } else if let Some(message) = message
            .filter(|message| message.is_number() || message.is_boolean() || message.is_null())
        {
            let text = if message.is_null() {
                "null".into()
            } else if message.is_boolean() {
                if message.is_true() {
                    "true".into()
                } else {
                    "false".into()
                }
            } else {
                message.number_value(scope).unwrap_or_default().to_string()
            };
            let length = text.len().min(budget.bytes);
            text[..length].clone_into(&mut output.message);
            budget.bytes -= length;
            budget.truncated |= length < text.len();
        } else if !value.is_native_error()
            || budget.unavailable
            || message.is_some_and(|message| !message.is_undefined())
        {
            output.message = "Thrown value details unavailable".into();
            output.unavailable = true;
        }
    }
    // Primitive labels are host-owned and small; charge their bytes as well.
    if value.is_null()
        || value.is_undefined()
        || value.is_boolean()
        || value.is_number()
        || output.unavailable
    {
        if output.message.len() > budget.bytes {
            output.message.truncate(budget.bytes);
            budget.truncated = true;
        }
        budget.bytes = budget.bytes.saturating_sub(output.message.len());
    }
    if !value.is_proxy()
        && let Some(trace) = v8::Exception::get_stack_trace(scope, value)
    {
        output.frames = trace_frames(scope, trace, budget);
    }
    output
}

pub(super) fn capture<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> ExceptionDiagnostic {
    v8::tc_scope!(let scope, scope);
    let mut budget = Budget {
        bytes: MAX_EXCEPTION_TEXT_BYTES,
        frames: MAX_EXCEPTION_FRAMES,
        truncated: false,
        unavailable: false,
    };
    let mut exception = snapshot(scope, value, &mut budget);
    let mut seen = vec![value];
    let mut causes = Vec::new();
    let mut current = value;
    while let Some(cause) = read_data(scope, current, "cause", &mut budget) {
        if causes.len() >= MAX_EXCEPTION_CAUSES || seen.iter().any(|v| v.strict_equals(cause)) {
            budget.truncated = true;
            break;
        }
        seen.push(cause);
        causes.push(snapshot(scope, cause, &mut budget));
        current = cause;
    }
    exception.unavailable |= budget.unavailable;
    ExceptionDiagnostic {
        artifact: None,
        exception,
        causes,
        truncated: budget.truncated,
    }
}

pub(super) fn summary(diagnostic: &ExceptionDiagnostic) -> String {
    use std::fmt::Write;
    let value = &diagnostic.exception;
    let mut text = value.name.as_ref().map_or_else(
        || value.message.clone(),
        |name| format!("{name}: {}", value.message),
    );
    for frame in &value.frames {
        if let Some(source) = &frame.generated {
            let _ = write!(
                text,
                "\n    at {} ({}:{}:{})",
                frame.function, source.source, source.line, source.column
            );
        }
    }
    let mut end = text.len().min(super::ERROR_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

pub(super) fn primitive_location<'s>(
    scope: &v8::PinScope<'s, '_>,
    diagnostic: &mut ExceptionDiagnostic,
    message: v8::Local<'s, v8::Message>,
) {
    if !diagnostic.exception.frames.is_empty() {
        return;
    }
    // Message resource names honor authored sourceURL comments. Detailed frames
    // retain the real script identity and are the only source of attribution.
    let Some(trace) = message.get_stack_trace(scope) else {
        diagnostic.exception.unavailable = true;
        return;
    };
    let mut budget = Budget {
        bytes: MAX_EXCEPTION_TEXT_BYTES.saturating_sub(diagnostic.exception.message.len()),
        frames: MAX_EXCEPTION_FRAMES,
        truncated: false,
        unavailable: false,
    };
    diagnostic.exception.frames = trace_frames(scope, trace, &mut budget);
    diagnostic.exception.unavailable |= diagnostic.exception.frames.is_empty();
    diagnostic.truncated |= budget.truncated;
}
