"""Generate third-party notices and a CycloneDX BOM from locked dependencies."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
from urllib.parse import quote
from repo_tools import ROOT, command, main_guard, write_json


def package_key(package):
    # Preserve the established underscore-before-hyphen notice ordering.
    return (package["name"].replace("_", ","), package["version"].replace("_", ","))


def production_npm(lock):
    packages = []
    for path, package in lock.get("packages", {}).items():
        if not path or package.get("dev") or not package.get("version"):
            continue
        if "node_modules/" not in path:
            raise ValueError(f"Unexpected npm lockfile package path: {path}")
        packages.append(dict(name=path.rsplit("node_modules/", 1)[1], version=package["version"],
                             license=package.get("license", ""), source=package.get("resolved", "")))
    return sorted(packages, key=package_key)


def artifacts(metadata, npm, version):
    packages = sorted(metadata["packages"], key=package_key)
    third_party = [p for p in packages if p.get("source") is not None]
    lines = ["# Third-party notices", "",
             "Transmog includes the following third-party software.",
             "",
             "| Package | Version | License expression | Source |", "|---|---:|---|---|"]
    lines += [f"| {p['name']} | {p['version']} | {p.get('license') or ''} | {p.get('repository') or p['source']} |" for p in third_party]
    lines += ["", "## Desktop npm production packages", "",
              "| Package | Version | License expression | Source |", "|---|---:|---|---|"]
    lines += [f"| {p['name']} | {p['version']} | {p['license']} | {p['source']} |" for p in npm]
    lines += ["", "## Bundled native code", "",
              "`boring-sys` and `quiche` compile bundled BoringSSL/quiche source. Their upstream license files and notices remain part of the Cargo source distribution and must be retained in binary distribution materials."]
    snapshot = json.loads((ROOT / "third_party/chromium-root-store/snapshot.json").read_text(encoding="utf-8"))
    lines += ["", "## Chromium root-store data", "",
              f"The bundled crs.pb is generated from public Chromium source revision {snapshot['sourceRevision']} (root-store version {snapshot['rootStoreVersion']}). It supplies classical Trust Anchor IDs and untrusted public chain-building certificates; it does not replace configured certificate trust.", "",
              "Source and reproducible generator: `third_party/chromium-root-store` and `scripts/generate-chromium-root-store.py`. Google’s PKIMetadata CRX package is not redistributed.", "",
              (ROOT / "third_party/chromium-root-store/LICENSE").read_text(encoding="utf-8").rstrip()]
    components = []
    for package in packages:
        component = {"type": "library" if package.get("source") else "application", "bom-ref": package["id"],
                     "name": package["name"], "version": package["version"],
                     "licenses": [{"license": {"expression": package.get("license")}}],
                     "purl": f"pkg:cargo/{quote(package['name'], safe='')}@{quote(package['version'], safe='')}"}
        if package.get("repository"):
            component["externalReferences"] = [{"type": "vcs", "url": package["repository"]}]
        components.append(component)
    for package in npm:
        components.append({"type": "library", "bom-ref": f"npm:{package['name']}@{package['version']}",
            "name": package["name"], "version": package["version"],
            "licenses": [{"license": {"expression": package["license"]}}],
            "purl": f"pkg:npm/{quote(package['name'], safe='/')}@{quote(package['version'], safe='')}",
            "externalReferences": [{"type": "distribution", "url": package["source"]}]})
    components.append({"type": "data", "bom-ref": "chromium-root-store", "name": "Chromium root-store data",
                       "version": str(snapshot["rootStoreVersion"]),
                       "licenses": [{"license": {"id": "BSD-3-Clause"}}],
                       "hashes": [{"alg": "SHA-256", "content": snapshot["protobufSha256"]}],
                       "externalReferences": [{"type": "vcs", "url": snapshot["source"]}]})
    sbom = {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
            "metadata": {"timestamp": datetime.now(timezone.utc).isoformat(),
                         "tools": [{"vendor": "Transmog", "name": "generate-supply-chain-artifacts.py"}],
                         "component": {"type": "application", "name": "transmog", "version": version}},
            "components": components,
            "dependencies": [{"ref": node["id"], "dependsOn": node["dependencies"]} for node in metadata["resolve"]["nodes"]]}
    return "\n".join(lines) + "\n", sbom


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--notice-path", type=Path, default=ROOT / "THIRD_PARTY_NOTICES.md")
    parser.add_argument("--sbom-path", type=Path, default=ROOT / "artifacts/sbom.cdx.json")
    args = parser.parse_args()
    metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked"], capture=True).stdout)
    version = json.loads((ROOT / "release-version.json").read_text(encoding="utf-8"))["version"]
    lock = ROOT / "apps/desktop/ui/package-lock.json"
    npm = production_npm(json.loads(lock.read_text(encoding="utf-8"))) if lock.is_file() else []
    notice, sbom = artifacts(metadata, npm, version)
    args.notice_path.parent.mkdir(parents=True, exist_ok=True)
    args.notice_path.write_text(notice, encoding="utf-8", newline="\n")
    write_json(args.sbom_path, sbom)
    print(f"NOTICE_PATH={args.notice_path.resolve()}")
    print(f"SBOM_PATH={args.sbom_path.resolve()}")


if __name__ == "__main__":
    main_guard(main)
