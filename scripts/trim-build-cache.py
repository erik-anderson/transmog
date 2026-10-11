"""Preview or trim older Cargo development variants, including their fingerprints."""
import argparse
import json
from pathlib import Path
from build_cache import maintain
from repo_tools import ROOT, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="Delete the listed cache groups; default is preview.")
    parser.add_argument("--daily", action="store_true", help="Check at most once per 24 hours unless space is critically low.")
    parser.add_argument("--keep", type=int, default=3, help="Retain at least this many variants per Cargo target (default: 3).")
    parser.add_argument("--min-age-hours", type=float, default=24)
    parser.add_argument("--min-free-gib", type=float, default=20,
                        help="Below this free space, trim excess recent variants too (default: 20 GiB).")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target",
                        help="A Cargo target directory inside this repository.")
    args = parser.parse_args()
    print(json.dumps(maintain(ROOT, args.target_dir, apply=args.apply, daily=args.daily,
                             keep=args.keep, min_age_hours=args.min_age_hours,
                             min_free_gib=args.min_free_gib), indent=2))


if __name__ == "__main__":
    main_guard(main)
