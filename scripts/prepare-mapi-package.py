"""Build the TypeScript extension and verify its explicit runtime inventory."""
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Verify authored metadata and rebuild ignored runtime artifacts")
    parser.parse_args()
    package = ROOT / "extensions/mapi"
    source = json.loads((package / "manifest.json").read_text(encoding="utf-8"))
    expected = {"dist/runtime/body.js", "dist/runtime/headers.js", "README.runtime.md", "LICENSES/Office-MIT.txt"}
    if {item["path"] for item in source["files"]} != expected:
        raise SystemExit("Unexpected MAPI runtime inventory; review the manifest allowlist")
    subprocess.run(["node", str(package / "build.mjs")], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
