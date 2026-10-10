"""Run opt-in live Playwright tests through the built proxy."""
import argparse
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard, native_name


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    env = development_environment()
    command(["cargo", "build", "--release", "--locked", "-p", "transmog"], env=env)
    project = ROOT / "e2e/playwright"
    command(["npm", "ci"], cwd=project, env=env)
    command(["node", "node_modules/playwright/cli.js", "install", "chromium"], cwd=project, env=env)
    env.update(TRANSMOG_BIN=str(cargo_output(env) / "release" / native_name("transmog-cli")), TRANSMOG_LIVE="1")
    command(["npm", "test"], cwd=project, env=env)


if __name__ == "__main__":
    main_guard(main)
