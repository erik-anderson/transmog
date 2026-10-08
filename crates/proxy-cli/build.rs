//! Windows identity for the standalone signed support tool.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    {
        let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo supplies a version");
        let (base, build) = version.split_once('+').unwrap_or((&version, "0"));
        let parts = base
            .split('.')
            .chain(std::iter::once(build))
            .map(|part| {
                part.parse::<u16>()
                    .expect("Release versions use four numeric components")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            parts.len(),
            4,
            "Release versions use four numeric components"
        );
        let numeric = parts
            .iter()
            .fold(0_u64, |value, part| value << 16 | u64::from(*part));
        let display = parts
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(".");
        tauri_winres::WindowsResource::new()
            .set("ProductName", "Transmog CLI")
            .set("FileDescription", "Transmog support capture tool")
            .set("OriginalFilename", "transmog-cli.exe")
            .set("FileVersion", &display)
            .set("ProductVersion", &display)
            .set_version_info(tauri_winres::VersionInfo::FILEVERSION, numeric)
            .set_version_info(tauri_winres::VersionInfo::PRODUCTVERSION, numeric)
            .set_icon("../../apps/desktop/icons/icon.ico")
            .compile()
            .expect("Windows CLI resources must compile");
    }
}
