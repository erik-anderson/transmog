"""Validate PQC and certificate verification with Chrome and isolated Linux trust."""
import argparse
from urllib.parse import urlsplit
from docker_tools import container_arguments
from browser_tools import build_browser_image
from repo_tools import ROOT, command, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="transmog-pqc-browser")
    parser.add_argument("--refresh-browser", action="store_true", help="Rebuild the browser image with the available Chrome release")
    parser.add_argument("--live-url", help="Opt-in public HTTPS site expected to negotiate X25519MLKEM768")
    args = parser.parse_args()
    if args.live_url:
        destination = urlsplit(args.live_url)
        if destination.scheme != "https" or not destination.hostname or destination.username or destination.password:
            parser.error("--live-url requires an HTTPS URL without credentials")
    docker = build_browser_image(args.image, refresh=args.refresh_browser)
    artifacts = ROOT / ".local/pqc-browser-verified"
    artifacts.mkdir(parents=True, exist_ok=True)
    argv = container_arguments(docker, args.image,
        environment=("CFLAGS=", "CXXFLAGS=", "CARGO_TARGET_DIR=/work-target"),
        mounts=("type=volume,source=transmog-linux-cargo-registry,target=/usr/local/cargo/registry",
                "type=volume,source=transmog-linux-target,target=/work-target",
                f"type=bind,source={artifacts},target=/evidence"))
    script = """set -euo pipefail
python3 scripts/dev-env.py --check
cargo build --locked -p transmog-tls --example pqc-origin
run=$(mktemp -d /tmp/transmog-pqc.XXXXXX)
trap 'rm -rf -- "$run"' EXIT
node /opt/pqc-browser/pqc-browser.mjs /work-target/debug/examples/pqc-origin "$run" chrome nss
cp "$run/results.json" /evidence/results.json
if [ "$#" -eq 1 ]; then
  cargo build --locked -p transmog --bins
  python3 -c 'import secrets; print(secrets.token_hex(32), end="")' | gnome-keyring-daemon --unlock --components=secrets >/dev/null
  node /opt/pqc-browser/pqc-live.mjs /work-target/debug/transmog-cli "$run" "$1"
  cp "$run/live-results.json" /evidence/live-results.json
fi
"""
    command([*argv, "dbus-run-session", "--", "bash", "-c", script, "pqc-validation", *([args.live_url] if args.live_url else [])])


if __name__ == "__main__":
    main_guard(main)
