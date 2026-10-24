"""Update or verify the PSL snapshot and certificate policy rules for a Chromium release.

Update: python scripts/update-chromium-psl.py --chromium-version 158.0.8117.2
Offline: python scripts/update-chromium-psl.py --check
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "third_party/chromium-suffix-list"
MAX_BYTES = 2 * 1024 * 1024


def generate_rules(data: bytes, *, include_private: bool = False) -> bytes:
    text = data.decode("utf-8")
    begin = "// ===BEGIN ICANN DOMAINS==="
    end = "// ===END ICANN DOMAINS==="
    if text.count(begin) != 1 or text.count(end) != 1:
        raise ValueError("PSL must contain exactly one ICANN section")
    section = text.split(begin, 1)[1].split(end, 1)[0]
    if include_private:
        private_begin = "// ===BEGIN PRIVATE DOMAINS==="
        private_end = "// ===END PRIVATE DOMAINS==="
        if text.count(private_begin) != 1 or text.count(private_end) != 1:
            raise ValueError("PSL must contain exactly one private section")
        section += text.split(private_begin, 1)[1].split(private_end, 1)[0]
    rules = []
    for line in section.splitlines():
        line = line.strip()
        if not line or line.startswith("//"):
            continue
        # PSL labels are already normalized. Encode Unicode labels as A-labels;
        # Python's built-in IDNA2003 would incorrectly map sharp-s to 'ss'.
        prefix = "!" if line.startswith("!") else ""
        body = line.removeprefix("!")
        labels = [label if label.isascii() else "xn--" + label.encode("punycode").decode("ascii")
                  for label in body.split(".")]
        rules.append(prefix + ".".join(labels).lower())
    if len(rules) < 1000 or len(rules) != len(set(rules)):
        raise ValueError("Unexpected PSL rule set")
    return ("\n".join(sorted(rules)) + "\n").encode("ascii")


def fetch(url: str) -> bytes:
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "Transmog-PSL-maintenance"}), timeout=30) as response:
        data = response.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise ValueError("PSL exceeds its download bound")
    return data


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--chromium-version", help="Exact four-part Chromium release")
    group.add_argument("--check", action="store_true", help="Verify generated rules and hashes without network access")
    parser.add_argument("--input", type=Path, help="Use an already downloaded source snapshot when updating")
    args = parser.parse_args()
    if args.check:
        if args.input:
            parser.error("--input is only valid with --chromium-version")
        manifest = json.loads((DEST / "snapshot.json").read_text(encoding="utf-8"))
        data = (DEST / "effective_tld_names.dat").read_bytes()
        rules = (DEST / "icann-rules.txt").read_bytes()
        if rules != generate_rules(data):
            raise SystemExit("Generated ICANN rules are stale; rerun the update script")
        if (DEST / "all-rules.txt").read_bytes() != generate_rules(data, include_private=True):
            raise SystemExit("Generated full PSL rules are stale; rerun the update script")
        for name in ("effective_tld_names.dat", "icann-rules.txt", "all-rules.txt", "LICENSE"):
            if hashlib.sha256((DEST / name).read_bytes()).hexdigest() != manifest["sha256"][name]:
                raise SystemExit(f"Hash mismatch: {name}")
        print(f"Verified Chromium {manifest['chromiumVersion']} PSL and {len(rules.splitlines())} ICANN rules")
        return
    if not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", args.chromium_version):
        parser.error("--chromium-version must be an exact four-part release, not a moving branch")
    source = (f"https://raw.githubusercontent.com/chromium/chromium/{args.chromium_version}/"
              "net/base/registry_controlled_domains/effective_tld_names.dat")
    data = args.input.read_bytes() if args.input else fetch(source)
    if len(data) > MAX_BYTES:
        raise ValueError("PSL exceeds its input bound")
    rules = generate_rules(data)
    # Validate and prepare everything before replacing the checked-in snapshot.
    license_data = (DEST / "LICENSE").read_bytes()
    outputs = {"effective_tld_names.dat": data, "icann-rules.txt": rules,
               "all-rules.txt": generate_rules(data, include_private=True), "LICENSE": license_data}
    manifest = {"chromiumVersion": args.chromium_version, "source": source,
                "sha256": {name: hashlib.sha256(value).hexdigest() for name, value in outputs.items()}}
    DEST.mkdir(parents=True, exist_ok=True)
    for name, value in outputs.items():
        (DEST / name).write_bytes(value)
    (DEST / "snapshot.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"Updated Chromium {args.chromium_version} PSL ({len(rules.splitlines())} ICANN rules). Run the hostname regression tests before committing.")


if __name__ == "__main__":
    main()
