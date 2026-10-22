"""Build the opt-in no-throw C++ SDK; consumers use the staged CMake package."""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import shlex
import sys

from repo_tools import ROOT, command, development_environment, executable, main_guard, write_json
from sdk_tools import SDK, PIN, bootstrap, cargo_build, verify_selections


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--configuration", choices=("Debug", "Release"), default="Debug")
    parser.add_argument("--test", action="store_true", help="Run C++ contract and installed-consumer checks")
    args = parser.parse_args()
    if sys.platform not in {"win32", "linux"}:
        raise ValueError("The SDK currently builds on Windows and Linux hosts only")
    command([sys.executable, ROOT / "scripts/dev-env.py", "--check"])
    verify_selections()
    env = development_environment()
    # Do not inherit an opt-in compiler bootstrap into engine builds.
    env.pop("RUSTC_BOOTSTRAP", None)
    generator, generator_env, host = bootstrap(env)
    output = ROOT / ".local/sdk" / host / args.configuration
    generated = output / "generated"
    generated.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(ROOT / ".local/sdk/rust-target")
    profile = ["--release"] if args.configuration == "Release" else []
    print("Building the isolated Rust SDK facade", flush=True)
    # This is a host-only SDK build, regardless of inherited Cargo target settings.
    receipts = cargo_build(["-p", "transmog-sdk-facade", "--target", host, *profile], env)
    artifact = next(item for item in receipts if item.get("reason") == "compiler-artifact"
                    and item["target"]["name"] == "transmog_sdk_facade")
    rlib = Path(next(path for path in artifact["filenames"] if path.endswith(".rlib")))
    host_deps = Path(env["CARGO_TARGET_DIR"]) / ("release" if profile else "debug") / "deps"
    header, glue = generated / "sdk_generated.hpp", generated / "glue.rs"
    command([generator, "--h-out", header, "--rs-out", glue,
             "--crubit-support-path-format", "<support/{header}>",
             "--crate-feature", "self=supported", "--portable-abi-compatible",
             "--crate-namespace", "self=transmog_sdk_rust", "--enable-rmeta-interface=true",
             "--source-crate-name", "transmog_sdk_facade", "--target", host,
             "--extern", f"transmog_sdk_facade={rlib}", "-L", f"dependency={rlib.parent / 'deps'}",
             "-L", f"dependency={host_deps}"],
            cwd=SDK, env=generator_env)
    # Fail before compiling C++ if a required facade operation was omitted.
    errors = header.read_text(encoding="utf-8")
    for name in ("application_create", "application_create_with_body_store", "application_release", "application_valid",
                 "status", "product_state", "save_product_state", "start_proxy", "stop_proxy",
                 "shutdown", "query_sessions", "query_traffic_view", "traffic_view_rows",
                 "release_traffic_view", "subscribe_session_updates", "session_updates_valid",
                 "session_updates_status", "session_updates_release", "session_detail", "read_body_range", "release", "succeeded",
                 "result_json", "result_json_length", "result_error", "result_error_length"):
        if not re.search(rf"\b{name}\s*\(", errors):
            raise ValueError(f"Crubit omitted required SDK operation {name}: inspect {header}")
    env["TRANSMOG_SDK_GLUE"] = str(glue)
    bindings = cargo_build(["-p", "transmog-sdk-bindings", "--target", host, *profile], env)
    receipts += bindings
    native_dirs = sorted({path.split("=", 1)[-1] for item in receipts
                          if item.get("reason") == "build-script-executed"
                          for path in item.get("linked_paths", [])})
    receipt = command(["cargo", "rustc", "--manifest-path", SDK / "Cargo.toml", "--locked",
                       "-p", "transmog-sdk-bindings", "--target", host, *profile,
                       "--", "--print", "native-static-libs"],
                      cwd=SDK, env=env, capture=True)
    match = re.search(r"native-static-libs:\s*(.+)", receipt.stderr + receipt.stdout)
    if not match:
        raise ValueError("Rust did not report the static library's native link dependencies")
    native_libs = shlex.split(match.group(1))
    artifact = next(item for item in bindings if item.get("reason") == "compiler-artifact"
                    and item["target"]["name"] == "transmog_sdk_bindings")
    extension = ".lib" if os.name == "nt" else ".a"
    rust_lib = Path(next(path for path in artifact["filenames"] if path.endswith(extension)))
    build, install = output / "cmake", output / "install"
    command(["cmake", "-S", SDK, "-B", build, "-G", "Ninja",
             f"-DCMAKE_BUILD_TYPE={args.configuration}", f"-DCMAKE_INSTALL_PREFIX={install.as_posix()}",
             f"-DCMAKE_CXX_COMPILER={executable(env['CXX'], env)}",
             f"-DTRANSMOG_RUST_LIBRARY={rust_lib.as_posix()}", f"-DTRANSMOG_GENERATED_DIR={generated.as_posix()}",
             f"-DTRANSMOG_CRUBIT_ROOT={(ROOT / '.tools/sdk-crubit').as_posix()}",
             f"-DTRANSMOG_NATIVE_LIBS={';'.join(native_libs)}",
             "-DTRANSMOG_NATIVE_DIRS=" + ";".join(path.replace("\\", "/") for path in native_dirs),
             f"-DBUILD_TESTING={'ON' if args.test else 'OFF'}"], env=env)
    command(["cmake", "--build", build], env=env)
    command(["cmake", "--install", build], env=env)
    write_json(install / "share/transmog/build.json", {
        "configuration": args.configuration, "target": host, **PIN,
        "source_revision": command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip(),
        "source_dirty": bool(command(["git", "status", "--porcelain"], capture=True).stdout.strip()),
        "rustc": command(["rustc", "-vV"], env=env, capture=True).stdout,
        "cxx": command([env["CXX"], "--version"], env=env, capture=True).stdout,
    })
    if args.test:
        command(["ctest", "--test-dir", build, "--output-on-failure"], env=env)
        consumer = output / "consumer"
        command(["cmake", "-S", ROOT / "examples/cpp-embedding", "-B", consumer, "-G", "Ninja",
                 f"-DCMAKE_BUILD_TYPE={args.configuration}", f"-DCMAKE_PREFIX_PATH={install}",
                 f"-DCMAKE_CXX_COMPILER={executable(env['CXX'], env)}"], env=env)
        command(["cmake", "--build", consumer], env=env)
        command(["ctest", "--test-dir", consumer, "--output-on-failure"], env=env)
    print(f"SDK_OUTPUT={install}")


if __name__ == "__main__":
    main_guard(main)
