//! Fixed local engine benchmark. No user code, listener or worker protocol is accepted.
use std::{cell::Cell, rc::Rc, sync::mpsc, time::Instant};
use transmog_js_runtime::{Runtime, debugger::DebugTransport};
use transmog_script_diagnostics::debugger::{DebugOptions, ExceptionPolicy, MAX_DEBUG_COMMANDS};

fn main() {
    let mut results = Vec::new();
    for mode in [
        "baseline",
        "console-idle",
        "one-log",
        "saturated-logs",
        "attached",
    ] {
        let mut runtime = Runtime::new(1024 * 1024, 32 * 1024 * 1024).unwrap();
        let records = Rc::new(Cell::new(0u64));
        let accepted = Rc::clone(&records);
        if mode != "baseline" {
            runtime
                .enable_console(move |_, _| {
                    accepted.set(accepted.get() + 1);
                    true
                })
                .unwrap();
        }
        let (_commands, input) = mpsc::sync_channel(MAX_DEBUG_COMMANDS);
        if mode == "attached" {
            runtime
                .enable_debugger(
                    DebugOptions {
                        session: "a".repeat(32),
                        exceptions: ExceptionPolicy::None,
                        breakpoints: vec![],
                    },
                    DebugTransport::new(input, |_| true),
                )
                .unwrap();
        }
        let script = match mode {
            "one-log" => "console.debug('value',7);String(1+2)",
            "saturated-logs" => "for(let i=0;i<1000;i++)console.debug('value',i);String(1+2)",
            _ => "String(1+2)",
        };
        let mut samples = Vec::new();
        for batch in 0..12 {
            let start = Instant::now();
            for invocation in 0..1000 {
                if mode != "baseline" {
                    runtime.begin_console(invocation);
                    runtime.begin_debug_operation(invocation);
                }
                assert_eq!(
                    runtime
                        .execute_string("transmog:benchmark", script, 32)
                        .unwrap(),
                    "3"
                );
                if mode != "baseline" {
                    runtime.finish_console();
                }
            }
            if batch > 0 {
                samples.push(start.elapsed().as_nanos() / 1000);
            }
        }
        samples.sort_unstable();
        results.push(serde_json::json!({"mode":mode,"medianNs":samples[5],"samplesNs":samples,"acceptedEvents":records.get()}));
    }
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
