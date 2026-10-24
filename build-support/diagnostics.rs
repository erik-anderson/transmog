//! Build-only diagnostic metadata shared by the application and desktop host.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn read(path: &Path) -> Vec<u8> {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read(path).unwrap_or_else(|error| panic!("read build metadata {}: {error}", path.display()))
}

fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&read(path)).expect("valid JSON build metadata")
}

fn manifest(path: &Path) -> toml::Value {
    toml::from_str(std::str::from_utf8(&read(path)).expect("UTF-8 Cargo metadata"))
        .expect("valid Cargo metadata")
}

fn required_text<'a>(metadata: &'a Value, field: &str) -> &'a str {
    metadata[field]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("build receipt requires nonempty {field}"))
}

fn required_hash<'a>(metadata: &'a Value, field: &str, length: usize) -> &'a str {
    let value = required_text(metadata, field);
    assert!(
        value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "build receipt requires a {length}-digit hexadecimal {field}"
    );
    value
}

fn git(root: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn git_text(root: &Path, arguments: &[&str]) -> Option<String> {
    String::from_utf8(git(root, arguments)?)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn source_identity(root: &Path) -> (Option<String>, Option<bool>) {
    // Portable Cargo wrappers also invalidate this on untracked-file changes.
    println!("cargo:rerun-if-env-changed=TRANSMOG_SOURCE_STATE");
    let owner = git_text(root, &["rev-parse", "--show-toplevel"])
        .and_then(|path| PathBuf::from(path).canonicalize().ok());
    if owner != root.canonicalize().ok() || owner.is_none() {
        return (None, None);
    }
    let Some(commit) = git_text(root, &["rev-parse", "--verify", "HEAD"]) else {
        return (None, None);
    };
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return (None, None);
    }
    for option in ["--git-dir", "--git-common-dir"] {
        if let Some(directory) = git_text(root, &["rev-parse", option]) {
            let directory = root.join(directory);
            let references = directory.join("refs");
            if references.is_dir() {
                println!("cargo:rerun-if-changed={}", references.display());
            }
            for name in ["HEAD", "index", "packed-refs"] {
                let path = directory.join(name);
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
            if let Some(reference) = git_text(root, &["symbolic-ref", "-q", "HEAD"]) {
                let path = directory.join(reference);
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
        }
    }
    if let Some(paths) = git(root, &["ls-files", "-z"]) {
        for path in paths
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            if let Ok(path) = std::str::from_utf8(path) {
                println!("cargo:rerun-if-changed={}", root.join(path).display());
            }
        }
    }
    // Cargo cannot watch newly created untracked files. A snapshot from the
    // portable runner is refreshed on every invocation; raw Cargo and snapshots
    // for another checkout must leave the full modified state unknown.
    let snapshot = env::var("TRANSMOG_SOURCE_STATE")
        .ok()
        .and_then(|value| serde_json::from_str::<Value>(&value).ok());
    let fresh = snapshot.as_ref().is_some_and(|snapshot| {
        snapshot["root"]
            .as_str()
            .and_then(|path| PathBuf::from(path).canonicalize().ok())
            == root.canonicalize().ok()
            && snapshot["state"].as_str().is_some_and(|state| {
                state.len() == 64 && state.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    });
    let dirty = fresh
        .then(|| {
            git(
                root,
                &["status", "--porcelain=v1", "--untracked-files=normal"],
            )
            .map(|status| !status.is_empty())
        })
        .flatten();
    (Some(commit), dirty)
}

fn identity(root: &Path) -> Value {
    let (source_commit, source_dirty) = source_identity(root);
    println!("cargo:rerun-if-env-changed=RUSTC");
    let rustc = Command::new(env::var_os("RUSTC").expect("Cargo supplies RUSTC"))
        .arg("--version")
        .output()
        .expect("read Rust compiler version");
    assert!(
        rustc.status.success(),
        "Rust compiler version command failed"
    );
    json!({
        "sourceCommit": source_commit,
        "sourceDirty": source_dirty,
        "target": env::var("TARGET").expect("Cargo supplies TARGET"),
        "profile": env::var("PROFILE").expect("Cargo supplies PROFILE"),
        "rustc": std::str::from_utf8(&rustc.stdout).expect("UTF-8 Rust compiler version").trim(),
    })
}

fn dependency(lock: &toml::Value, name: &str, label: &str, declared: &toml::Value) -> Value {
    let requirement = declared
        .as_str()
        .or_else(|| declared.get("version").and_then(toml::Value::as_str))
        .expect("dependency has a version");
    let version = requirement
        .strip_prefix('=')
        .expect("diagnostic dependencies must have exact versions");
    let matches = lock["package"]
        .as_array()
        .expect("locked packages")
        .iter()
        .filter(|package| {
            package["name"].as_str() == Some(name) && package["version"].as_str() == Some(version)
        })
        .count();
    assert_eq!(
        matches, 1,
        "{name} does not match its locked dependency metadata"
    );
    json!({"name": name, "label": label, "version": version})
}

fn write_output(filename: &str, value: &Value) {
    let path =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR")).join(filename);
    let bytes = serde_json::to_vec(value).expect("serializable build metadata");
    if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        fs::write(path, bytes).expect("write build metadata");
    }
}

pub fn application(root: &Path) {
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build-support/diagnostics.rs").display()
    );
    let cargo = manifest(&root.join("Cargo.toml"));
    let lock = manifest(&root.join("Cargo.lock"));
    let declared = &cargo["workspace"]["dependencies"];
    let mut dependencies = vec![json!({
        "name": "transmog-app", "label": "transmog-app",
        "version": env::var("CARGO_PKG_VERSION").expect("Cargo package version"),
    })];
    for (name, label) in [
        ("boring", "BoringSSL via boring"),
        ("v8", "V8 via rusty_v8 (isolated worker)"),
        ("swc_common", "SWC common"),
        ("swc_ecma_ast", "SWC AST"),
        ("swc_ecma_parser", "SWC parser"),
        ("swc_ecma_codegen", "SWC code generation"),
        ("swc_ecma_transforms_base", "SWC base transforms"),
        (
            "swc_ecma_transforms_typescript",
            "SWC TypeScript transforms",
        ),
        ("swc_ecma_visit", "SWC visitor"),
    ] {
        dependencies.push(dependency(&lock, name, label, &declared[name]));
    }
    let sources = bundled_sources(root, &dependencies);
    write_output(
        "build-info.json",
        &json!({"identity": identity(root), "dependencies": dependencies, "bundledSources": sources}),
    );
}

fn bundled_sources(root: &Path, dependencies: &[Value]) -> Value {
    let version = |name: &str| {
        dependencies
            .iter()
            .find(|dependency| dependency["name"] == name)
            .and_then(|dependency| dependency["version"].as_str())
            .expect("qualified dependency version")
    };
    let tls = json_file(&root.join("third_party/boringssl-pki/manifest.json"));
    let parser = json_file(&root.join("third_party/swc-ecma-parser/transmog-snapshot.json"));
    let parser_source = json_file(&root.join("third_party/swc-ecma-parser/.cargo_vcs_info.json"));
    let policy = json_file(&root.join("third_party/chromium-certificate-policy/snapshot.json"));
    let roots = json_file(&root.join("third_party/chromium-root-store/snapshot.json"));
    let suffixes = json_file(&root.join("third_party/chromium-suffix-list/snapshot.json"));
    assert_eq!(
        required_text(&tls, "boring_version"),
        version("boring"),
        "qualified TLS version differs"
    );
    assert_eq!(
        required_text(&parser["source"], "version"),
        version("swc_ecma_parser"),
        "qualified SWC parser version differs"
    );
    let root_version = roots["rootStoreVersion"]
        .as_u64()
        .filter(|version| *version > 0)
        .expect("build receipt requires positive rootStoreVersion");
    json!([
        {"name": "BoringSSL", "revision": required_hash(&tls, "revision", 40), "version": null, "patchSha256": null},
        {"name": "Boring Rust bindings", "revision": required_hash(&tls, "boring_revision", 40), "version": required_text(&tls, "boring_version"), "patchSha256": null},
        {"name": "SWC parser patch", "revision": required_hash(&parser_source["git"], "sha1", 40), "version": required_text(&parser["source"], "version"), "patchSha256": required_hash(&parser, "patchSha256", 64)},
        {"name": "Chromium certificate policy", "revision": null, "version": required_text(&policy, "chromiumVersion"), "patchSha256": null},
        {"name": "Chromium root store", "revision": required_hash(&roots, "sourceRevision", 40), "version": root_version.to_string(), "patchSha256": null},
        {"name": "Chromium Public Suffix List", "revision": null, "version": required_text(&suffixes, "chromiumVersion"), "patchSha256": null}
    ])
}

pub fn desktop(root: &Path) {
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build-support/diagnostics.rs").display()
    );
    let lock = manifest(&root.join("Cargo.lock"));
    let desktop = manifest(&root.join("apps/desktop/Cargo.toml"));
    let renderer = manifest(&root.join("crates/app-webui/Cargo.toml"));
    let mut dependencies = vec![
        dependency(
            &lock,
            "tauri",
            "Tauri",
            &desktop["target"]["cfg(windows)"]["dependencies"]["tauri"],
        ),
        dependency(
            &lock,
            "microsoft-webui",
            "Microsoft WebUI (Rust)",
            &renderer["dependencies"]["microsoft-webui"],
        ),
    ];
    let dist = root.join("apps/desktop/ui/dist");
    let client = json_file(&dist.join("client-build-info.json"));
    let inventory = json_file(&dist.join("client-assets.json"));
    let npm = json_file(&root.join("apps/desktop/ui/package-lock.json"));
    let expected: std::collections::BTreeSet<_> = inventory
        .as_array()
        .expect("client assets")
        .iter()
        .map(|asset| asset["file"].as_str().expect("asset file"))
        .collect();
    let actual: std::collections::BTreeSet<_> = client["assets"]
        .as_array()
        .expect("client hashes")
        .iter()
        .map(|asset| asset["file"].as_str().expect("hashed asset file"))
        .collect();
    assert!(
        !expected.is_empty() && expected == actual,
        "client build metadata has a different asset inventory"
    );
    for asset in client["assets"].as_array().expect("client asset hashes") {
        let name = asset["file"].as_str().expect("client asset file");
        assert!(
            !name.is_empty()
                && name.split('/').all(|part| !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))),
            "unsafe client asset path"
        );
        let actual = hex::encode(Sha256::digest(read(&dist.join(name))));
        assert_eq!(
            Some(actual.as_str()),
            asset["sha256"].as_str(),
            "client build metadata does not match {name}"
        );
    }
    for (name, label) in [
        ("@microsoft/webui-framework", "Microsoft WebUI (browser)"),
        ("@tauri-apps/api", "Tauri browser API"),
    ] {
        let version = client["dependencies"][name]
            .as_str()
            .expect("built client dependency version");
        assert_eq!(
            Some(version),
            npm["packages"][format!("node_modules/{name}")]["version"].as_str(),
            "built client version differs from locked {name}; rebuild the UI"
        );
        dependencies.push(json!({"name": name, "label": label, "version": version}));
    }
    write_output(
        "host-build-info.json",
        &json!({"identity": identity(root), "dependencies": dependencies}),
    );
}
