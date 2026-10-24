"""Build the bundled schema-free Protobuf and Bond Compact Binary visualizers."""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

if __name__ == "__main__":
    subprocess.run(["node", str(ROOT / "extensions/binary/build.mjs")], cwd=ROOT, check=True)
