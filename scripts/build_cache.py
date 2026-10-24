"""Conservative maintenance of complete Cargo development artifact variants."""
from __future__ import annotations

from collections import defaultdict
from contextlib import contextmanager
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
    for parent, directories, names in os.walk(path, followlinks=False):
        for name in directories + names:
            checked(Path(parent) / name, root)
        files.extend(Path(parent) / name for name in names)
    return files


@contextmanager
def cargo_locks(profile: Path, root: Path):
    """Try both legacy and current Cargo locks, never waiting for a build.

    Cargo's layout retains the legacy shared lock for compatibility:
    https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/compiler/layout.rs.html
    An exclusive lock on its first byte conflicts with Cargo's Windows range.
    """
    handles = []
    acquired = False
    try:
        for name in LOCKS:
            handle = checked(profile / name, root).open("a+b")
            try:
                handle.seek(0)
                if os.name == "nt":
                    import msvcrt
                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                handle.close()
                break
            handles.append(handle)
        else:
            acquired = True
        yield acquired
    finally:
        for handle in reversed(handles):
            handle.seek(0)
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
            handle.close()


def variants(profile: Path, root: Path):
    """Only pair recognized files with one valid Cargo fingerprint directory."""
    by_hash = defaultdict(list)
    for path in checked(profile / "deps", root).iterdir():
        match = HASHED.fullmatch(path.name)
        if match:
            by_hash[match[2]].append(path)
    groups = defaultdict(list)
    target_owners = defaultdict(set)
    for directory in checked(profile / ".fingerprint", root).iterdir():
        match = FINGERPRINT.fullmatch(directory.name)
        if not match or not directory.is_dir():
            continue
        try:
            checked(directory, root)
            metadata = list(directory.glob("*.json"))
            if len(metadata) != 1 or metadata[0].stat().st_size > 1024 * 1024:
                continue
            checked(metadata[0], root)
            data = json.loads(metadata[0].read_text(encoding="utf-8"))
            if not isinstance(data, dict) or not all(
                key in data for key in ("rustc", "target", "profile", "deps")
            ):
                continue
            stem = metadata[0].stem
            if stem.startswith(("build-script-", "run-build-script-")):
                continue
            artifacts = by_hash.get(match[2], [])
            if not artifacts or any((HASHED.fullmatch(p.name)[3] or "") not in SUFFIXES
                                    or not p.is_file() for p in artifacts):
                continue
            files = tree_files(directory, root)
            for artifact in artifacts:
                checked(artifact, root)
            newest = max(p.stat().st_mtime for p in [*files, *artifacts])
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
        for path in sorted(paths, key=lambda item: not item.is_dir()):
            checked(path, root)
            destination = checked(trash / path.name, root)
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


def maintain(root: Path, target: Path | None = None, *, apply=False, daily=False,
             keep=KEEP, min_age_hours=24, min_free_gib=20, now=None) -> dict:
    if keep < 1 or min_age_hours < 0 or min_free_gib < 0 or not all(
        math.isfinite(value) for value in (min_age_hours, min_free_gib)
    ):
        raise ValueError("Retention and age/space limits must be nonnegative; keep must be positive.")
    root = root.resolve()
    target = checked(target or root / "target", root)
    profile = checked(target / "debug", root)
    report = {"status": "missing", "variants": 0, "bytes": 0, "errors": [], "candidates": []}
    if not (profile / "deps").is_dir() or not (profile / ".fingerprint").is_dir():
        return report
    now = time.time() if now is None else now
    emergency = shutil.disk_usage(target).free < min_free_gib * 1024**3
    stamp = checked(profile / ".transmog-cache-maintenance.json", root)
    with cargo_locks(profile, root) as acquired:
        if not acquired:
            report["status"] = "busy"
            return report
        trash = checked(profile / ".transmog-cache-trash", root)
        if apply and trash.is_dir():
            for directory in trash.iterdir():
                if re.fullmatch(r"[0-9a-f]{32}", directory.name) and directory.is_dir():
                    try:
                        tree_files(directory, root)
                        shutil.rmtree(checked(directory, root))
                    except (OSError, ValueError) as error:
                        report["errors"].append(str(error))
        if daily and not emergency:
            try:
                last = json.loads(stamp.read_text(encoding="utf-8"))["lastRun"]
                if isinstance(last, (int, float)) and 0 <= now - last < DAY:
                    report["status"] = "recent"
                    return report
            except (OSError, ValueError, KeyError, TypeError):
                pass
        cutoff = now - (0 if emergency else min_age_hours * 3600)
        report["status"] = "applied" if apply else "preview"
        for choices in variants(profile, root).values():
            for newest, paths in sorted(choices, key=lambda item: item[0], reverse=True)[keep:]:
                if newest > cutoff:
                    continue
                try:
                    files = [file for path in paths for file in tree_files(path, root)]
                    size = sum(file.stat().st_size for file in files)
                    report["candidates"].append([str(path.relative_to(root)) for path in paths])
                    if apply:
                        size = remove_variant(paths, profile, root)
                    report["bytes"] += size
                    report["variants"] += 1
                except (OSError, ValueError) as error:
                    report["errors"].append(str(error))
        if apply and daily and not report["errors"]:
            temp = checked(stamp.with_name(stamp.name + "." + uuid.uuid4().hex + ".tmp"), root)
            try:
                temp.write_text(json.dumps({"lastRun": now}) + "\n", encoding="utf-8")
                temp.replace(stamp)
            finally:
                temp.unlink(missing_ok=True)
    return report


def automatic(environment, *, root=None, arguments=None):
    """A failed/busy maintenance check never prevents the requested build."""
    from repo_tools import ROOT
    root = ROOT if root is None else root
    if environment.get("TRANSMOG_BUILD_CACHE_TRIM") == "0" or environment.get("CI"):
        return
    if environment.get("CARGO_BUILD_BUILD_DIR") or arguments and any(
        str(arg).startswith(("--target-dir", "--manifest-path", "--config", "-Z", "-C")) for arg in arguments
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
                  f"({report['bytes'] / 1024**3:.2f} GiB); kept the newest {KEEP}.", file=sys.stderr)
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
