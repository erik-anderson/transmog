"""Opt-in SDK toolchain and build helpers; never imported by app/CLI builds."""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tomllib

from repo_tools import ROOT, command, executable

SDK = ROOT / "sdk"
PIN = json.loads((SDK / "toolchain.json").read_text(encoding="utf-8"))


def verify_selections():
    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    sdk = tomllib.loads((SDK / "Cargo.toml").read_text(encoding="utf-8"))
    if set(sdk["patch"]["crates-io"]) != set(root["patch"]["crates-io"]):
        raise ValueError("SDK overrides must match the root manifest exactly")
    for name, selection in root["patch"]["crates-io"].items():
        actual = dict(sdk["patch"]["crates-io"].get(name, {}))
        expected = dict(selection)
        for base, value in ((ROOT, expected), (SDK, actual)):
            if "path" in value:
                value["path"] = str((base / value["path"]).resolve())
        if actual != expected:
            raise ValueError(f"SDK {name} override differs from the app/CLI selection")
    for name in ("http", "serde", "serde_json", "tokio"):
        if sdk["workspace"]["dependencies"][name] != root["workspace"]["dependencies"][name]:
            raise ValueError(f"SDK {name} dependency differs from the repository selection")
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))
    if PIN["rust"] != toolchain["toolchain"]["channel"]:
        raise ValueError("SDK Rust pin needs qualification against the repository toolchain")


def ensure_crubit_source(source, environment):
    """Resume an interrupted fetch without resetting an existing checkout."""
    if not source.exists():
        command(["git", "init", source], env=environment)
    if command(["git", "-C", source, "status", "--porcelain"],
               env=environment, capture=True).stdout.strip():
        raise ValueError("SDK Crubit source must be clean")
    revision = command(["git", "-C", source, "rev-parse", "--verify", "HEAD"],
                       env=environment, capture=True, check=False)
    if revision.returncode:
        command(["git", "-C", source, "fetch", "--depth", "1", PIN["crubit_repository"],
                 PIN["crubit_revision"]], env=environment)
        command(["git", "-C", source, "checkout", "--detach", "FETCH_HEAD"], env=environment)
        revision = command(["git", "-C", source, "rev-parse", "HEAD"], env=environment, capture=True)
    if revision.stdout.strip() != PIN["crubit_revision"]:
        raise ValueError(f"Expected Crubit {PIN['crubit_revision']}, found {revision.stdout.strip()} in {source}")


def generator_environment(environment, compiler):
    """Confine compiler-private linkage and bootstrap flags to Crubit."""
    env = dict(environment, RUSTC_BOOTSTRAP="1", CARGO_TARGET_DIR=str(ROOT / ".tools/sdk-crubit-target"))
    if os.name != "nt":
        library = str(compiler / "lib")
        env["LD_LIBRARY_PATH"] = os.pathsep.join(filter(None, (library, environment.get("LD_LIBRARY_PATH", ""))))
        # rustc-dev references LLVM in the compiler's top-level lib directory,
        # which is not a default rustc native-link search directory on Linux.
        encoded = environment.get("CARGO_ENCODED_RUSTFLAGS")
        flags = encoded.split("\x1f") if encoded else environment.get("RUSTFLAGS", "").split()
        env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([*flags, "-L", f"native={library}"])
    return env


def bootstrap(environment):
    """Build just cc_bindings_from_rs with a compiler-private SDK-local sysroot."""
    source = ROOT / ".tools/sdk-crubit"
    rustup_home = ROOT / ".tools/sdk-rustup"
    compiler_details = command(["rustc", "-vV"], env=environment, capture=True).stdout
    host = next(line.split(": ", 1)[1] for line in compiler_details.splitlines() if line.startswith("host:"))
    compiler = rustup_home / "toolchains" / f"{PIN['rust']}-{host}"
    env = dict(environment, RUSTUP_HOME=str(rustup_home), RUSTUP_TOOLCHAIN=PIN["rust"])
    compiler_exe = compiler / "bin" / ("rustc.exe" if os.name == "nt" else "rustc")
    components = command(["rustup", "component", "list", "--installed", "--toolchain", PIN["rust"]],
                         env=env, capture=True, check=False)
    required = {f"{name}-{host}" for name in ("rustc-dev", "rust-src", "rustfmt")}
    # rust-src is a target-independent component.
    required.remove(f"rust-src-{host}")
    required.add("rust-src")
    if not compiler_exe.is_file() or not required.issubset(components.stdout.splitlines()):
        command(["rustup", "toolchain", "install", PIN["rust"], "--profile", "minimal",
                 "--component", "rustc-dev,rust-src,rustfmt", "--no-self-update"], env=env)
    if command([compiler_exe, "-vV"], env=env, capture=True).stdout != compiler_details:
        raise ValueError("SDK generator and engine must use the exact same Rust compiler; check toolchain overrides")
    ensure_crubit_source(source, env)
    env["PATH"] = str(compiler / "bin") + os.pathsep + env["PATH"]
    generator_env = generator_environment(env, compiler)
    # Stable rustc_private requires this opt-in. It is confined to the tool and
    # generation processes; the SDK engine and application never receive it.
    command(["cargo", "build", "--release", "--locked", "--target", host, "--manifest-path",
             source / "Cargo.toml", "-p", "cc_bindings_from_rs"], cwd=source, env=generator_env)
    binary = Path(generator_env["CARGO_TARGET_DIR"]) / host / "release" / (
        "cc_bindings_from_rs.exe" if os.name == "nt" else "cc_bindings_from_rs")
    return binary, generator_env, host


def cargo_build(arguments, environment):
    """Read Cargo's artifact receipt instead of guessing hashed rlib names."""
    from tls_dependencies import preflight
    argv = ["build", "--manifest-path", str(SDK / "Cargo.toml"), "--locked", *arguments,
            "--message-format=json-render-diagnostics"]
    preflight(executable("cargo", environment), argv, ROOT, environment)
    result = command(["cargo", *argv], cwd=SDK, env=environment, capture=True, check=False)
    if result.stderr:
        print(result.stderr, end="", flush=True)
    if result.returncode:
        print(result.stdout, end="", flush=True)
        raise subprocess.CalledProcessError(result.returncode, ["cargo", *argv])
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
