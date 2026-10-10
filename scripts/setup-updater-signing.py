"""Verify the backed-up updater keypair and optionally upload its protected secret."""
import argparse
import json
from pathlib import Path
from repo_tools import ROOT, command, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--private-key-path", type=Path, default=ROOT / ".local/updater.key")
    parser.add_argument("--upload", action="store_true")
    parser.add_argument("--github-cli-path", default="gh")
    args = parser.parse_args()
    if not args.private_key_path.is_file():
        raise ValueError("Restore the matching updater private key from its secure backup before uploading it.")
    config = json.loads((ROOT / "apps/desktop/tauri.conf.json").read_text(encoding="utf-8"))
    public = args.private_key_path.with_name(args.private_key_path.name + ".pub").read_text(encoding="utf-8").strip()
    if public != config["plugins"]["updater"]["pubkey"]:
        raise ValueError("The local keypair differs from the embedded updater public key; do not rotate it implicitly.")
    if args.upload:
        options = ["--repo", "erik-anderson/transmog", "--env", "release-signing"]
        existing = json.loads(command([args.github_cli_path, "secret", "list", *options, "--json", "name"], capture=True).stdout)
        if any(secret["name"] == "TRANSMOG_UPDATER_PRIVATE_KEY" for secret in existing):
            raise ValueError("The updater secret already exists; verify ownership before replacing it.")
        key = args.private_key_path.read_text(encoding="utf-8")
        result = command([args.github_cli_path, "secret", "set", "TRANSMOG_UPDATER_PRIVATE_KEY", *options],
                         input=key, capture=True, check=False)
        if result.returncode:
            raise RuntimeError("GitHub did not accept the protected updater signing secret.")
        print("Configured the updater private key in the protected release-signing environment.")
    else:
        print("The local updater keypair matches the embedded public key. Use --upload to configure release signing.")


if __name__ == "__main__":
    main_guard(main)
