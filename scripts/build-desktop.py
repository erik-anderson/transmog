"""Build desktop UI and native application helpers."""
import argparse
from repo_tools import ROOT, command, development_environment, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--configuration", choices=("Debug", "Release"), default="Debug")
    parser.add_argument("--locked-dependencies", action="store_true")
    args = parser.parse_args()
    env = development_environment()
    command(["npm", "run", "check"], cwd=ROOT / "apps/desktop/ui", env=env)
    argv = ["cargo", "build", "-p", "transmog-desktop", "-p", "transmog", "-p", "transmog-script-host", "-p", "transmog-preview-worker"]
    if args.configuration == "Release":
        argv.append("--release")
    if args.locked_dependencies:
        argv.append("--locked")
    command(argv, env=env)


if __name__ == "__main__":
    main_guard(main)
