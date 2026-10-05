//! Delegates Windows resources to Tauri.

fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");

    #[cfg(windows)]
    tauri_build::build();
}
