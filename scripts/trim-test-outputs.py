"""Preview or prune completed test-run bulk data, preserving reports and pins."""
import argparse
import json
from repo_tools import ROOT, main_guard
from test_run_cache import maintain_outputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--kind", choices=("load", "desktop"))
    parser.add_argument("--max-age-days", type=float, default=14)
    parser.add_argument("--max-gib", type=float, default=20, help="Bulk-data budget per registered output root.")
    parser.add_argument("--keep-success", type=int, default=2)
    parser.add_argument("--keep-failures", type=int, default=3)
    parser.add_argument("--include-legacy", action="store_true", help="Also consider old load runs with a valid completed report; old native runs remain protected.")
    args = parser.parse_args()
    report = maintain_outputs(ROOT, **vars(args))
    print(json.dumps(report, indent=2))
    if report["errors"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main_guard(main)
