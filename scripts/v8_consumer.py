"""Reuse an immutable successful native V8 consumer check when its context matches."""
from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import re
import subprocess
import time
import tomllib
import zipfile
from urllib.error import URLError
from urllib.request import Request

import github_release as github
from repo_tools import ROOT, main_guard
import v8_artifact as engine

TARGET = "aarch64-pc-windows-msvc"
COMMAND = ["cargo", "check", "--locked", "-p", "transmog-js-runtime", "--target", TARGET]
JOB = "Verify cross-built V8 on native ARM64"
WORKFLOWS = {".github/workflows/prepare-windows-v8.yml", ".github/workflows/windows-release.yml"}
INPUTS = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "scripts/v8_consumer.py",
          "scripts/dev-env.py", "scripts/dev-env.ps1", "scripts/repo_tools.py", "scripts/windows_cross.py",
          "scripts/v8_artifact.py", "scripts/github_release.py", "scripts/setup-windows-ci.ps1", "scripts/setup-windows-msvc.ps1",
          ".github/actions/windows-release-setup/action.yml", ".github/workflows/prepare-windows-v8.yml")


def patched_dependencies(root):
    # --no-deps metadata omits registry dependencies replaced with local patches.
    locked = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))["package"]
    pending = [p for p in locked if p["name"] == "transmog-js-runtime"]
    if len(pending) != 1:
        raise ValueError("Expected one locked native V8 consumer")
    visited = set()
    while pending:
        package = pending.pop()
        identity = (package["name"], package["version"], package.get("source", ""))
        if identity in visited:
            continue
        visited.add(identity)
        for dependency in package.get("dependencies", []):
            parts = dependency.split()
            matches = [p for p in locked if p["name"] == parts[0]
                       and (len(parts) == 1 or p["version"] == parts[1])
                       and (len(parts) < 3 or p.get("source") == parts[2].strip("()"))]
            if len(matches) != 1:
                raise ValueError("Ambiguous locked native V8 consumer dependency")
            pending.extend(matches)
    directories = []
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    for patches in workspace.get("patch", {}).values():
        for entry in patches.values():
            if isinstance(entry, dict) and "path" in entry:
                directory = (root / entry["path"]).resolve()
                directory.relative_to(root.resolve())
                package = tomllib.loads((directory / "Cargo.toml").read_text(encoding="utf-8"))["package"]
                if any(name == package["name"] and version == package["version"] for name, version, _ in visited):
                    directories.append(directory)
    return directories


def consumer_inputs(root):
    """Include all tracked source of every local dependency of the checked package."""
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"], cwd=root, text=True,
        encoding="utf-8"))
    packages = {Path(p["manifest_path"]).resolve().parent: p for p in metadata["packages"]}
    pending = [Path(p["manifest_path"]).resolve().parent for p in packages.values()
               if p["name"] == "transmog-js-runtime"]
    if len(pending) != 1:
        raise ValueError("Expected one native V8 consumer package")
    directories = set()
    while pending:
        directory = pending.pop()
        if directory in directories:
            continue
        directory.relative_to(root.resolve())
        directories.add(directory)
        pending.extend(Path(d["path"]).resolve() for d in packages[directory]["dependencies"] if d.get("path"))
    directories.update(patched_dependencies(root))
    paths = subprocess.check_output(
        ["git", "ls-files", "-z", "--", *INPUTS, *[p.relative_to(root.resolve()).as_posix() for p in directories]],
        cwd=root).decode("utf-8").split("\0")
    names = set(paths) - {""}
    if not set(INPUTS) <= names:
        raise ValueError("Native V8 consumer inputs are missing from the checkout")
    return {name: engine.digest(root / name) for name in sorted(names)}


def context(directory, root=ROOT, *, native=True):
    # Producers may check an existing proof on x64. Only native verification can
    # create a fresh proof; all later application consumers recheck their own SDK.
    rustc = engine.raw_tools()["rustc"].splitlines()
    if native and "host: " + TARGET not in rustc:
        raise ValueError("The V8 consumer check requires native ARM64 Rust")
    if not native and not any("host: " + target in rustc for target in engine.TARGETS):
        raise ValueError("The V8 proof lookup requires a supported Windows Rust host")
    manifest = engine.validate(directory, TARGET, root)
    return {"schema": 1, "command": COMMAND, "manifest": manifest,
            "inputs": consumer_inputs(root), "consumerAbi": engine.abi(TARGET, root)}


def read_report(payload):
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        matches = [p for p in archive.infolist() if p.filename.split("/")[-1] == "v8-native-consumer.json"]
        if len(matches) != 1 or matches[0].file_size > 1024 * 1024:
            raise ValueError("Missing or ambiguous native V8 consumer report")
        return json.loads(archive.read(matches[0]))


def artifact_bytes(api, artifact):
    # Construct the API URL ourselves; a report cannot choose a credential destination.
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "Transmog-release-tools"}
    if os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    with github.urlopen(Request(f"{api}/actions/artifacts/{artifact['id']}/zip", headers=headers), timeout=30) as response:
        payload = response.read(8 * 1024 * 1024 + 1)
    import hashlib
    if len(payload) > 8 * 1024 * 1024 or artifact.get("digest") != "sha256:" + hashlib.sha256(payload).hexdigest():
        raise ValueError("Native V8 consumer artifact digest differs")
    return payload


def verify_origin(api, artifact, report, expected, key, branch):
    origin = artifact["workflow_run"]
    run = github.request(f"{api}/actions/runs/{origin['id']}")
    if (not run or run["head_sha"] != origin["head_sha"] or run["head_branch"] != branch
            or origin["head_branch"] != branch or run["event"] not in {"push", "workflow_dispatch"}
            or run["path"] not in WORKFLOWS or run["head_repository"]["full_name"] != os.environ["GITHUB_REPOSITORY"]):
        raise ValueError("Native V8 consumer workflow origin differs")
    attempt = report["RunAttempt"]
    if (type(attempt) is not int or attempt < 1 or report["Commit"] != run["head_sha"]
            or str(report["RunId"]) != str(run["id"]) or report["Target"] != TARGET
            or report["ValidationMode"] != "fresh" or report["CargoAbiVerified"] is not True
            or report["Context"] != expected or report["ValidationKey"] != key
            or artifact["name"] != f"windows-v8-arm64-consumer-{key}-{attempt}"):
        raise ValueError("Native V8 consumer report context differs")
    jobs = github.request(f"{api}/actions/runs/{run['id']}/attempts/{attempt}/jobs?per_page=100")["jobs"]
    matching = [j for j in jobs if j["name"].split(" / ")[-1] == JOB]
    if len(matching) != 1 or matching[0]["conclusion"] != "success" or matching[0]["head_sha"] != run["head_sha"]:
        raise ValueError("Native V8 consumer job did not succeed")
    return {"ArtifactId": artifact["id"], "ArtifactDigest": artifact["digest"],
            "Commit": report["Commit"], "RunId": report["RunId"], "RunAttempt": attempt, "JobId": matching[0]["id"]}


def reusable(expected, key):
    api = github.api_root(os.environ["GITHUB_REPOSITORY"])
    branch = os.environ["GITHUB_REF_NAME"]
    # Bound the lookup. An expired/old/unavailable report causes a fresh check.
    deadline = time.monotonic() + 30
    for page in range(1, 4):
        if time.monotonic() >= deadline:
            break
        artifacts = github.request(f"{api}/actions/artifacts?per_page=100&page={page}")["artifacts"]
        for artifact in artifacts:
            if time.monotonic() >= deadline:
                return None
            if (artifact["expired"] or not re.fullmatch(f"windows-v8-arm64-consumer-{key}-[1-9][0-9]*", artifact["name"])
                    or artifact["size_in_bytes"] > 8 * 1024 * 1024):
                continue
            try:
                report = read_report(artifact_bytes(api, artifact))
                if report.get("ValidationMode") != "fresh":
                    continue  # Reuse the original proof, not an unbounded chain of reports.
                return verify_origin(api, artifact, report, expected, key, branch)
            except (ValueError, KeyError, TypeError, RuntimeError, URLError, zipfile.BadZipFile):
                print("Prior native V8 report is not reusable; checking other evidence.", flush=True)
        if len(artifacts) < 100:
            break
    return None


def verify(directory, report_path, *, use_cache=True, root=ROOT, probe=False):
    report_path = Path(report_path)
    report_path.unlink(missing_ok=True)
    expected = context(directory, root, native=not probe)
    key = engine.encoded_digest(expected)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with Path(output).open("a", encoding="utf-8") as stream:
            stream.write(f"key={key}\n")
    origin = None
    if use_cache:
        try:
            origin = reusable(expected, key)
        except (RuntimeError, URLError, TimeoutError, KeyError, TypeError, ValueError):
            print("Prior native V8 evidence is unavailable; fresh native verification is required.", flush=True)
    if probe:
        if output := os.environ.get("GITHUB_OUTPUT"):
            with Path(output).open("a", encoding="utf-8") as stream:
                stream.write(f"reused-arm64={str(origin is not None).lower()}\n")
        if origin is None:
            print("ARM64 V8: no matching native proof; the native verification job is required.", flush=True)
            return None
    if origin is None:
        environment = dict(os.environ, TRANSMOG_V8_ARTIFACT=str(Path(directory).resolve()))
        subprocess.run(COMMAND, cwd=root, env=environment, check=True)
    report = {"Commit": os.environ["GITHUB_SHA"], "RunId": os.environ["GITHUB_RUN_ID"],
              "RunAttempt": int(os.environ["GITHUB_RUN_ATTEMPT"]), "Target": TARGET,
              "ArtifactId": os.environ["V8_ARTIFACT_ID"], "EngineKey": expected["manifest"]["key"],
              "CargoAbiVerified": True, "ValidationKey": key, "Context": expected,
              "ValidationMode": "reused" if origin else "fresh", "ValidationOrigin": origin,
              "NativeRunnerSkipped": probe}
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    message = (f"reused successful native Cargo check from run {origin['RunId']}, artifact {origin['ArtifactId']}"
               if origin else "fresh native Cargo check passed")
    suffix = ("; native verification runner skipped; producer engine hashes, SDK and consumer context verified."
              if probe else "; current engine hashes, SDK and consumer context verified.")
    print("ARM64 V8: " + message + suffix, flush=True)
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(summary).open("a", encoding="utf-8") as stream:
            stream.write("\n### ARM64 V8 consumer: " + message + suffix + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--no-reuse", action="store_true")
    parser.add_argument("--probe", action="store_true", help="Verify an existing proof on a producer; never run Cargo")
    args = parser.parse_args()
    verify(args.directory, args.report, use_cache=not args.no_reuse, probe=args.probe)


if __name__ == "__main__":
    main_guard(main)
