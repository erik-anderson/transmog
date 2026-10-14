//! Native overrides are rejected even without the portable Cargo preflight.
#[path = "../build_support.rs"]
mod build_support;

#[test]
fn source_and_archive_overrides_are_rejected_in_every_supported_env_spelling() {
    for option in ["PATH", "INCLUDE_PATH", "SOURCE_PATH", "ASSUME_PATCHED"] {
        for prefix in ["BORING_BSSL_", "BORING_BSSL_FIPS_"] {
            let name = format!("{prefix}{option}");
            for key in [
                name.clone(),
                format!("HOST_{name}"),
                format!("TARGET_{name}"),
                format!("{name}_x86_64-pc-windows-msvc"),
                format!("{name}_x86_64_pc_windows_msvc"),
            ] {
                assert!(build_support::replaces_native_library(&key));
                assert!(build_support::replaces_native_library(&key.to_lowercase()));
            }
        }
    }
    for allowed in [
        "PATH",
        "CC",
        "CXX",
        "BORING_BSSL_SYSROOT",
        "BORING_BSSL_RUST_CPPLIB",
        "BORING_BSSL_INSTALL_DIR",
    ] {
        assert!(!build_support::replaces_native_library(allowed));
    }
}
