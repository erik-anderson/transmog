"""Bind independent release gates to one source/run and merge their payloads."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil

from repo_tools import main_guard
from v8_artifact import digest, input_hashes, encoded_digest

GATES = {
    "Frontend": ("UI", "Browser"),
    "Validation": ("Analysis", "Tests", "Native", "Archives"),
    "Package": ("Build", "Qualify"),
}


def catalog(root):
    result = {}
    for path in root.rglob("*"):
        if path.is_symlink() or getattr(path, "is_junction", lambda: False)():
            raise ValueError("Release stage links are forbidden")
        if path.is_file() and path != root / "stage-manifest.json":
            result[path.relative_to(root).as_posix()] = digest(path)
    return result


def verify(root, stage, commit, run_id, target, attempt):
    manifest = json.loads((root / "stage-manifest.json").read_text(encoding="utf-8-sig"))
    expected = {"Stage": stage, "Commit": commit, "RunId": run_id, "Target": target, "RunAttempt": manifest.get("RunAttempt")}
    if not str(expected["RunAttempt"]).isdigit() or not 1 <= int(expected["RunAttempt"]) <= int(attempt):
        raise ValueError("Release stage attempt is invalid or from the future")
    if any(manifest.get(name) != value for name, value in expected.items()):
        raise ValueError("Release stage source, run, attempt, target or producer differs")
    if manifest["Files"] != catalog(root):
        raise ValueError("Release stage file catalog differs")
    gate_paths = {f"evidence/gates/{gate}.json" for gate in GATES[stage]}
    for relative in manifest["Files"]:
        owned = relative in gate_paths
        if stage == "Frontend":
            owned |= relative.startswith("evidence/policy/")
        elif stage == "Validation":
            owned |= relative == "evidence/v8-engine-validation.json"
        elif stage == "Package":
            owned |= (relative.startswith(("target/release/", "apps/desktop/ui/dist/"))
                      or relative.startswith("evidence/") and relative.count("/") == 1)
        if not owned:
            raise ValueError(f"Unexpected {stage} producer path: {relative}")
    actual_gates = {path.stem for path in (root / "evidence/gates").glob("*.json")}
    if actual_gates != set(GATES[stage]):
        raise ValueError("Release stage gate set differs")
    for gate in GATES[stage]:
        result = json.loads((root / f"evidence/gates/{gate}.json").read_text(encoding="utf-8-sig"))
        if (any(result.get(name) != value for name, value in expected.items())
                or result.get("Name") != gate or result.get("Result") != "passed"):
            raise ValueError(f"Release gate {gate} did not pass for this producer")
    if stage != "Frontend":
        engine = json.loads((root / f"evidence/v8-engine-{stage.lower()}.json").read_text(encoding="utf-8-sig"))
        if (engine.get("Commit") != commit or engine.get("RunId") != run_id
                or engine.get("Target") != target or not str(engine.get("ArtifactId", "")).isdigit()):
            raise ValueError("V8 consumer provenance differs")
        native = engine["Engine"]
        if (native["identity"]["inputs"] != input_hashes()
                or native["identity"]["target"] != target
                or native["key"] != encoded_digest(native["identity"])):
            raise ValueError("V8 engine inputs differ from the release candidate")
    return manifest


def export(root, stage, commit, run_id, target, attempt):
    manifest = {"Stage": stage, "Commit": commit, "RunId": run_id, "Target": target,
                "RunAttempt": attempt, "Files": catalog(root)}
    (root / "stage-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    verify(root, stage, commit, run_id, target, attempt)


def merge(inputs, artifact_ids, output, commit, run_id, target, attempt):
    if output.exists():
        raise ValueError("Release merge requires a fresh output directory")
    manifests = {stage: verify(root, stage, commit, run_id, target, attempt) for stage, root in inputs.items()}
    if set(manifests) != set(GATES) or set(artifact_ids) != set(GATES):
        raise ValueError("Every required release stage must be supplied exactly once")
    if any(not str(value).isdigit() for value in artifact_ids.values()) or len(set(artifact_ids.values())) != 3:
        raise ValueError("Immutable producer artifact IDs must be distinct")
    engines = [json.loads((inputs[stage] / f"evidence/v8-engine-{stage.lower()}.json").read_text(encoding="utf-8-sig"))
               for stage in ("Validation", "Package")]
    if engines[0] != engines[1]:
        raise ValueError("Validation and packaging used different V8 artifacts")
    seen = set()
    for stage, root in inputs.items():
        for relative in manifests[stage]["Files"]:
            if relative in seen or relative in {"build-manifest.json", "evidence/pipeline-inputs.json"}:
                raise ValueError(f"Duplicate or reserved release stage path: {relative}")
            seen.add(relative)
            destination = output / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / relative, destination)
    metadata = json.loads((output / "evidence/package-metadata.json").read_text(encoding="utf-8-sig"))
    if metadata["Commit"] != commit or metadata["RunId"] != run_id or metadata["Target"] != target:
        raise ValueError("Package metadata source differs")
    provenance = {"Commit": commit, "RunId": run_id, "RunAttempt": attempt, "Target": target,
                  "Stages": {stage: {"ArtifactId": artifact_ids[stage],
                                      "ManifestSha256": digest(inputs[stage] / "stage-manifest.json"),
                                      "RunAttempt": manifests[stage]["RunAttempt"]}
                             for stage in GATES}, "V8": engines[0]}
    (output / "evidence/pipeline-inputs.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    metadata["PipelineSchema"] = 1
    metadata["Files"] = [{"Path": name, "Sha256": sha} for name, sha in catalog(output).items()]
    (output / "build-manifest.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("export", "merge"))
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--stage", choices=GATES)
    parser.add_argument("--root", type=Path)
    for stage in GATES:
        parser.add_argument(f"--{stage.lower()}", type=Path)
        parser.add_argument(f"--{stage.lower()}-id")
    args = parser.parse_args()
    common = (args.commit, os.environ["GITHUB_RUN_ID"], args.target, os.environ["GITHUB_RUN_ATTEMPT"])
    if args.operation == "export":
        export(args.root, args.stage, *common)
    else:
        merge({stage: getattr(args, stage.lower()) for stage in GATES},
              {stage: getattr(args, stage.lower() + "_id") for stage in GATES}, args.root, *common)


if __name__ == "__main__":
    main_guard(main)
