"""Build and run the Chromium load harness with the current Python runtime."""
import argparse
import sys
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard, native_name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=("both", "cli", "desktop"), default="both")
    parser.add_argument("--storage", choices=("disk", "memory"), default="disk")
    parser.add_argument("--python", default=sys.executable)
    for flag in ("smoke", "headed", "skip-build", "skip-dependency-install", "skip-browser-install"):
        parser.add_argument("--" + flag, action="store_true")
    parser.add_argument("--navigations", type=int)
    parser.add_argument("--requests-per-navigation", type=int)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--timeout-minutes", type=int, default=30)
    parser.add_argument("--output", default="artifacts/load")
    args = parser.parse_args()
    for name, maximum in (("navigations", 10000), ("requests_per_navigation", 10000), ("concurrency", 32), ("timeout_minutes", 240)):
        value = getattr(args, name)
        if value is not None and not 1 <= value <= maximum:
            parser.error(f"--{name.replace('_', '-')} must be between 1 and {maximum}")
    env = development_environment() if not args.skip_build else None
    if not args.skip_build:
        packages = ["-p", "transmog"]
        if args.target != "cli":
            packages += ["-p", "transmog-desktop"]
        command(["cargo", "build", "--locked", "--release", *packages], env=env)
    if not args.skip_dependency_install:
        command(["npm", "--prefix", "e2e/playwright", "ci"], env=env)
    if not args.skip_browser_install:
        command(["node", "e2e/playwright/node_modules/playwright/cli.js", "install", "chromium"], env=env)
    command(["node", "--test", "e2e/load/harness.test.mjs", "e2e/output-run.test.mjs"], env=env)
    command([args.python, "-m", "unittest", "discover", "-s", "e2e/load", "-p", "test_*.py"], env=env)
    output = cargo_output(env) / "release"
    argv = ["node", "e2e/load/run.mjs", "--target", args.target, "--storage", args.storage,
            "--python", args.python, "--concurrency", args.concurrency, "--timeout-minutes", args.timeout_minutes,
            "--output", args.output, "--cli", output / native_name("transmog-cli"),
            "--desktop", output / native_name("transmog")]
    for flag in ("smoke", "headed"):
        if getattr(args, flag):
            argv.append("--" + flag)
    for name in ("navigations", "requests_per_navigation"):
        value = getattr(args, name)
        if value is not None or not args.smoke:
            argv += ["--" + name.replace("_", "-"), value if value is not None else 100]
    command(argv, env=env)


if __name__ == "__main__":
    main_guard(main)
