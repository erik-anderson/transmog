"""Run the SDK gate in native Linux userspace, without modifying the host checkout."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from repo_tools import command, write_json

REPORTS = Path("/qualification")
SDK_METADATA = "https://builds.dotnet.microsoft.com/dotnet/release-metadata/10.0/releases.json"


def install_dotnet():
    """Install Microsoft's checksum-verified portable SDK into the Linux-only cache."""
    metadata = json.loads(urllib.request.urlopen(SDK_METADATA, timeout=60).read())
    version = metadata["latest-sdk"]
    sdks = [sdk for release in metadata["releases"] for sdk in release.get("sdks", [release.get("sdk", {})])]
    sdk = next(sdk for sdk in sdks if sdk["version"] == version)
    archive = next(item for item in sdk["files"] if item["rid"] == "linux-x64")
    directory = ROOT / ".tools/dotnet" / version
    executable = directory / "dotnet"
    if not executable.is_file():
        directory.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=directory.parent) as temporary:
            download = Path(temporary) / "sdk.tar.gz"
            print(f"Downloading .NET SDK {version}", flush=True)
            with urllib.request.urlopen(archive["url"], timeout=120) as response, download.open("wb") as output:
                shutil.copyfileobj(response, output)
            with download.open("rb") as source:
                actual = hashlib.file_digest(source, "sha512").hexdigest()
            if actual.lower() != archive["hash"].lower():
                raise ValueError("Microsoft .NET SDK SHA-512 did not match release metadata")
            command(["tar", "-xzf", download, "-C", directory])
    os.environ["DOTNET_ROOT"] = str(directory)
    os.environ["PATH"] = str(directory) + os.pathsep + os.environ["PATH"]
    return {"version": version, "url": archive["url"], "sha512": archive["hash"]}


def source_digest():
    """Fingerprint tracked and non-ignored new sources, including the uncommitted SDK."""
    paths = command(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], capture=True).stdout.split("\0")
    digest = hashlib.sha256()
    for name in sorted(set(filter(None, paths))):
        path = ROOT / name
        if path.is_file():
            digest.update(name.encode() + b"\0")
            with path.open("rb") as source:
                digest.update(hashlib.file_digest(source, "sha256").digest())
    return digest.hexdigest()


def relocated_consumer(install, env):
    """Use copied consumer sources and installation with no compiler-private inputs."""
    with tempfile.TemporaryDirectory(prefix="sdk-relocated-") as temporary:
        directory = Path(temporary)
        relocated, source, build = directory / "SDK with spaces", directory / "consumer", directory / "build"
        shutil.copytree(install, relocated)
        shutil.copytree(ROOT / "examples/cpp-embedding", source)
        consumer_env = dict(env, PATH=os.pathsep.join([str(Path(env["DOTNET_ROOT"])), "/usr/bin", "/bin"]))
        command(["cmake", "-S", source, "-B", build, "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release",
                 f"-DCMAKE_PREFIX_PATH={relocated}", "-DCMAKE_CXX_COMPILER=clang++", "-DCMAKE_C_COMPILER=clang"], env=consumer_env)
        command(["cmake", "--build", build], env=consumer_env)
        command(["ctest", "--test-dir", build, "--output-on-failure"], env=consumer_env)
        command([sys.executable, ROOT / "scripts/build-dotnet-sample.py", "--configuration", "Release",
                 "--sdk-output", relocated], env=consumer_env)


def main():
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise ValueError("This qualification image targets Linux x86_64")
    command(["git", "config", "--global", "--add", "safe.directory", str(ROOT)])
    before = source_digest()
    report = {
        "status": "running", "source_sha256": before,
        "source_revision": command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip(),
        "source_dirty": bool(command(["git", "status", "--porcelain"], capture=True).stdout.strip()),
        "platform": Path("/etc/os-release").read_text(), "kernel": platform.release(),
        "steps": [],
    }
    report_path = REPORTS / "qualification.json"
    write_json(report_path, report)

    def step(name, argv):
        print(f"Starting {name}", flush=True)
        report["current_step"] = name
        write_json(report_path, report)
        started = time.monotonic()
        log = REPORTS / (name + ".log")
        with log.open("w", encoding="utf-8") as output:
            result = subprocess.run([str(value) for value in argv], cwd=ROOT,
                                    stdout=output, stderr=subprocess.STDOUT, check=False)
        record = {"name": name, "exit_code": result.returncode, "seconds": round(time.monotonic() - started, 2)}
        report["steps"].append(record)
        write_json(report_path, report)
        if result.returncode:
            print(log.read_text(errors="replace")[-12000:], flush=True)
            raise RuntimeError(f"{name} failed; see {log}")
        print(f"Passed {name} ({record['seconds']}s)", flush=True)

    try:
        report["dotnet"] = install_dotnet()
        step("environment", [sys.executable, "scripts/dev-env.py", "--check"])
        step("portable-tools", [sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_tools.py"])
        report["rustc"] = command(["rustc", "-vV"], capture=True).stdout
        report["clang"] = command(["clang++", "--version"], capture=True).stdout
        step("format", [sys.executable, "scripts/dev-env.py", "--run", "cargo", "fmt", "--manifest-path", "sdk/Cargo.toml", "--all", "--", "--check"])
        step("isolation", [sys.executable, "scripts/check-sdk-isolation.py"])
        step("bootstrap-regression", [sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_sdk_tools.py"])
        for configuration in ("Debug", "Release"):
            step(configuration.lower(), [sys.executable, "scripts/test-sdk.py", "--configuration", configuration])
        cargo = [sys.executable, "scripts/dev-env.py", "--run", "cargo"]
        selection = ["--manifest-path", "sdk/Cargo.toml", "--locked", "--target", "x86_64-unknown-linux-gnu",
                     "-p", "transmog-sdk-engine", "-p", "transmog-sdk-facade"]
        step("clippy", [*cargo, "clippy", *selection, "--all-targets", "--", "-D", "warnings"])
        step("parity-tests", [*cargo, "test", *selection])
        application_tests = ["--manifest-path", "Cargo.toml", "--locked", "--target", "x86_64-unknown-linux-gnu",
                             "-p", "transmog-app", "--lib"]
        step("body-range-tests", [*cargo, "test", *application_tests, "body_ranges::tests"])
        step("composer-body-tests", [*cargo, "test", *application_tests, "composer_response::tests"])
        install = ROOT / ".local/sdk/x86_64-unknown-linux-gnu/Release/install"
        relocated_consumer(install, dict(os.environ))
        report["relocated_consumers"] = "passed without Rust/Crubit on PATH"
        report["elf_dependencies"] = command(["ldd", install / "lib/libtransmog_sdk.so"], capture=True).stdout
        exports = command(["nm", "-D", "--defined-only", install / "lib/libtransmog_sdk.so"], capture=True).stdout
        (REPORTS / "exports.txt").write_text(exports)
        if "__crubit" in exports or "transmog_sdk_application_create" not in exports:
            raise ValueError("Installed ELF exports do not match the private-bindings contract")
        if source_digest() != before:
            raise ValueError("Source changed during qualification; rerun against the final working tree")
        for configuration in ("Debug", "Release"):
            stage = ROOT / ".local/sdk/x86_64-unknown-linux-gnu" / configuration / "install"
            with tarfile.open(REPORTS / f"transmog-sdk-linux-x64-{configuration.lower()}.tar.gz", "w:gz") as archive:
                archive.add(stage, arcname="transmog")
        report["status"] = "passed"
    except Exception as error:
        report["status"], report["error"] = "failed", str(error)
        raise
    finally:
        write_json(report_path, report)
    print("Linux native SDK qualification passed", flush=True)


if __name__ == "__main__":
    main()
