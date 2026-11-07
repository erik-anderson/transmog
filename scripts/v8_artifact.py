"""Export and verify the reusable, profile-bound Windows V8 engine."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

from repo_tools import ROOT, main_guard

TARGETS = ("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")
ABI_ENV = (
    "CARGO_FEATURE_SIMDUTF", "CARGO_FEATURE_USE_CUSTOM_LIBCXX", "CARGO_FEATURE_V8_ENABLE_SANDBOX",
    "CARGO_FEATURE_V8_ENABLE_POINTER_COMPRESSION", "CARGO_FEATURE_V8_ENABLE_V8_CHECKS",
    "CARGO_ENCODED_RUSTFLAGS", "CARGO_CFG_TARGET_FEATURE",
    "CFLAGS", "CXXFLAGS", "CLANG_BASE_PATH", "LIBCLANG_PATH",
    "GN_ARGS", "EXTRA_GN_ARGS", "V8_FORCE_DEBUG", "DISABLE_CLANG", "CXXSTDLIB",
)
INPUTS = (
    "rust-toolchain.toml", "scripts/setup-windows-ci.ps1", "scripts/repo_tools.py",
    "scripts/v8_artifact.py", "scripts/cache-v8.py", "scripts/v8_native.py",
    "scripts/v8_cache.py", "scripts/v8_bindgen.py", "scripts/icu_data.py",
)
FILES = ("rusty_v8.lib", "src_binding.rs", "args.gn")


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def encoded_digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def locked_dependencies(root):
    packages = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))["package"]
    engines = [package for package in packages if package["name"] == "v8" and "source" not in package]
    if len(engines) != 1:
        raise ValueError("Expected exactly one locked local V8 package")
    selected = {}
    pending = engines[:]
    while pending:
        package = pending.pop()
        key = (package["name"], package["version"], package.get("source", ""))
        if key in selected:
            continue
        selected[key] = package
        for dependency in package.get("dependencies", []):
            parts = dependency.split()
            matches = [value for value in packages if value["name"] == parts[0]
                       and (len(parts) == 1 or value["version"] == parts[1])
                       and (len(parts) < 3 or value.get("source") == parts[2].strip("()"))]
            if len(matches) != 1:
                raise ValueError(f"Ambiguous locked V8 build dependency: {dependency}")
            pending.extend(matches)
    return [selected[key] for key in sorted(selected)]


def input_hashes(root=ROOT):
    paths = [root / name for name in INPUTS]
    paths += sorted(path for path in (root / "third_party/rusty-v8").rglob("*") if path.is_file())
    result = {path.relative_to(root).as_posix(): digest(path) for path in paths}
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    # Release version bumps and unrelated dependency updates must not discard
    # an identical native engine. Keep only the workspace's V8 selection here.
    runtime = tomllib.loads((root / "crates/js-runtime/Cargo.toml").read_text(encoding="utf-8"))
    selection = {"runtimeDependency": runtime["dependencies"]["v8"],
                 "runtimeFeatures": runtime.get("features", {}), "dependency": workspace["workspace"]["dependencies"]["v8"],
                 "patch": workspace["patch"]["crates-io"]["v8"]}
    result["Cargo.toml#v8"] = encoded_digest(selection)
    # Bindgen and the other V8 build dependencies are engine inputs too. Hash
    # their locked transitive graph without including unrelated app packages.
    result["Cargo.lock#v8"] = encoded_digest(locked_dependencies(root))
    return result


def tools():
    result = {}
    for name, arguments in (("rustc", ["-vV"]), ("clang", ["--version"])):
        result[name] = subprocess.run([name, *arguments], cwd=ROOT, check=True, capture_output=True,
                                      text=True, encoding="utf-8").stdout.strip()
    # The reviewed native environment fixes SDK and MSVC include/library paths.
    # Include them so an image/toolset change cannot silently reuse old objects.
    result.update({name: os.environ.get(name, "") for name in ("INCLUDE", "LIB")})
    return result


def identity(target, root=ROOT):
    return {"schema": 1, "target": target, "inputs": input_hashes(root), "tools": tools()}


def validate(directory, target, root=ROOT, *, cargo=False):
    directory = Path(directory).resolve()
    expected = {"manifest.json", *FILES}
    if {path.name for path in directory.iterdir()} != expected:
        raise ValueError("V8 artifact has missing or unexpected files")
    for path in directory.iterdir():
        if path.is_symlink() or not path.is_file() or getattr(path, "is_junction", lambda: False)():
            raise ValueError("V8 artifact links and directories are forbidden")
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    if manifest["identity"] != identity(target, root):
        raise ValueError("V8 artifact source, profile, target or compiler differs")
    if manifest["key"] != encoded_digest(manifest["identity"]):
        raise ValueError("V8 artifact identity key differs")
    if manifest["files"] != {name: digest(directory / name) for name in FILES}:
        raise ValueError("V8 artifact checksum mismatch")
    if cargo and manifest["abi"] != {name: os.environ.get(name, "") for name in ABI_ENV}:
        raise ValueError("V8 artifact Cargo features or ABI flags differ")
    if not manifest.get("originCommit") or not manifest.get("originRunId"):
        raise ValueError("V8 artifact has no producer provenance")
    return manifest


def export(directory, target, library, bindings, gn_args, root=ROOT):
    directory = Path(directory)
    if directory.exists():
        raise ValueError("V8 export requires a fresh destination")
    directory.mkdir(parents=True)
    for name, source in zip(FILES, (library, bindings, gn_args)):
        shutil.copyfile(source, directory / name)
    engine = identity(target, root)
    manifest = {
        "identity": engine, "key": encoded_digest(engine),
        "abi": {name: os.environ.get(name, "") for name in ABI_ENV},
        "files": {name: digest(directory / name) for name in FILES},
        "originCommit": subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, check=True,
                                        text=True, capture_output=True).stdout.strip(),
        "originRunId": os.environ.get("GITHUB_RUN_ID", "local"),
    }
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    validate(directory, target, root, cargo=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("key", "verify", "export"))
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--library", type=Path)
    parser.add_argument("--bindings", type=Path)
    parser.add_argument("--gn-args", type=Path)
    parser.add_argument("--cargo", action="store_true")
    args = parser.parse_args()
    if args.operation != "key" and args.directory is None:
        parser.error("--directory is required for verify/export")
    if args.operation == "export" and not all((args.library, args.bindings, args.gn_args)):
        parser.error("export requires --library, --bindings and --gn-args")
    if args.operation == "key":
        print(encoded_digest(identity(args.target)))
    elif args.operation == "export":
        export(args.directory, args.target, args.library, args.bindings, args.gn_args)
    else:
        validate(args.directory, args.target, cargo=args.cargo)
        # Machine-readable local paths for build.rs; no native source setup or Ninja.
        for name in ("rusty_v8.lib", "src_binding.rs", "manifest.json"):
            print((args.directory / name).resolve())


if __name__ == "__main__":
    main_guard(main)
