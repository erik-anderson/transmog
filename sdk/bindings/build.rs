use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=TRANSMOG_SDK_GLUE");
    let source = PathBuf::from(
        env::var_os("TRANSMOG_SDK_GLUE")
            .expect("Build this opt-in SDK with python scripts/build-sdk.py"),
    );
    let source = source.canonicalize().expect("Crubit Rust glue is missing");
    println!("cargo:rerun-if-changed={}", source.display());
    let wrapper = format!("#[path = {:?}] mod generated;\n", source.to_string_lossy());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings.rs");
    fs::write(output, wrapper).unwrap();
}
