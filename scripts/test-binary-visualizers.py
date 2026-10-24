"""Build and exercise the bundled binary visualizers with bounded host continuations."""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

if __name__ == "__main__":
    subprocess.run(["node", str(ROOT / "extensions/binary/build.mjs")], cwd=ROOT, check=True)
    subprocess.run(["node", "--experimental-vm-modules", "--test", str(ROOT / "extensions/binary/test/visualizers.test.mjs")], cwd=ROOT, check=True)
