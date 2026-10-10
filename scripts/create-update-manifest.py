"""Sign final installer bytes and produce the Tauri update manifest."""
import argparse
import os
from pathlib import Path
import shutil
from release_version import semver
from repo_tools import ROOT, command, main_guard, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output-directory", type=Path, required=True)
    parser.add_argument("--notes", default="Download the verified release and update now, or when you quit Transmog.")
    args = parser.parse_args()
    semver(args.version)
    installer = args.installer.resolve(strict=True)
    expected = f"Transmog_{args.version}_x64-setup.exe"
    if installer.name != expected:
        raise ValueError("Unexpected updater installer name.")
    if not os.environ.get("TAURI_SIGNING_PRIVATE_KEY") and not os.environ.get("TAURI_SIGNING_PRIVATE_KEY_PATH"):
        raise ValueError("The protected release-signing environment needs an updater signing key.")
    # Capture signer output: key material and passwords must not enter logs.
    result = command(["node", ROOT / "apps/desktop/ui/node_modules/@tauri-apps/cli/tauri.js", "signer", "sign", "--app-version", args.version, installer], capture=True, check=False)
    if result.returncode:
        raise RuntimeError("Updater signing failed; check the protected signing key and password.")
    signature_path = installer.with_name(installer.name + ".sig")
    signature = signature_path.read_text(encoding="utf-8").strip()
    if not signature or len(signature) > 8192:
        raise ValueError("Unexpected updater signature size.")
    args.output_directory.mkdir(parents=True, exist_ok=True)
    destination = args.output_directory / signature_path.name
    if destination.resolve() != signature_path:
        shutil.copy2(signature_path, destination)
    write_json(args.output_directory / "latest.json", {"version": args.version, "notes": args.notes,
        "platforms": {"windows-x86_64-nsis": {"url": f"https://github.com/erik-anderson/transmog/releases/download/v{args.version}/{expected}", "signature": signature}}})
    print(f"Prepared the signed update manifest for Transmog {args.version}.")


if __name__ == "__main__":
    main_guard(main)
