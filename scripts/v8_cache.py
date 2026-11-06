"""Leases and opt-in retention for owned V8 native outputs and source caches."""
from contextlib import contextmanager, ExitStack
import hashlib
import json
from pathlib import Path
import re
import time

from build_cache import (PIN, cargo_locks, checked, file_lock, measure, read_json,
                         recover_trash, registered_targets, related_profiles,
                         remove_variant, tree_files)

LEASE = ".tools/rusty-v8/cache.fslock"
VERSION = r"\d+\.\d+\.\d+"
TRIPLE = r"[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}"


@contextmanager
def cache_lease(root):
    """Hold the same first-byte/whole-file OS lock as the native build script."""
    path = checked(root / LEASE, root)
    path.parent.mkdir(parents=True, exist_ok=True)
    while True:
        with file_lock(path, root) as acquired:
            if acquired:
                yield
                return
        time.sleep(0.1)


def source_name(version, inputs):
    identity = hashlib.sha256(json.dumps(inputs, sort_keys=True).encode("utf-8")).hexdigest()[:16]
    return f"{version}-{identity}"


def current_source(root):
    vendor = root / "third_party/rusty-v8"
    pin = read_json(vendor / "source.json", root)
    inputs = {"format": 1, "archiveSha256": pin["archive_sha256"],
              "nativePatchSha256": hashlib.sha256(checked(vendor / "native.patch", root).read_bytes()).hexdigest(),
              "icuDataSha256": pin["icuData"]["filteredSha256"]}
    return source_name(pin["version"], inputs)


def recognized_source(path, root, *, legacy):
    match = re.fullmatch(f"({VERSION})-([0-9a-f]{{{12 if legacy else 16}}})", path.name)
    if not match or not path.is_dir():
        return False
    receipt = read_json(path / ".transmog-source.json", root)
    digest = receipt.get("archive_sha256", "")
    if not re.fullmatch(r"[0-9a-f]{64}", digest) or not isinstance(receipt.get("files"), dict) or not receipt["files"]:
        return False
    if legacy:
        return digest.startswith(match[2])
    inputs = read_json(path / ".transmog-inputs.json", root)
    return (inputs.get("format") == 1 and inputs.get("archiveSha256") == digest
            and all(re.fullmatch(r"[0-9a-f]{64}", inputs.get(key, ""))
                    for key in ("nativePatchSha256", "icuDataSha256"))
            and source_name(match[1], inputs) == path.name)


def recognized_native(path, root):
    if not re.fullmatch(r"[0-9a-f]{16}", path.name) or not path.is_dir():
        return False
    if not all(checked(path / name, root).is_file() for name in
               ("v8.fslock", "gn_out/args.gn", "gn_out/build.ninja", "gn_out/project.json")):
        return False
    project = read_json(path / "gn_out/project.json", root)
    return "//:rusty_v8" in project.get("targets", {})


def pinned_scope(path, root):
    return any(checked(parent / PIN, root).exists() for parent in (path, *path.parents)
               if parent == root or parent.is_relative_to(root))


def maintain_v8(root, *, apply=False, deep=True, include_release=False):
    root = root.resolve()
    report = {"status": "applied" if apply else "preview", "candidates": [], "retained": [],
              "errors": [], "logicalBytes": 0, "uniqueBytes": 0,
              "reclaimableBytesEstimate": 0, "removedLogicalBytes": 0}
    cache = checked(root / ".tools/rusty-v8", root)
    targets = registered_targets(root)
    native = [checked(target / "native-v8", root) for target in targets]
    if not cache.is_dir() and not any(path.is_dir() for path in native):
        return report
    if not include_release:
        report["retained"].append({"paths": [str(p.relative_to(root)) for p in [cache, *native] if p.is_dir()],
                                    "reason": "shared production V8 cleanup requires --include-release"})
        return report
    # Cargo owns the native archive while rustc bundles it after build.rs exits.
    # The cache lease protects preparation, including standalone --prepare-only.
    with ExitStack() as stack:
        cache.mkdir(parents=True, exist_ok=True)
        if not stack.enter_context(file_lock(root / LEASE, root)):
            report["status"] = "busy"
            return report
        for target in targets:
            for profile in related_profiles(target, root):
                if not stack.enter_context(cargo_locks(profile, root)):
                    report.update(status="busy", busyProfile=str(profile.relative_to(root)))
                    return report
        tools = checked(cache / "tools", root)
        if tools.is_dir():
            for host in tools.iterdir():
                if not re.fullmatch(TRIPLE, host.name) or not host.is_dir():
                    continue
                lock = checked(host / "toolchain.fslock", root)
                if lock.is_file() and not stack.enter_context(file_lock(lock, root)):
                    report["status"] = "busy"
                    return report
        try:
            current = current_source(root)
        except (OSError, ValueError, KeyError, TypeError):
            # Without the checkout pin, do not guess which sources are current.
            current = None
        parents = [(cache, "source", True), (checked(cache / "sources", root), "source", False)]
        for parent in native:
            if parent.is_dir():
                for triple in parent.iterdir():
                    if re.fullmatch(TRIPLE, triple.name) and checked(triple, root).is_dir():
                        parents.append((triple, str(triple.relative_to(root)), False))
        groups = {}
        current_retained = False
        for parent, group, legacy in parents:
            if not parent.is_dir():
                continue
            if pinned_scope(parent, root):
                report["retained"].append({"path": str(parent.relative_to(root)), "reason": "pinned"})
                continue
            if apply:
                recovered, errors = recover_trash(parent, root)
                report["removedLogicalBytes"] += recovered
                report["errors"].extend(errors)
            for path in parent.iterdir():
                if path.name in {"sources", "tools", "cache.fslock", ".transmog-cache-trash"}:
                    continue
                relative = str(path.relative_to(root))
                try:
                    checked(path, root)
                    recognized = recognized_source(path, root, legacy=legacy) if group == "source" else recognized_native(path, root)
                    if not recognized:
                        report["retained"].append({"path": relative, "reason": "unknown layout"})
                        continue
                    files = tree_files(path, root)
                    if pinned_scope(path, root) or any(p.name == PIN for p in files):
                        report["retained"].append({"path": relative, "reason": "pinned"})
                        continue
                    if group == "source" and (current is None or path.name == current):
                        current_retained |= path.name == current
                        report["retained"].append({"path": relative, "reason": "current or unverified source pin"})
                        continue
                    newest = max([path.stat().st_mtime, *[p.stat().st_mtime for p in files]])
                    groups.setdefault(group, []).append((newest, path, parent))
                except (OSError, ValueError, TypeError, AttributeError, RecursionError) as error:
                    report["retained"].append({"path": relative, "reason": "unreadable or unsafe", "detail": str(error)})
        selected = []
        for group, choices in groups.items():
            # The current prepared source is already retained. Legacy source
            # copies count in the same budget, rather than each keeping one.
            keep = (1 if deep else 3) - int(group == "source" and current_retained)
            for index, (newest, path, parent) in enumerate(sorted(choices, reverse=True)):
                if index < keep or not deep and newest > time.time() - 86400:
                    report["retained"].append({"path": str(path.relative_to(root)), "reason": "retention or recent use"})
                    continue
                sizes = measure([path], root)
                report["candidates"].append({"path": str(path.relative_to(root)), **sizes})
                selected.append((path, parent))
        report.update(measure([path for path, _ in selected], root))
        for path, parent in selected:
            if apply:
                try:
                    report["removedLogicalBytes"] += remove_variant([path], parent, root)
                except (OSError, ValueError) as error:
                    report["errors"].append(str(error))
    return report
