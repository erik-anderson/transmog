//! Owned-process Inspector proof using the production Windows sandbox.

fn main() {
    if std::env::args_os().any(|arg| arg == "--sandboxed") {
        transmog_process_sandbox::verify_current_process().expect("sandbox verification");
        transmog_js_runtime::inspector_probe::run().expect("inspector proof");
    } else {
        let code = transmog_process_sandbox::sandbox_bootstrap(
            transmog_process_sandbox::SandboxIdentity {
                profile_namespace: "InspectorProbe",
                display_name: "Transmog Inspector proof",
                description: "Development-only isolated debugger verification",
            },
            384 * 1024 * 1024,
        )
        .expect("sandbox bootstrap");
        std::process::exit(code);
    }
}
