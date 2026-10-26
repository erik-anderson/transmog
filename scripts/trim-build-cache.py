"""Preview or trim older Cargo development variants, including their fingerprints."""
import argparse
import json
from pathlib import Path
from build_cache import REGISTERED_TARGETS, checked, maintain, related_profiles
from repo_tools import ROOT, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="Delete the listed cache groups; default is preview.")
    parser.add_argument("--daily", action="store_true", help="Check at most once per 24 hours unless space is critically low.")
    parser.add_argument("--keep", type=int, default=3, help="Retain at least this many variants per Cargo target (default: 3).")
    parser.add_argument("--min-age-hours", type=float, default=24)
    parser.add_argument("--min-free-gib", type=float, default=20,
                        help="Below this free space, trim excess recent variants too (default: 20 GiB).")
    parser.add_argument("--target-dir", type=Path,
                        help="A Cargo target directory inside this repository.")
    parser.add_argument("--registered", action="store_true", help="Inspect the registered workspace, SDK and fuzz caches, including cross-target debug profiles.")
    parser.add_argument("--profile", choices=("debug", "release"), default="debug",
                        help="Release cleanup requires this explicit selection.")
    parser.add_argument("--target-triple", help="Select a cross-target profile within --target-dir.")
    parser.add_argument("--purge-incremental", action="store_true",
                        help="Purge all unpinned incremental caches, ignoring age/count retention; preserve compiled artifacts.")
    parser.add_argument("--build-scripts", action="store_true",
                        help="Also trim complete obsolete build-script units whose retained dependencies are known.")
    args = parser.parse_args()
    if args.registered and (args.target_dir or args.target_triple):
        parser.error("--registered cannot be combined with --target-dir or --target-triple")
    options = dict(apply=args.apply, daily=args.daily, keep=args.keep,
                   min_age_hours=args.min_age_hours, min_free_gib=args.min_free_gib,
                   purge_incremental=args.purge_incremental, build_scripts=args.build_scripts,
                   profile_name=args.profile)
    if args.registered:
        reports = []
        for name in REGISTERED_TARGETS:
            target = checked(ROOT / name, ROOT)
            for profile in related_profiles(target, ROOT):
                if profile.name == args.profile:
                    triple = None if profile.parent == target else profile.parent.name
                    reports.append(maintain(ROOT, target, target_triple=triple, **options))
        report = {"profiles": reports}
    else:
        report = maintain(ROOT, args.target_dir, target_triple=args.target_triple, **options)
    print(json.dumps(report, indent=2))
    if any(item["errors"] for item in report.get("profiles", [report])):
        raise SystemExit(1)


if __name__ == "__main__":
    main_guard(main)
