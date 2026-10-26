"""Lease a fresh test output directory until an explicit JSON completion on stdin."""
import argparse
import json
from pathlib import Path
import sys
from repo_tools import main_guard
from test_run_cache import output_run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", type=Path, required=True)
    parser.add_argument("--kind", choices=("load", "desktop"), required=True)
    parser.add_argument("--group", default="default")
    args = parser.parse_args()
    with output_run(args.path, args.kind, group=args.group) as outcome:
        print(json.dumps({"ready": True}), flush=True)
        line = sys.stdin.readline()
        if line:
            data = json.loads(line)
            if not isinstance(data, dict) or type(data.get("passed")) is not bool:
                raise ValueError("Completion requires a boolean passed outcome")
            outcome["passed"] = data["passed"]


if __name__ == "__main__":
    main_guard(main)
