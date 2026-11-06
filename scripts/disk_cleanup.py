"""Combined, opt-in cleanup of generated builds and completed test bulk data."""
from pathlib import Path
import shutil

from build_cache import maintain, registered_targets, related_profiles
from test_run_cache import maintain_outputs
from v8_cache import maintain_v8


def clean_disk(root: Path, *, apply=False, deep=True, include_release=False) -> dict:
    root = root.resolve()
    report = {"status": "applied" if apply else "preview", "deep": deep,
              "includeRelease": include_release, "preservedProfiles": [],
              "freeBytesBefore": shutil.disk_usage(root).free, "profiles": [],
              "errors": [], "logicalBytes": 0, "reclaimableBytesEstimate": 0,
              "removedLogicalBytes": 0}
    for target in registered_targets(root):
        for profile in related_profiles(target, root):
            if profile.name == "release" and not include_release:
                report["preservedProfiles"].append({"profile": str(profile.relative_to(root)),
                                                     "reason": "release cleanup not selected"})
                continue
            triple = None if profile.parent == target else profile.parent.name
            result = maintain(root, target, apply=apply, keep=1 if deep else 3,
                              min_age_hours=0 if deep else 24,
                              target_triple=triple, profile_name=profile.name,
                              purge_incremental=True, build_scripts=deep)
            report["profiles"].append(result)
    report["testOutputs"] = maintain_outputs(
        root, apply=apply, include_legacy=True,
        keep_success=0 if deep else 2, keep_failures=0 if deep else 3,
        max_age_days=0 if deep else 14, max_gib=0 if deep else 20)
    report["v8Caches"] = maintain_v8(root, apply=apply, deep=deep, include_release=include_release)
    for item in [*report["profiles"], report["testOutputs"], report["v8Caches"]]:
        report["errors"].extend(item["errors"])
        for field in ("logicalBytes", "reclaimableBytesEstimate", "removedLogicalBytes"):
            report[field] += item[field]
    report["freeBytesAfter"] = shutil.disk_usage(root).free
    report["observedFreeBytesChange"] = report["freeBytesAfter"] - report["freeBytesBefore"]
    return report
