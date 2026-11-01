"""Preview or reclaim build caches and completed test bulk data together."""
import argparse
import json

from disk_cleanup import clean_disk
from repo_tools import ROOT, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="Delete previewed data; default is preview.")
    policy = parser.add_mutually_exclusive_group()
    policy.add_argument("--conservative", dest="deep", action="store_false",
                        help="Retain three build variants and recent completed-run bulk data instead of the aggressive default.")
    policy.add_argument("--deep", dest="deep", action="store_true",
                        help="Use the default aggressive policy: keep one build variant, trim obsolete native outputs, and clear completed-run bulk data; keep reports, logs and screenshots.")
    parser.set_defaults(deep=True)
    parser.add_argument("--include-release", action="store_true",
                        help="Also trim recognized release cache groups; preserve signed bundles and other outputs.")
    args = parser.parse_args()
    report = clean_disk(ROOT, **vars(args))
    print(json.dumps(report, indent=2))
    if report["errors"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main_guard(main)
