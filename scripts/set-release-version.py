"""Check or update the synchronized Cargo, desktop and release versions."""
import argparse
from pathlib import Path
from release_version import set_version
from repo_tools import ROOT, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--version")
    group.add_argument("--check", action="store_true")
    parser.add_argument("--channel", choices=("Canary", "Release"))
    parser.add_argument("--release-type", choices=("Canary", "Beta", "Stable"))
    parser.add_argument("--repository-root", type=Path, default=ROOT)
    args = parser.parse_args()
    if args.check and (args.channel or args.release_type):
        parser.error("--check does not change a release track")
    set_version(args.repository_root, args.version, check=args.check, channel=args.channel, release_type=args.release_type)


if __name__ == "__main__":
    main_guard(main)
