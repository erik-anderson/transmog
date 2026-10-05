//! AppContainer entry point for the one-shot raster preview decoder.

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--sandbox-bootstrap")) {
        match transmog_script_host::sandbox_bootstrap(transmog_preview_worker::PROCESS_MEMORY_BYTES)
        {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("preview sandbox bootstrap failed: {error}");
                std::process::exit(71);
            }
        }
    }
    if !std::env::args_os().any(|argument| argument == "--sandboxed") {
        eprintln!("preview worker requires its operating-system sandbox");
        std::process::exit(72);
    }
    if let Err(error) = transmog_script_host::verify_sandbox() {
        eprintln!("preview sandbox verification failed: {error}");
        std::process::exit(72);
    }
    if let Err(error) =
        transmog_preview_worker::run(std::io::stdin().lock(), std::io::stdout().lock())
    {
        eprintln!("preview worker failed: {error}");
        std::process::exit(70);
    }
}
