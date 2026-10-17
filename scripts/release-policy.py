"""Check release dependency policy against a recorded RustSec snapshot."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess

from repo_tools import ROOT, main_guard, write_json

DATABASE = "https://github.com/RustSec/advisory-db"
INPUTS = ("Cargo.toml", "Cargo.lock", "deny.toml", "fuzz/Cargo.toml", "fuzz/Cargo.lock", "fuzz/deny.toml")


def git(directory, *arguments):
    return subprocess.check_output(["git", "-C", str(directory), *arguments], text=True, encoding="utf-8").strip()


def input_hashes(root=ROOT):
    # All inputs are TOML/text. Git checkout line-ending preferences can differ
    # across Windows runners without changing the dependency graph or policy.
    return {name: hashlib.sha256((root / name).read_bytes().replace(b"\r\n", b"\n")).hexdigest() for name in INPUTS}


def configuration(policy, database):
    text = policy.read_text(encoding="utf-8")
    if text.count("[advisories]") != 1:
        raise ValueError("Expected one advisory policy section")
    return text.replace("[advisories]", "[advisories]\ndb-path = " + json.dumps(database.as_posix(), ensure_ascii=False)
                        + "\ndb-urls = [" + json.dumps(DATABASE) + "]", 1)


def snapshot_repository(snapshot, metadata):
    relative = metadata["databasePath"]
    if not isinstance(relative, str) or not re.fullmatch(r"db/[A-Za-z0-9_.-]+", relative):
        raise ValueError("Invalid advisory database path")
    repository = snapshot / relative
    if repository.resolve() != snapshot.resolve().joinpath(*relative.split("/")):
        raise ValueError("Advisory database path follows a link")
    return repository


def validate_snapshot(snapshot, source_commit, hashes):
    metadata = json.loads((snapshot / "snapshot.json").read_text(encoding="utf-8"))
    if metadata.get("schema") != 1 or metadata.get("sourceCommit") != source_commit or metadata.get("inputs") != hashes:
        raise ValueError("Advisory snapshot does not match this source commit and dependency policy")
    repository = snapshot_repository(snapshot, metadata)
    revision = metadata.get("databaseCommit", "")
    if not re.fullmatch(r"[0-9a-f]{40}", revision) or git(repository, "rev-parse", "HEAD") != revision:
        raise ValueError("Advisory database revision differs from its snapshot")
    if git(repository, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Advisory database working tree must be clean")
    return metadata


def diagnostics(text):
    counts = Counter()
    errors = []
    for line in text.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        fields = event.get("fields", {})
        severity = fields.get("severity", "")
        if severity in ("error", "warning"):
            counts[severity + ":" + str(fields.get("code", "unknown"))] += 1
        if severity == "error":
            errors.append(fields)
    return dict(counts), errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--prepare", action="store_true", help="Fetch a fresh database before checking both locked graphs")
    args = parser.parse_args()
    snapshot, output = args.snapshot.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source_commit = git(ROOT, "rev-parse", "HEAD")
    hashes = input_hashes()
    policies = (("workspace", ROOT / "Cargo.toml", ROOT / "deny.toml"),
                ("fuzz", ROOT / "fuzz/Cargo.toml", ROOT / "fuzz/deny.toml"))
    configs = {}
    for name, _, policy in policies:
        configs[name] = output / (name + ".toml")
        configs[name].write_text(configuration(policy, snapshot / "db"), encoding="utf-8")
    if args.prepare:
        snapshot.mkdir(parents=True, exist_ok=True)
        subprocess.run(["cargo", "deny", "--config", str(configs["workspace"]), "--locked", "fetch", "db"], cwd=ROOT, check=True)
        repositories = [path for path in (snapshot / "db").iterdir() if (path / ".git").is_dir()]
        if len(repositories) != 1:
            raise ValueError("Expected one RustSec repository in the snapshot")
        repository = repositories[0]
        write_json(snapshot / "snapshot.json", {
            "schema": 1, "sourceCommit": source_commit, "inputs": hashes,
            "databaseUrl": DATABASE, "databasePath": repository.relative_to(snapshot).as_posix(),
            "databaseCommit": git(repository, "rev-parse", "HEAD"),
            "capturedAtUtc": datetime.now(timezone.utc).isoformat(),
        })
    metadata = validate_snapshot(snapshot, source_commit, hashes)
    print("RustSec snapshot: " + metadata["databaseCommit"] + " (" + metadata["capturedAtUtc"] + ")", flush=True)
    results = []
    for name, manifest, _ in policies:
        # Populate package sources/licenses and the registry index before the
        # offline check. The advisory database itself stays fixed for this run.
        subprocess.run(["cargo", "fetch", "--locked", "--manifest-path", str(manifest)], cwd=ROOT, check=True)
        subprocess.run(["cargo", "deny", "--locked", "--manifest-path", str(manifest), "--config", str(configs[name]), "fetch", "index"], cwd=ROOT, check=True)
        process = subprocess.run(["cargo", "deny", "--format", "json", "--color", "never", "--offline", "--locked",
                                  "--manifest-path", str(manifest), "--config", str(configs[name]), "check"],
                                 cwd=ROOT, text=True, encoding="utf-8", errors="replace", capture_output=True)
        full_output = process.stdout + process.stderr
        (output / (name + ".jsonl")).write_text(full_output, encoding="utf-8")
        counts, errors = diagnostics(full_output)
        result = {"graph": name, "result": "passed" if process.returncode == 0 else "failed", "diagnostics": counts}
        results.append(result)
        print(f"{name} policy: {result['result']}; {json.dumps(counts, sort_keys=True)}", flush=True)
        for error in errors:
            print(json.dumps(error, ensure_ascii=False), flush=True)
        if process.returncode and not errors:
            print("\n".join(full_output.splitlines()[-15:]), flush=True)
    passed = all(result["result"] == "passed" for result in results)
    if validate_snapshot(snapshot, source_commit, hashes) != metadata or git(ROOT, "rev-parse", "HEAD") != source_commit or input_hashes() != hashes:
        raise ValueError("Source or advisory snapshot changed during policy checking")
    report = {**metadata, "result": "passed" if passed else "failed", "checks": results,
              "checkedAtUtc": datetime.now(timezone.utc).isoformat()}
    write_json(output / "policy.json", report)
    print("Full policy diagnostics: " + str(output), flush=True)
    if not passed:
        raise ValueError("Release dependency policy failed; inspect policy.json and the diagnostic artifacts")


if __name__ == "__main__":
    main_guard(main)
