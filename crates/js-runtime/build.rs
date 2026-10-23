//! Use the workspace's dynamic MSVC runtime in V8-linked unit tests as in both hosts.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // The pinned prebuilt V8 archive embeds a conflicting static CRT default.
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:libcmt.lib");
    }
}
