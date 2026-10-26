"""Observe successful Cargo JSON messages without changing program output."""
import json
from pathlib import Path
import subprocess
import sys
import time

from build_cache import (HASHED, USAGE, cargo_locks, checked,
                         successful_uses, write_json)


def record(root, messages):
    """Refresh only units actually reported by this successful invocation."""
    units = {}
    for message in messages:
        if message.get("reason") == "compiler-artifact":
            paths = [*message.get("filenames", []), message.get("executable")]
        elif message.get("reason") == "build-script-executed":
            paths = [message.get("out_dir")]
        else:
            continue
        for value in paths:
            if not value:
                continue
            path = checked(Path(value), root)
            if path.parent.name in ("deps", "examples"):
                match = HASHED.fullmatch(path.name)
                if match:
                    units.setdefault(path.parent.parent, set()).update(
                        (path.parent.parent / ".fingerprint").glob("*-" + match[2]))
            elif path.parent.parent.name == "build":
                units.setdefault(path.parent.parent.parent, set()).add(
                    path.parent.parent.parent / ".fingerprint" / path.parent.name)
            elif path.parent.name in {"debug", "release"} and path.is_file():
                # Cargo reports the unhashed top-level executable for binaries.
                # Its hardlinked deps entry identifies the exact fingerprint.
                for stem in {path.stem, path.stem.replace("-", "_")}:
                    for artifact in (path.parent / "deps").glob(stem + "-*" + path.suffix):
                        match = HASHED.fullmatch(artifact.name)
                        if match and checked(artifact, root).samefile(path):
                            units.setdefault(path.parent, set()).update(
                                (path.parent / ".fingerprint").glob("*-" + match[2]))
    for profile, directories in units.items():
        with cargo_locks(profile, root) as acquired:
            if not acquired:
                continue
            uses = successful_uses(profile, root)
            for directory in directories:
                metadata = list(checked(directory, root).glob("*.json"))
                if len(metadata) == 1:
                    fingerprint = checked(directory / metadata[0].stem, root)
                    if fingerprint.is_file() and fingerprint.stat().st_size == 16:
                        uses[directory.name] = {"stem": metadata[0].stem,
                                                "fingerprint": fingerprint.read_text(),
                                                "lastSuccessfulUse": time.time()}
            # Evicted units should not grow the record forever.
            uses = {name: row for name, row in uses.items()
                    if checked(profile / ".fingerprint" / name, root).is_dir()}
            write_json(profile / USAGE, {"schema": 1, "units": uses}, root)


def run(argv, *, root, cwd, env, check):
    """Stream rendered diagnostics while collecting build/check/clippy units.

    Only these compile commands use this adapter. Programs/tests retain their
    original stdout and stdin behavior; caller-requested message formats are
    never rewritten.
    """
    argv = [*argv, "--message-format=json-render-diagnostics"]
    messages = []
    finished = False
    with subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                          text=True, encoding="utf-8", errors="replace") as child:
        for line in child.stdout:
            try:
                message = json.loads(line) if not finished else None
            except ValueError:
                message = None
            if not isinstance(message, dict) or "reason" not in message:
                print(line, end="", flush=True)
            elif message["reason"] == "compiler-message":
                rendered = message.get("message", {}).get("rendered")
                if rendered:
                    print(rendered, end="", file=sys.stderr, flush=True)
            elif message["reason"] in {"compiler-artifact", "build-script-executed", "build-finished"}:
                messages.append(message)
                finished = message["reason"] == "build-finished"
            else:
                print(line, end="", flush=True)
        returncode = child.wait()
    if returncode == 0 and any(m.get("reason") == "build-finished" and m.get("success") is True for m in messages):
        try:
            record(root, messages)
        except (OSError, ValueError, TypeError, KeyError) as error:
            print(f"Build cache use tracking skipped: {error}", file=sys.stderr)
    if check and returncode:
        raise subprocess.CalledProcessError(returncode, argv)
    return subprocess.CompletedProcess(argv, returncode)
