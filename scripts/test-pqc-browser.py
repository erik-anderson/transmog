"""Qualify PQC with an available Chromium browser, without pinning its version."""
import argparse
import tempfile
from pathlib import Path
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard, native_name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--channel", choices=("chromium", "chrome", "msedge"), default="chromium")
    parser.add_argument("--trust", choices=("spki", "nss"), default="spki",
                        help="nss uses an isolated Linux trust database without certificate-error overrides")
    args = parser.parse_args()
    env = development_environment()
    command(["cargo", "build", "--locked", "-p", "transmog-tls", "--example", "pqc-origin"], env=env)
    artifacts = ROOT / ".local/pqc-browser"
    artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="run-", dir=artifacts) as temporary:
        run = Path(temporary)
        command(["node", ROOT / "e2e/playwright/support/pqc-browser.mjs",
                 cargo_output(env) / "debug/examples" / native_name("pqc-origin"), run, args.channel, args.trust], env=env)
        # Keep public-only qualification evidence, independently of temporary fixture roots.
        (artifacts / "results.json").write_bytes((run / "results.json").read_bytes())


if __name__ == "__main__":
    main_guard(main)
