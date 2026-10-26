"""Retention for explicitly owned, completed test-run bulk data."""
from collections import defaultdict
from contextlib import contextmanager, ExitStack
from datetime import datetime
import math
import json
from pathlib import Path
import shutil
import time

from build_cache import PIN, checked, file_lock, measure, read_json, recover_trash, remove_variant, write_json

MANIFEST = ".transmog-test-run.json"
RUN_LOCK = ".transmog-test-run.lock"
ROOTS = {"load": "artifacts/load", "desktop": "artifacts/windows-desktop-validation"}
# Reports, screenshots, logs and diagnostic JSON live outside these bulk paths.
# A manifest cannot nominate arbitrary files for deletion.
BULK = {"load": ("cli/traffic.tmcap", "cli/profile", "desktop/profile"),
        "desktop": ("profile",)}


@contextmanager
def output_run(path: Path, kind: str, *, group="default", now=None):
    """Hold an OS lease from before startup through child-process shutdown.

    EOF/crash without an explicit completion leaves an interrupted run protected.
    Paths outside the repository may be produced, but repository cleanup never
    discovers them. The run directory itself is the explicit ownership boundary.
    """
    if kind not in ROOTS:
        raise ValueError("Unknown test-run kind")
    path = path.resolve()
    path.mkdir(parents=True, exist_ok=True)
    with file_lock(path / RUN_LOCK, path) as acquired:
        if not acquired:
            raise ValueError("Test output is already leased")
        if checked(path / MANIFEST, path).exists():
            raise ValueError("Test output is already registered; use a fresh run directory")
        manifest = {"schema": 1, "kind": kind, "group": group, "state": "running",
                    "startedAt": time.time() if now is None else now}
        write_json(path / MANIFEST, manifest, path)
        outcome = {}
        try:
            yield outcome
        finally:
            passed = outcome.get("passed")
            manifest.update(state="completed" if type(passed) is bool else "interrupted",
                            finishedAt=time.time())
            if type(passed) is bool:
                manifest["passed"] = passed
            write_json(path / MANIFEST, manifest, path)


def legacy_load(run: Path, root: Path):
    """Opt-in adoption of the old harness's final, completed report only."""
    data = read_json(run / "report.json", root)
    if data.get("schema") != 1 or type(data.get("passed")) is not bool or not isinstance(data.get("results"), list):
        raise ValueError("Not a completed legacy load report")
    if any(not isinstance(result, dict) or result.get("cleanupError") for result in data["results"]):
        raise ValueError("Legacy load run did not finish stopping its processes")
    if data["passed"] and (not data["results"] or any(result.get("passed") is not True for result in data["results"])):
        raise ValueError("Legacy load report has inconsistent outcomes")
    finished = datetime.fromisoformat(data["finishedAt"].replace("Z", "+00:00"))
    if finished.tzinfo is None:
        raise ValueError("Legacy report has no time zone")
    config = data["config"]
    group = json.dumps([config.get("target"), config.get("storage"), config.get("smoke", False)], separators=(",", ":"))
    return {"schema": 1, "kind": "load", "group": group, "state": "completed",
            "finishedAt": finished.timestamp(), "passed": data["passed"], "legacy": True}


def maintain_outputs(root: Path, *, apply=False, kind=None, max_age_days=14,
                     max_gib=20, keep_success=2, keep_failures=3,
                     include_legacy=False, now=None):
    if kind is not None and kind not in ROOTS:
        raise ValueError("Unknown test-run kind")
    if any(not math.isfinite(v) or v < 0 for v in (max_age_days, max_gib)) or min(keep_success, keep_failures) < 0:
        raise ValueError("Test retention limits must be finite and nonnegative")
    root = root.resolve()
    now = time.time() if now is None else now
    report = {"status": "applied" if apply else "preview", "runs": 0, "candidates": [],
              "retained": [], "errors": [], "logicalBytes": 0, "uniqueBytes": 0,
              "reclaimableBytesEstimate": 0, "removedLogicalBytes": 0,
              "recoveredTrashLogicalBytes": 0, "freeBytesBefore": shutil.disk_usage(root).free}
    for run_kind in ([kind] if kind else ROOTS):
        parent = checked(root / ROOTS[run_kind], root)
        if not parent.is_dir():
            continue
        if checked(parent / PIN, root).exists():
            report["retained"].append({"path": str(parent.relative_to(root)), "reason": "pinned"})
            continue
        # Keep leases through planning and deletion, including the newest runs
        # that determine retention. A concurrent cleaner must not reorder them.
        with ExitStack() as stack:
            runs = []
            pinned_bytes = 0
            for run in sorted(parent.iterdir()):
                relative = str(run.relative_to(root))
                try:
                    checked(run, root)
                    if not run.is_dir():
                        continue
                    if not stack.enter_context(file_lock(run / RUN_LOCK, root)):
                        report["retained"].append({"path": relative, "reason": "active or busy"})
                        continue
                    if checked(run / PIN, root).exists():
                        paths = [checked(run / p, root) for p in BULK[run_kind] if (run / p).exists()]
                        pinned_bytes += measure(paths, root)["uniqueBytes"]
                        report["retained"].append({"path": relative, "reason": "pinned"})
                        continue
                    if (run / MANIFEST).exists():
                        manifest = read_json(run / MANIFEST, root)
                    elif include_legacy and run_kind == "load":
                        manifest = legacy_load(run, root)
                    else:
                        report["retained"].append({"path": relative, "reason": "unregistered"})
                        continue
                    if manifest.get("schema") != 1 or manifest.get("kind") != run_kind or manifest.get("state") != "completed" or type(manifest.get("passed")) is not bool:
                        report["retained"].append({"path": relative, "reason": "incomplete or unknown run"})
                        continue
                    finished = manifest["finishedAt"]
                    if type(finished) not in (int, float) or not math.isfinite(finished) or not 0 <= finished <= now:
                        raise ValueError("Invalid test completion time")
                    if apply:
                        recovered, errors = recover_trash(run, root)
                        report["recoveredTrashLogicalBytes"] += recovered
                        report["errors"].extend(errors)
                    paths = [checked(run / p, root) for p in BULK[run_kind] if (run / p).exists()]
                    # Pins inside a bulk profile protect it as well.
                    if any(path.is_dir() and checked(path / PIN, root).exists() for path in paths):
                        report["retained"].append({"path": relative, "reason": "pinned"})
                        continue
                    sizes = measure(paths, root)
                    if not sizes["logicalBytes"]:
                        report["retained"].append({"path": relative, "reason": "no bulk data"})
                        continue
                    group = manifest.get("group", "default")
                    if not isinstance(group, str):
                        raise ValueError("Invalid test-run group")
                    runs.append((finished, run, manifest, paths, sizes, (group, manifest["passed"])))
                except (OSError, ValueError, KeyError, TypeError, AttributeError, RecursionError) as error:
                    report["retained"].append({"path": relative, "reason": "unreadable or unsafe", "detail": str(error)})
            retained = defaultdict(int)
            eligible = []
            total = pinned_bytes + sum(item[4]["uniqueBytes"] for item in runs)
            for item in sorted(runs, key=lambda item: item[0], reverse=True):
                group = item[5]
                limit = keep_success if group[1] else keep_failures
                if retained[group] < limit:
                    retained[group] += 1
                    report["retained"].append({"path": str(item[1].relative_to(root)), "reason": "recent success" if group[1] else "failure evidence"})
                else:
                    eligible.append(item)
            accepted = []
            selected = []
            for finished, run, manifest, paths, sizes, group in sorted(eligible, key=lambda item: item[0]):
                if now - finished < max_age_days * 86400 and total <= max_gib * 1024**3:
                    report["retained"].append({"path": str(run.relative_to(root)), "reason": "within age and size budgets"})
                    continue
                candidate = {"path": str(run.relative_to(root)), "paths": [str(p.relative_to(root)) for p in paths],
                             "reason": "expired" if now - finished >= max_age_days * 86400 else "size budget", **sizes}
                report["candidates"].append(candidate)
                accepted.extend(paths)
                total -= sizes["uniqueBytes"]
                selected.append((run, manifest, paths, sizes))
            sizes = measure(accepted, root)
            for field in sizes:
                report[field] += sizes[field]
            for run, manifest, paths, sizes in selected:
                if apply:
                    try:
                        report["removedLogicalBytes"] += remove_variant(paths, run, root)
                        manifest.update(bulkPrunedAt=now, bulkBytesRemoved=sizes["logicalBytes"])
                        write_json(run / MANIFEST, manifest, root)
                    except (OSError, ValueError) as error:
                        report["errors"].append(str(error))
                        continue
                report["runs"] += 1
    report["freeBytesAfter"] = shutil.disk_usage(root).free
    report["observedFreeBytesChange"] = report["freeBytesAfter"] - report["freeBytesBefore"]
    return report
