"""Compile the TypeScript MAPI project to ignored ES2022 build artifacts."""
import argparse
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Type-check and rebuild runtime artifacts")
    parser.parse_args()
    subprocess.run(["node", str(ROOT / "extensions/mapi/build.mjs")], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
