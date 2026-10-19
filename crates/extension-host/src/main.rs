//! Production extension helper; source and captured input require verified OS containment.
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--sandbox-bootstrap") {
        let heap = args
            .get(1)
            .and_then(|s| s.to_string_lossy().parse::<usize>().ok())
            .unwrap_or(128 * 1024 * 1024);
        match transmog_extension_host::sandbox_bootstrap(heap) {
            Ok(code) => std::process::exit(code),
            Err(_) => std::process::exit(71),
        }
    }
    // No development flag can bypass the same production boundary.
    if args.len() != 1
        || args[0] != "--sandboxed"
        || transmog_process_sandbox::verify_current_process().is_err()
    {
        std::process::exit(72);
    }
    transmog_extension_host::configure_v8();
    if transmog_extension_host::run(std::io::stdin().lock(), std::io::stdout().lock()).is_err() {
        std::process::exit(70);
    }
}
