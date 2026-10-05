//! Entrypoint for the capability-free Transmog script helper.

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--sandbox-bootstrap")) {
        let max_heap_bytes = arguments
            .next()
            .and_then(|value| value.to_string_lossy().parse::<usize>().ok())
            .unwrap_or(64 * 1024 * 1024);
        match transmog_script_host::sandbox_bootstrap(max_heap_bytes) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("script sandbox bootstrap failed: {error}");
                std::process::exit(71);
            }
        }
    }
    let sandboxed = std::env::args_os().any(|argument| argument == "--sandboxed");
    if sandboxed && let Err(error) = transmog_script_host::verify_sandbox() {
        eprintln!("script sandbox verification failed: {error}");
        std::process::exit(72);
    }
    transmog_script_host::configure_v8();
    if let Err(error) = transmog_script_host::run(std::io::stdin().lock(), std::io::stdout().lock())
    {
        eprintln!("script host failed: {error}");
        std::process::exit(70);
    }
}
