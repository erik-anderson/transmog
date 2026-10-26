"""Conservative maintenance of complete Cargo development artifact variants."""
from __future__ import annotations

from collections import defaultdict
from contextlib import contextmanager, ExitStack
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import time
import uuid

DAY = 24 * 60 * 60
KEEP = 3
LOCKS = (".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock")
HASHED = re.compile(r"(.+)-([0-9a-f]{16})(\..+)?")
FINGERPRINT = re.compile(r"([A-Za-z0-9_-]+)-([0-9a-f]{16})")
INCREMENTAL = re.compile(r"([A-Za-z0-9_]+)-[a-z0-9]+")
SUFFIXES = {"", ".rlib", ".rmeta", ".pdb", ".exe", ".dll", ".dll.lib", ".dll.exp",
            ".so", ".dylib", ".lib", ".a", ".d", ".exp", ".ilk", ".dwp"}
REGISTERED_TARGETS = ("target", "sdk/target", "fuzz/target", ".local/sdk/rust-target",
                      ".local/sdk/model-codegen-target", ".tools/sdk-crubit-target")
PIN = ".transmog-cache-pin"
USAGE = ".transmog-cache-use.json"


def read_json(path: Path, root: Path):
    checked(path, root)
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError(f"Cache metadata is too large: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value, root: Path):
    temp = checked(path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp"), root)
    try:
        temp.write_text(json.dumps(value) + "\n", encoding="utf-8")
        temp.replace(checked(path, root))
    finally:
        temp.unlink(missing_ok=True)


def checked(path: Path, root: Path) -> Path:
    """Keep every mutation inside the named root, without traversing links."""
    path = path.absolute()
    root = root.resolve()
    # absolute() preserves '..'. Reject it before the lexical containment and
    # link checks, rather than resolving away a link followed by a parent hop.
    if ".." in path.parts:
        raise ValueError(f"Cache paths with parent traversal are not supported: {path}")
    if path == root or not path.is_relative_to(root):
        raise ValueError(f"Cache path is outside its owning directory: {path}")
    node = root
    for part in path.relative_to(root).parts:
        node /= part
        try:
            info = node.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError(f"Cache links and reparse points are not supported: {node}")
    return path


def tree_files(path: Path, root: Path) -> list[Path]:
    checked(path, root)
    if path.is_file():
        return [path]
    files = []
    def failed(error):
        raise error
    for parent, directories, names in os.walk(path, followlinks=False, onerror=failed):
        for name in directories + names:
            checked(Path(parent) / name, root)
        files.extend(Path(parent) / name for name in names)
    return files


def measure(paths: list[Path], root: Path) -> dict:
    """Count hardlinks once, and exclude content with links outside this plan.

    These are file-content sizes, not allocated clusters. A free-space delta is
    reported separately because concurrent disk activity makes it observational.
    Use stat(), not scandir's cached Windows stat (which has zero file IDs).
    """
    identities = {}
    logical = 0
    for path in paths:
        for file in tree_files(path, root):
            info = file.stat()
            logical += info.st_size
            key = (info.st_dev, info.st_ino) if info.st_ino else str(file)
            row = identities.setdefault(key, [info.st_size, info.st_nlink, 0])
            row[2] += 1
    return {"logicalBytes": logical, "uniqueBytes": sum(row[0] for row in identities.values()),
            "reclaimableBytesEstimate": sum(row[0] for row in identities.values() if row[2] >= row[1])}


@contextmanager
def file_lock(path: Path, root: Path):
    handle = checked(path, root).open("a+b")
    acquired = False
    try:
        handle.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            acquired = True
        except OSError:
            pass
        yield acquired
    finally:
        try:
            if acquired:
                handle.seek(0)
                if os.name == "nt":
                    import msvcrt
                    msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        finally:
            handle.close()


def related_profiles(target: Path, root: Path) -> list[Path]:
    """Only inspect Cargo layouts directly below a target root or target triple."""
    result = []
    if not target.is_dir():
        return result
    parents = [target]
    for child in target.iterdir():
        if re.fullmatch(r"[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}", child.name):
            checked(child, root)
            if child.is_dir():
                parents.append(child)
    for parent in parents:
        for name in ("debug", "release"):
            profile = checked(parent / name, root)
            if (profile / "deps").is_dir() and (profile / ".fingerprint").is_dir():
                result.append(profile)
    return sorted(result)


def successful_uses(profile: Path, root: Path) -> dict:
    try:
        data = read_json(profile / USAGE, root)
        if data.get("schema") == 1 and isinstance(data.get("units"), dict):
            return data["units"]
    except (OSError, ValueError, AttributeError, RecursionError):
        pass
    return {}


def last_use(directory: Path, uses: dict, root: Path) -> float:
    """Only trust use records whose fingerprint still matches this unit."""
    try:
        row = uses[directory.name]
        when = row["lastSuccessfulUse"]
        if not isinstance(when, (int, float)) or not math.isfinite(when):
            return 0
        fingerprints = [p for p in directory.iterdir() if p.name == row["stem"]]
        if (len(fingerprints) == 1 and checked(fingerprints[0], root).stat().st_size == 16
                and fingerprints[0].read_text() == row["fingerprint"]):
            return when
    except (KeyError, OSError, TypeError, ValueError):
        pass
    return 0


@contextmanager
def cargo_locks(profile: Path, root: Path):
    """Try both legacy and current Cargo locks, never waiting for a build.

    Cargo's layout retains the legacy shared lock for compatibility:
    https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/compiler/layout.rs.html
    An exclusive lock on its first byte conflicts with Cargo's Windows range.
    """
    with ExitStack() as stack:
        yield all(stack.enter_context(file_lock(profile / name, root)) for name in LOCKS)


def variants(profile: Path, root: Path, retained=None):
    """Only pair recognized files with one valid Cargo fingerprint directory."""
    by_hash = defaultdict(list)
    for path in checked(profile / "deps", root).iterdir():
        match = HASHED.fullmatch(path.name)
        if match:
            by_hash[match[2]].append(path)
    groups = defaultdict(list)
    target_owners = defaultdict(set)
    uses = successful_uses(profile, root)

    def skipped(path, reason):
        if retained is not None:
            retained.append({"paths": [str(path.relative_to(root))], "reason": reason})

    for directory in checked(profile / ".fingerprint", root).iterdir():
        match = FINGERPRINT.fullmatch(directory.name)
        if not match or not directory.is_dir():
            skipped(directory, "unknown fingerprint layout")
            continue
        try:
            checked(directory, root)
            metadata = list(directory.glob("*.json"))
            if len(metadata) != 1 or metadata[0].stat().st_size > 1024 * 1024:
                skipped(directory, "unknown fingerprint metadata")
                continue
            checked(metadata[0], root)
            data = json.loads(metadata[0].read_text(encoding="utf-8"))
            if not isinstance(data, dict) or not all(
                key in data for key in ("rustc", "target", "profile", "deps")
            ):
                skipped(directory, "unknown fingerprint metadata")
                continue
            stem = metadata[0].stem
            if stem.startswith(("build-script-", "run-build-script-")):
                continue
            artifacts = by_hash.get(match[2], [])
            if not artifacts or any((HASHED.fullmatch(p.name)[3] or "") not in SUFFIXES
                                    or not p.is_file() for p in artifacts):
                skipped(directory, "unknown or unmatched artifact layout")
                continue
            files = tree_files(directory, root)
            for artifact in artifacts:
                checked(artifact, root)
            newest = max(last_use(directory, uses, root), *[p.stat().st_mtime for p in [*files, *artifacts]])
            # Metadata-only checks must not displace the retained rlib/PDB
            # builds merely because `cargo check` ran more recently.
            linked = any((HASHED.fullmatch(p.name)[3] or "") in
                         {"", ".rlib", ".pdb", ".exe", ".dll", ".so", ".dylib", ".a", ".lib"}
                         for p in artifacts)
            key = (match[1], stem, "linked" if linked else "metadata")
            groups[key].append((newest, [*artifacts, directory]))
            # Ambiguous test/build-script target names are not grouped together
            # when identifying independently hashed incremental caches.
            prefixes = ("lib-", "bin-", "test-lib-", "test-bin-", "test-integration-test-",
                        "example-", "test-example-", "bench-", "test-bench-")
            dep_name = next((stem[len(prefix):] for prefix in prefixes if stem.startswith(prefix)), None)
            if dep_name:
                target_owners[dep_name.replace("-", "_")].add(match[1])
        except (OSError, ValueError, RecursionError):
            skipped(directory, "unreadable or unsafe fingerprint")
            continue
    incremental = checked(profile / "incremental", root)
    if incremental.is_dir():
        for directory in incremental.iterdir():
            match = INCREMENTAL.fullmatch(directory.name)
            if not match or not directory.is_dir() or len(target_owners[match[1]]) != 1:
                continue
            try:
                checked(directory, root)
                sessions = list(directory.iterdir())
                newest = max([directory.stat().st_mtime, *[p.lstat().st_mtime for p in sessions]])
                groups[(match[1], "incremental")].append((newest, [directory]))
            except (OSError, ValueError):
                continue
    return groups


def remove_variant(paths: list[Path], profile: Path, root: Path) -> int:
    """Move the complete group before deletion; roll back a failed move.

    Move fingerprints first: an interrupted trim must force recompilation.
    A loaded Windows binary can refuse a move; rollback restores metadata last.
    """
    files = [file for path in paths for file in tree_files(path, root)]
    size = sum(file.stat().st_size for file in files)
    trash = checked(profile / ".transmog-cache-trash" / uuid.uuid4().hex, root)
    trash.mkdir(parents=True)
    moved = []
    try:
        ordered = sorted(paths, key=lambda item: (item.parent.name != ".fingerprint", not item.is_dir()))
        # Build directories and their fingerprint directories have the same name.
        # Separate destinations also make interrupted staging recoverable.
        destinations = [trash / str(index) / path.name for index, path in enumerate(ordered)]
        write_json(trash / "manifest.json", {"schema": 1, "paths": [str(p.relative_to(root)) for p in ordered]}, root)
        for path, destination in zip(ordered, destinations):
            checked(path, root)
            checked(destination, root).parent.mkdir()
            path.rename(destination)
            moved.append((path, destination))
    except (OSError, ValueError):
        for original, destination in reversed(moved):
            checked(destination, root).rename(checked(original, root))
        shutil.rmtree(trash)
        raise
    tree_files(trash, root)
    shutil.rmtree(checked(trash, root))
    return size


def recover_trash(profile: Path, root: Path) -> tuple[int, list[str]]:
    """Finish deleting safely staged groups while the owning lease is held."""
    trash = checked(profile / ".transmog-cache-trash", root)
    size = 0
    errors = []
    if trash.is_dir():
        for directory in trash.iterdir():
            if re.fullmatch(r"[0-9a-f]{32}", directory.name) and directory.is_dir():
                try:
                    files = tree_files(directory, root)
                    item_size = sum(file.stat().st_size for file in files)
                    shutil.rmtree(checked(directory, root))
                    size += item_size
                except (OSError, ValueError) as error:
                    errors.append(str(error))
    return size, errors


def build_script_variants(profile: Path, profiles: list[Path], root: Path):
    """Index Cargo's full fingerprint graph before considering native outputs.

    Cargo serializes the fingerprint file as little-endian hexadecimal; JSON
    dependency records contain the same u64. Equal/ambiguous hashes retain all
    matching units. Unknown nonempty layouts disable this optional tier.
    """
    nodes = {}
    hashes = defaultdict(set)
    groups = defaultdict(list)
    unknown = []
    uses = successful_uses(profile, root)
    for owner in profiles:
        for directory in checked(owner / ".fingerprint", root).iterdir():
            try:
                checked(directory, root)
                if not directory.is_dir():
                    continue
                metadata = list(directory.glob("*.json"))
                if not metadata and not list(directory.iterdir()):
                    continue
                if len(metadata) != 1 or not FINGERPRINT.fullmatch(directory.name):
                    unknown.append(str(directory.relative_to(root)))
                    continue
                data = read_json(metadata[0], root)
                stem = metadata[0].stem
                fingerprint = checked(directory / stem, root)
                if fingerprint.stat().st_size != 16:
                    raise ValueError("Unrecognized fingerprint size")
                digest = fingerprint.read_text().strip()
                if not re.fullmatch(r"[0-9a-f]{16}", digest) or not isinstance(data, dict) or not all(
                    key in data for key in ("rustc", "target", "profile", "deps")
                ):
                    raise ValueError("Unrecognized fingerprint")
                deps = data["deps"]
                if not isinstance(deps, list) or any(not isinstance(d, list) or len(d) != 4 or
                                                     type(d[3]) is not int or not 0 <= d[3] < 2**64 for d in deps):
                    raise ValueError("Unrecognized dependencies")
                nodes[directory] = [d[3] for d in deps]
                hashes[int.from_bytes(bytes.fromhex(digest), "little")].add(directory)
                if owner != profile or not stem.startswith(("build-script-", "run-build-script-")):
                    continue
                build = checked(profile / "build" / directory.name, root)
                if not build.is_dir():
                    continue
                files = [*tree_files(directory, root), *tree_files(build, root)]
                newest = max(last_use(directory, uses, root), *[p.stat().st_mtime for p in files])
                package = FINGERPRINT.fullmatch(directory.name)[1]
                groups[(package, stem)].append((newest, [directory, build]))
            except (OSError, ValueError, KeyError, TypeError, RecursionError):
                unknown.append(str(directory.relative_to(root)))
    return groups, nodes, hashes, unknown


def pinned(paths: list[Path], root: Path) -> bool:
    for path in paths:
        if path.is_dir() and checked(path / PIN, root).exists():
            return True
        if path.parent.name == "build" and checked(path / "out" / PIN, root).exists():
            return True
    return False


def maintain(root: Path, target: Path | None = None, *, apply=False, daily=False,
             keep=KEEP, min_age_hours=24, min_free_gib=20, now=None,
             profile_name="debug", target_triple=None, purge_incremental=False,
             build_scripts=False) -> dict:
    if keep < 1 or min_age_hours < 0 or min_free_gib < 0 or not all(
        math.isfinite(value) for value in (min_age_hours, min_free_gib)
    ):
        raise ValueError("Retention and age/space limits must be nonnegative; keep must be positive.")
    root = root.resolve()
    target = checked(target or root / "target", root)
    if profile_name not in ("debug", "release") or target_triple is not None and not re.fullmatch(
        r"[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}", target_triple
    ):
        raise ValueError("Select debug/release and a valid Cargo target triple.")
    profile = checked((target / target_triple if target_triple else target) / profile_name, root)
    report = {"status": "missing", "profile": str(profile.relative_to(root)), "variants": 0,
              "bytes": 0, "logicalBytes": 0, "uniqueBytes": 0, "reclaimableBytesEstimate": 0,
              "removedLogicalBytes": 0, "errors": [], "candidates": [], "details": [], "retained": []}
    if not (profile / "deps").is_dir() or not (profile / ".fingerprint").is_dir():
        return report
    now = time.time() if now is None else now
    emergency = shutil.disk_usage(target).free < min_free_gib * 1024**3
    stamp = checked(profile / ".transmog-cache-maintenance.json", root)
    profiles = related_profiles(target, root)
    with ExitStack() as stack:
        # Host build scripts/proc macros can be reused by cross-target builds.
        # Lock every related host/target profile before scanning any of them.
        for owner in profiles:
            if not stack.enter_context(cargo_locks(owner, root)):
                report.update(status="busy", busyProfile=str(owner.relative_to(root)))
                return report
        if checked(target / PIN, root).exists() or checked(profile / PIN, root).exists():
            report["status"] = "pinned"
            return report
        report["freeBytesBefore"] = shutil.disk_usage(target).free
        if apply:
            recovered, errors = recover_trash(profile, root)
            report["recoveredTrashLogicalBytes"] = recovered
            report["errors"].extend(errors)
        if daily and not emergency:
            try:
                data = read_json(stamp, root)
                last = data["lastRun"]
                policy = [keep, min_age_hours, min_free_gib, purge_incremental, build_scripts]
                if data.get("policy") == policy and isinstance(last, (int, float)) and 0 <= now - last < DAY:
                    report["status"] = "recent"
                    return report
            except (OSError, ValueError, KeyError, TypeError):
                pass
        cutoff = now - (0 if emergency else min_age_hours * 3600)
        report["status"] = "applied" if apply else "preview"
        plan = []

        def choose(groups, category):
            for choices in groups.values():
                for index, (newest, paths) in enumerate(sorted(choices, key=lambda item: item[0], reverse=True)):
                    reason = None
                    if pinned(paths, root):
                        reason = "pinned"
                    elif index < keep:
                        reason = "retention"
                    elif newest > cutoff:
                        reason = "recent"
                    if reason:
                        report["retained"].append({"paths": [str(p.relative_to(root)) for p in paths], "reason": reason})
                    else:
                        plan.append((newest, paths, category))

        ordinary = variants(profile, root, report["retained"])
        if purge_incremental:
            ordinary = {k: v for k, v in ordinary.items() if k[-1] != "incremental"}
        choose(ordinary, "artifact")
        if purge_incremental:
            incremental = checked(profile / "incremental", root)
            if incremental.is_dir():
                for directory in incremental.iterdir():
                    if not INCREMENTAL.fullmatch(directory.name) or not directory.is_dir():
                        report["retained"].append({"paths": [str(directory.relative_to(root))], "reason": "unknown layout"})
                        continue
                    checked(directory, root)
                    if pinned([directory], root):
                        report["retained"].append({"paths": [str(directory.relative_to(root))], "reason": "pinned"})
                    else:
                        plan.append((directory.stat().st_mtime, [directory], "incremental purge"))
        if build_scripts:
            groups, nodes, hashes, unknown = build_script_variants(profile, profiles, root)
            if unknown:
                report["retained"].append({"paths": [str((profile / "build").relative_to(root))],
                                            "reason": "incomplete fingerprint graph", "unknownUnits": unknown})
            else:
                choose(groups, "build script")
                selected = {p for _, paths, _ in plan for p in paths if p.parent.name == ".fingerprint"}
                pending = list(set(nodes) - selected)
                reachable = set(pending)
                while pending:
                    for digest in nodes[pending.pop()]:
                        for dependency in hashes[digest] - reachable:
                            reachable.add(dependency)
                            pending.append(dependency)
                filtered = []
                for item in plan:
                    if item[2] == "build script" and any(p in reachable for p in item[1]):
                        report["retained"].append({"paths": [str(p.relative_to(root)) for p in item[1]], "reason": "retained dependency"})
                    else:
                        filtered.append(item)
                plan = filtered
        elif (profile / "build").is_dir():
            report["retained"].append({"paths": [str((profile / "build").relative_to(root))],
                                        "reason": "build-script cleanup not selected"})
        accepted = []
        uses = successful_uses(profile, root)
        for newest, paths, category in plan:
            try:
                sizes = measure(paths, root)
                report["details"].append({"paths": [str(p.relative_to(root)) for p in paths], "category": category,
                                          "lastModifiedOrUsed": newest,
                                          "lastSuccessfulUse": max((last_use(p, uses, root) for p in paths if p.parent.name == ".fingerprint"), default=0) or None,
                                          **sizes})
                report["candidates"].append([str(p.relative_to(root)) for p in paths])
                accepted.extend(paths)
            except (OSError, ValueError) as error:
                report["errors"].append(str(error))
        report.update(measure(accepted, root))
        report["bytes"] = report["logicalBytes"]
        for paths in report["candidates"]:
            try:
                if apply and report["errors"] and any((root / p).parent.name == "build" for p in paths):
                    report["retained"].append({"paths": paths, "reason": "earlier cleanup failed"})
                    continue
                if apply:
                    report["removedLogicalBytes"] += remove_variant([root / p for p in paths], profile, root)
                report["variants"] += 1
            except (OSError, ValueError) as error:
                report["errors"].append(str(error))
        if apply:
            report["bytes"] = report["removedLogicalBytes"]
        report["freeBytesAfter"] = shutil.disk_usage(target).free
        report["observedFreeBytesChange"] = report["freeBytesAfter"] - report["freeBytesBefore"]
        if apply and daily and not report["errors"]:
            write_json(stamp, {"lastRun": now, "policy": [keep, min_age_hours, min_free_gib, purge_incremental, build_scripts]}, root)
    return report


def automatic(environment, *, root=None, arguments=None):
    """A failed/busy maintenance check never prevents the requested build."""
    from repo_tools import ROOT
    root = ROOT if root is None else root
    if environment.get("TRANSMOG_BUILD_CACHE_TRIM") == "0" or environment.get("CI"):
        return
    if environment.get("CARGO_BUILD_BUILD_DIR") or arguments and any(
        str(arg).startswith(("--target-dir", "--manifest-path", "--config", "--profile", "-Z", "-C")) for arg in arguments
    ):
        return
    target = Path(environment.get("CARGO_TARGET_DIR", root / "target"))
    if not target.is_absolute():
        target = root / target
    try:
        target = checked(target, root)
    except (OSError, ValueError):
        return
    try:
        report = maintain(root, target, apply=True, daily=True)
        if report["status"] == "applied" and report["variants"]:
            print(f"Build cache: removed {report['variants']} older variants "
                  f"({report['removedLogicalBytes'] / 1024**3:.2f} GiB of file content); "
                  f"kept the newest {KEEP}.", file=sys.stderr)
        for error in report["errors"]:
            print(f"Build cache maintenance skipped an item: {error}", file=sys.stderr)
    except (OSError, ValueError) as error:
        print(f"Build cache maintenance skipped: {error}", file=sys.stderr)


def is_build_command(arguments) -> bool:
    """Recognize the command after Cargo's optional toolchain/global flags."""
    args = iter(map(str, arguments))
    for argument in args:
        if argument in {"--color", "--config", "--manifest-path", "--target-dir", "-C"}:
            next(args, None)
        elif argument.startswith(("-", "+")):
            continue
        else:
            return argument in {"build", "check", "clippy", "test", "run", "bench", "doc", "rustc", "rustdoc",
                                "b", "c", "t", "r", "d"}
    return False
