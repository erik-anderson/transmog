//! Link configuration for the isolated script-host executable.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // The prebuilt rusty_v8 archive embeds /DEFAULTLIB:libcmt even when its
        // build script selects the dynamic MSVC runtime used by Rust. Keep the
        // workspace on that runtime and discard only the conflicting default.
        // Apply this to the binary and to test harnesses, which also link V8.
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:libcmt.lib");
    }
}
