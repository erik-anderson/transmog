"""Resolved TLS source/feature checks shared by build, roll and provenance tools."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import parse_qs, urlsplit

REPOSITORY = "https://github.com/cloudflare/boring"
FAMILY = ("boring", "boring-sys", "hyper-boring", "tokio-boring")
UNSUPPORTED_FEATURES = {"fips", "rpk", "underscore-wildcards", "allow-crl-extensions-bad-version", "relax-cert-validation"}
OVERRIDES = ("PATH", "INCLUDE_PATH", "SOURCE_PATH", "ASSUME_PATCHED")


def reject_native_overrides(environment):
    for name in environment:
        suffix = name.upper().split("BORING_BSSL_", 1)
        if len(suffix) == 2:
            value = suffix[1].removeprefix("FIPS_")
            if any(value == option or value.startswith(option + "_") for option in OVERRIDES):
                raise ValueError(f"{name} can replace the qualified native TLS library; remove this override")


def baseline(root: Path):
    receipt = json.loads((root / "third_party/boringssl-pki/manifest.json").read_text(encoding="utf-8"))
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    revision = receipt["boring_revision"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or not re.fullmatch(r"[0-9a-f]{40}", receipt["revision"]):
        raise ValueError("TLS receipt requires full immutable source revisions")
    versions = set()
    for name in FAMILY:
        pin = cargo["patch"]["crates-io"][name]
        if pin.get("git") != REPOSITORY or pin.get("rev") != revision:
            raise ValueError(f"{name} patch disagrees with the PKI source receipt")
        dependency = cargo["workspace"]["dependencies"][name]
        version = dependency["version"] if isinstance(dependency, dict) else dependency
        if not version.startswith("="):
            raise ValueError(f"{name} must have an exact version")
        versions.add(version[1:])
    if len(versions) != 1:
        raise ValueError("The Boring crate family has inconsistent versions")
    version = versions.pop()
    if receipt.get("boring_version") != version:
        raise ValueError("Boring package version disagrees with the PKI receipt")
    return {"repository": REPOSITORY, "revision": revision,
            "version": version, "boringssl_revision": receipt["revision"]}


def git(source: Path, *arguments):
    return subprocess.check_output(["git", "-C", str(source), *arguments], text=True, encoding="utf-8").strip()


def verify_source(source: Path, expected):
    if git(source, "rev-parse", "HEAD") != expected["revision"]:
        raise ValueError("Resolved Boring checkout has the wrong revision")
    dirty = git(source, "status", "--porcelain", "--untracked-files=all")
    if any(line != "?? .cargo-ok" for line in dirty.splitlines()):
        raise ValueError("Resolved Boring checkout must be clean")
    native = source / "boring-sys/deps/boringssl"
    link = git(source, "ls-tree", "HEAD", "boring-sys/deps/boringssl").split()
    if link[:3] != ["160000", "commit", expected["boringssl_revision"]]:
        raise ValueError("BoringSSL gitlink disagrees with the PKI receipt")
    if git(native, "rev-parse", "HEAD") != expected["boringssl_revision"]:
        raise ValueError("Resolved BoringSSL submodule has the wrong revision")
    dirty = git(native, "status", "--porcelain", "--untracked-files=all")
    if any(line != "?? .cargo-ok" for line in dirty.splitlines()):
        raise ValueError("Resolved BoringSSL submodule must be clean")
    patches = source / "boring-sys/patches"
    return {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(patches.glob("*.patch"))}


def verify_stack(metadata, root: Path, *, verify_checkout=True):
    expected = baseline(root)
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    selected = {}
    for package in metadata["packages"]:
        if package["name"] not in FAMILY:
            continue
        if package["name"] in selected:
            raise ValueError(f"Multiple resolved {package['name']} sources")
        source = package.get("source", "") or ""
        parsed = urlsplit(source.removeprefix("git+"))
        repository = f"{parsed.scheme}://{parsed.netloc}{parsed.path}"
        if not source.startswith("git+") or repository != expected["repository"] or parsed.fragment != expected["revision"] or parse_qs(parsed.query) != {"rev": [expected["revision"]]}:
            raise ValueError(f"Resolved {package['name']} does not use the qualified Git revision")
        if package["version"] != expected["version"]:
            raise ValueError(f"Resolved {package['name']} has the wrong package version")
        forbidden = set(nodes[package["id"]]["features"]) & UNSUPPORTED_FEATURES
        if forbidden:
            raise ValueError(f"Unqualified {package['name']} features: {sorted(forbidden)}")
        selected[package["name"]] = package
    if not {"boring", "boring-sys"}.issubset(selected):
        raise ValueError("The resolved graph is missing the qualified TLS provider")
    for package in metadata["packages"]:
        if package["name"] == "quiche":
            features = set(nodes[package["id"]]["features"])
            if "boringssl-boring-crate" not in features or any(f.startswith("boringssl-") and f != "boringssl-boring-crate" for f in features):
                raise ValueError("quiche must use only the shared Boring crate provider")
    source = Path(selected["boring-sys"]["manifest_path"]).parent.parent
    patches = verify_source(source, expected) if verify_checkout else {}
    return {**expected, "source": str(source), "patches": patches,
            "features": {name: nodes[p["id"]]["features"] for name, p in selected.items()}}


def preflight(cargo, arguments, cwd, environment):
    """Validate the actual configured Cargo graph before a portable build."""
    reject_native_overrides(environment)
    root = Path(cwd).resolve()
    manifest = root / "Cargo.toml"
    forwarded = []
    flags = {"--manifest-path", "--features", "-F", "--target", "--config"}
    switches = {"--all-features", "--no-default-features", "--offline", "--frozen", "--locked"}
    targets = []
    iterator = iter(arguments)
    toolchain = []
    for argument in iterator:
        if argument == "--":
            break
        if argument.startswith("+"):
            toolchain = [argument]
        elif argument in flags:
            value = next(iterator)
            if argument == "--target":
                targets.append(value)
            else:
                forwarded.extend(("--features" if argument == "-F" else argument, value))
            if argument == "--manifest-path":
                manifest = (root / value).resolve()
        elif argument.split("=", 1)[0] in flags and "=" in argument:
            if argument.startswith("--target="):
                targets.append(argument.split("=", 1)[1])
            else:
                forwarded.append(argument)
            if argument.startswith("--manifest-path="):
                manifest = (root / argument.split("=", 1)[1]).resolve()
        elif argument in switches:
            forwarded.append(argument)
        elif argument.startswith("-F") and len(argument) > 2:
            forwarded.extend(("--features", argument[2:]))
    for target in targets or [None]:
        platform = ["--filter-platform", target] if target else []
        result = subprocess.run([cargo, *toolchain, "metadata", "--format-version", "1", *forwarded, *platform],
                                cwd=cwd, env=environment, check=True, capture_output=True,
                                text=True, encoding="utf-8")
        metadata = json.loads(result.stdout)
        native = [p for p in metadata["packages"] if p["name"] == "transmog-tls-native"]
        if len(native) > 1:
            raise ValueError("Multiple native verifier instances would share incompatible C symbols")
        if native:
            # Also protects independent embedding workspaces: use the dependency's
            # qualified receipt, with the consumer's actual resolved graph.
            owner = Path(native[0]["manifest_path"]).parent.parent.parent
            verify_stack(metadata, owner)
