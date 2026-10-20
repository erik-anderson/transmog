//! Run the same extension contracts against either development or shipping hosts.
pub fn host_executable() -> &'static str {
    option_env!("CARGO_BIN_EXE_transmog-script-host")
        .or(option_env!("CARGO_BIN_EXE_transmog-extension-host"))
        .expect("extension tests require a helper executable")
}
