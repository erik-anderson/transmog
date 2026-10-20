//! Development extension helper; shipping uses the shared script-host executable.
fn main() {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--extension") {
        args.remove(0);
    }
    std::process::exit(transmog_extension_host::run_process(&args));
}
