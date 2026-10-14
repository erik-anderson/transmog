"""Check the current baseline, or prepare an isolated, reviewable dependency roll.

Example: --boring-source PATH --chromium-version FOUR.PART.VERSION --output-dir
.local/dependency-rolls/candidate. The source workspace must be clean. The
candidate does not change profile recipes or publish/commit any changes.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys
import tomllib
import urllib.request

from repo_tools import ROOT, command, development_environment, main_guard
from tls_dependencies import FAMILY, REPOSITORY, git, reject_native_overrides, verify_stack

ROOT_INPUTS = {"root_store.proto": "net/cert/root_store.proto",
               "root_store.textproto": "net/data/ssl/chrome_root_store/root_store.textproto",
               "root_store.certs": "net/data/ssl/chrome_root_store/root_store.certs",
               "additional.certs": "net/data/ssl/chrome_root_store/additional.certs", "LICENSE": "LICENSE"}


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), ROOT / "scripts" / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def update_cargo(text, version, revision):
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or not re.fullmatch(r"\d+\.\d+\.\d+(?:-[\w.-]+)?", version):
        raise ValueError("Invalid immutable Boring revision or package version")
    section, changed, lines = "", set(), []
    for line in text.splitlines(keepends=True):
        if line.lstrip().startswith("["):
            section = line.strip()
        for name in FAMILY:
            if re.match(rf"^{name}\s*=", line):
                if section == "[workspace.dependencies]":
                    line, count = re.subn(r'"=[^"\n]+"', '"=' + version + '"', line, count=1)
                elif section == "[patch.crates-io]":
                    line, count = re.subn(r'rev\s*=\s*"[0-9a-f]{40}"', 'rev = "' + revision + '"', line, count=1)
                else:
                    continue
                if count != 1:
                    raise ValueError(f"Could not update {name} in {section}")
                changed.add((section, name))
        lines.append(line)
    if len(changed) != len(FAMILY) * 2:
        raise ValueError("Cargo manifest is missing a Boring family pin")
    return "".join(lines)


def fetch_chromium(revision, path):
    url = f"https://chromium.googlesource.com/chromium/src/+/{revision}/{path}?format=TEXT"
    with urllib.request.urlopen(url, timeout=30) as response:
        encoded = response.read(8 * 1024 * 1024 + 1)
    if len(encoded) > 8 * 1024 * 1024:
        raise ValueError(f"Chromium input exceeds its download bound: {path}")
    data = base64.b64decode(encoded, validate=True)
    if not data or len(data) > 4 * 1024 * 1024:
        raise ValueError(f"Chromium input is empty or oversized: {path}")
    return data


def check(root, environment):
    reject_native_overrides(environment)
    load("vendor-boringssl-pki").check_snapshot(root / "third_party/boringssl-pki")
    for name in ("update-chromium-psl", "update-chromium-certificate-policy"):
        command([sys.executable, root / "scripts" / (name + ".py"), "--check"], cwd=root, env=environment)
    receipt = json.loads((root / "third_party/chromium-root-store/snapshot.json").read_text(encoding="utf-8"))
    data = root / "third_party/chromium-root-store"
    for name, digest in receipt["inputSha256"].items():
        if name not in ROOT_INPUTS or hashlib.sha256((data / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"Root-store input mismatch: {name}")
    if set(receipt["inputSha256"]) != set(ROOT_INPUTS) or hashlib.sha256((data / "crs.pb").read_bytes()).hexdigest() != receipt["protobufSha256"]:
        raise ValueError("Root-store receipt is incomplete or the protobuf differs")
    checker = load("check-crypto-graph")
    information = None
    for target in checker.TARGETS:
        metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked", "--all-features",
                                      "--manifest-path", root / "Cargo.toml", "--filter-platform", target], cwd=root, env=environment, capture=True).stdout)
        checker.check_graph(metadata, target)
        information = verify_stack(metadata, root)
    return information


def export_candidate(candidate, environment):
    # A PKI roll can introduce new headers and remove obsolete ones. An
    # unstaged diff silently omits new paths, producing an incomplete roll.
    command(["git", "add", "--all", "--", "."], cwd=candidate, env=environment)
    # The validated PKI snapshot may contain new files matching broad ignores
    # such as *.pem. Include the entire checked snapshot, regardless of those.
    if (candidate / "third_party/boringssl-pki").is_dir():
        command(["git", "add", "--all", "--force", "--", "third_party/boringssl-pki"], cwd=candidate, env=environment)
    changed = command(["git", "diff", "--cached", "--stat"], cwd=candidate, env=environment, capture=True).stdout
    patch = command(["git", "diff", "--cached", "--binary"], cwd=candidate, env=environment, capture=True).stdout
    return changed, patch


def prepare(args, environment):
    if git(ROOT, "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("Commit or stash workspace changes before preparing a dependency roll")
    output = args.output_dir.absolute()
    if output.exists() or output.resolve() != output or output == ROOT or ROOT.is_relative_to(output):
        raise ValueError("Use a new output directory with no links, outside the workspace's parents")
    output.mkdir(parents=True)
    candidate = output / "candidate"
    command(["git", "clone", "--quiet", "--no-hardlinks", ROOT, candidate], env=environment)
    # Do not allow the review checkout to push to the source workspace.
    command(["git", "remote", "remove", "origin"], cwd=candidate, env=environment)
    before = check(ROOT, environment)
    source = args.boring_source
    if args.boring_revision:
        source = output / "boring-source"
        command(["git", "clone", "--quiet", "--filter=blob:none", "--no-checkout", REPOSITORY, source], env=environment)
        command(["git", "checkout", "--quiet", "--detach", args.boring_revision], cwd=source, env=environment)
        command(["git", "submodule", "update", "--init", "--recursive"], cwd=source, env=environment)
    if source is not None:
        source = source.resolve(strict=True)
        revision = git(source, "rev-parse", "HEAD")
        workspace = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
        version = workspace["workspace"]["package"]["version"]
        native_revision = git(source, "ls-tree", "HEAD", "boring-sys/deps/boringssl").split()[2]
        from tls_dependencies import verify_source
        verify_source(source, {"revision": revision, "boringssl_revision": native_revision})
        manifest = candidate / "Cargo.toml"
        manifest.write_text(update_cargo(manifest.read_text(encoding="utf-8"), version, revision), encoding="utf-8", newline="\n")
        load("vendor-boringssl-pki").generate_snapshot(source / "boring-sys/deps/boringssl", candidate / "third_party/boringssl-pki", native_revision, revision, version)
        if revision != before["revision"] or version != before["version"]:
            command(["cargo", "update", "--manifest-path", manifest, *sum((["-p", name] for name in FAMILY), [])], cwd=candidate, env=environment)
    if args.chromium_version:
        for name in ("update-chromium-psl", "update-chromium-certificate-policy"):
            command([sys.executable, candidate / "scripts" / (name + ".py"), "--chromium-version", args.chromium_version], cwd=candidate, env=environment)
    if args.root_store_revision:
        data = candidate / "third_party/chromium-root-store"
        inputs = {name: fetch_chromium(args.root_store_revision, path) for name, path in ROOT_INPUTS.items()}
        for name, value in inputs.items():
            (data / name).write_bytes(value)
        load("generate-chromium-root-store").generate_snapshot(data, args.root_store_revision)
    after = check(candidate, environment)
    changed, patch = export_candidate(candidate, environment)
    receipts = {name: json.loads((candidate / "third_party" / name / "snapshot.json").read_text(encoding="utf-8"))
                for name in ("chromium-suffix-list", "chromium-certificate-policy", "chromium-root-store")}
    report = {"sourceCommit": git(ROOT, "rev-parse", "HEAD"), "candidate": str(candidate),
              "before": before, "after": after, "chromiumDataVersion": args.chromium_version,
              "rootStoreRevision": args.root_store_revision, "profileRecipesChanged": False,
              "dataReceipts": receipts,
              "patchChanges": {name: {"before": before["patches"].get(name), "after": after["patches"].get(name)}
                               for name in sorted(before["patches"].keys() | after["patches"].keys())
                               if before["patches"].get(name) != after["patches"].get(name)},
              "qualificationRequired": ["Review upstream API, patch and verifier policy changes, schemas and version-specific source labels",
                                         "Formatting, strict Clippy and workspace tests",
                                         "TLS fingerprint, ECH/ALPS, client-auth and Docker interoperability checks"],
              "changedFiles": changed}
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8", newline="\n")
    (output / "changes.patch").write_text(patch, encoding="utf-8", newline="\n")
    print(f"Prepared {candidate}; review report.json and changes.patch, then qualify the candidate before applying its changes.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Check current sources and all target graphs")
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--boring-source", type=Path, help="Clean upstream Boring checkout, including its submodule")
    selection.add_argument("--boring-revision", help="Fetch one immutable upstream Boring Git revision")
    parser.add_argument("--chromium-version", help="Roll PSL and certificate policy data together")
    parser.add_argument("--root-store-revision", help="Fetch all root-store inputs from one immutable Chromium revision")
    parser.add_argument("--output-dir", type=Path, help="New isolated candidate and review report directory")
    args = parser.parse_args()
    if args.chromium_version and not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", args.chromium_version):
        parser.error("Use an exact four-part Chromium release")
    for revision in (args.boring_revision, args.root_store_revision):
        if revision and not re.fullmatch(r"[0-9a-f]{40}", revision):
            parser.error("Use full immutable Git revisions")
    updating = bool(args.boring_source or args.boring_revision or args.chromium_version or args.root_store_revision)
    if args.check and (updating or args.output_dir) or not args.check and (not updating or not args.output_dir):
        parser.error("Use --check alone, or select roll inputs and a new --output-dir")
    environment = development_environment()
    if args.check:
        check(ROOT, environment)
        print("Verified current TLS/native and Chromium data sources on all supported targets")
    else:
        prepare(args, environment)


if __name__ == "__main__":
    main_guard(main)
