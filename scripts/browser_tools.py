"""Shared isolated Chrome image used by the native protocol qualification tools."""
from pathlib import Path
import shutil
import tempfile
from docker_tools import build_image
from repo_tools import ROOT, command


def build_browser_image(image="transmog-pqc-browser", *, refresh=False):
    docker = build_image("transmog-linux-tools:rust-1.97.1-nightly-2026-10-01")
    with tempfile.TemporaryDirectory(prefix="transmog-browser-image-") as temporary:
        context = Path(temporary)
        shutil.copyfile(ROOT / "e2e/pqc/Dockerfile", context / "Dockerfile")
        for name in ("package.json", "package-lock.json"):
            shutil.copyfile(ROOT / "e2e/playwright" / name, context / name)
        for name in ("pqc-browser.mjs", "pqc-live.mjs"):
            shutil.copyfile(ROOT / "e2e/playwright/support" / name, context / name)
        command([docker, "build", *(["--no-cache"] if refresh else []), "--tag", image, context])
    return docker
