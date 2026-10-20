"""Generate third-party notices and a CycloneDX BOM from locked dependencies."""
import argparse
import base64
from datetime import datetime, timezone
import json
import hashlib
from pathlib import Path
import re
from urllib.parse import quote, urlsplit
from repo_tools import ROOT, command, main_guard, write_json
from transport_dependencies import resolved_provenance


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


def source_identity(package):
    """Keep Git builds distinct from the registry release of the same version."""
    source = package.get("source") or ""
    if not source.startswith("git+"):
        return None
    parsed = urlsplit(source[4:])
    revision = parsed.fragment
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError(f"Git source lacks an immutable resolved revision: {package['name']}")
    repository = f"{parsed.scheme}://{parsed.netloc}{parsed.path}"
    browser = repository.removesuffix(".git")
    url = f"{browser}/tree/{revision}" if parsed.netloc == "github.com" else f"{repository}#{revision}"
    return {"repository": repository, "revision": revision, "url": url, "source": source}


def license_choices(expression):
    return [{"expression": expression}] if expression else []


def artifacts(metadata, npm, version, *, native=None):
    packages = sorted(metadata["packages"], key=package_key)
    transports = resolved_provenance(metadata, ROOT)
    third_party = [p for p in packages if p.get("source") is not None or p["id"] in transports]
    lines = ["# Third-party notices", "",
             "Transmog includes the following third-party software.",
             "",
             "| Package | Version | License expression | Source |", "|---|---:|---|---|"]
    lines += [f"| {p['name']} | {p['version']} | {p.get('license') or ''} | {transports[p['id']]['archiveUrl'] if p['id'] in transports else (source_identity(p) or {}).get('url', p.get('repository') or p['source'])} |" for p in third_party]
    if transports:
        lines += ["", "## Patched Rust transport dependencies", "",
                  "The registry archives below are vendored with local patches. Original license and copyright texts are included in the desktop Credits.", "",
                  "| Package | Archive SHA-256 | Local patch | Patch SHA-256 |", "|---|---|---|---|"]
        for package in third_party:
            if package["id"] in transports:
                source = transports[package["id"]]
                lines.append(f"| {package['name']} | {source['archiveSha256']} | [{source['patchPath']}]({source['patchPath']}) | {source['patchSha256']} |")
    lines += ["", "## Desktop npm production packages", "",
              "| Package | Version | License expression | Source |", "|---|---:|---|---|"]
    lines += [f"| {p['name']} | {p['version']} | {p['license']} | {p['source']} |" for p in npm]
    lines += ["", "## Bundled native code", "",
              "`boring-sys` and `quiche` compile bundled BoringSSL/quiche source. Their upstream license files and notices remain part of the Cargo source distribution and must be retained in binary distribution materials."]
    pki = json.loads((ROOT / "third_party/boringssl-pki/manifest.json").read_text(encoding="utf-8"))
    psl = json.loads((ROOT / "third_party/chromium-suffix-list/snapshot.json").read_text(encoding="utf-8"))
    lines += ["", "## BoringSSL PKI", "",
              f"The unmodified PKI target from BoringSSL {pki['revision']} is compiled against the same boring-sys crypto instance. Source hashes and licenses: `third_party/boringssl-pki`.", "",
              (ROOT / "third_party/boringssl-pki/LICENSE").read_text(encoding="utf-8").rstrip(),
              "", "## Chromium Public Suffix List", "",
              f"Chromium {psl['chromiumVersion']}'s Public Suffix List is used for ICANN wildcard boundaries and fixed CA domain restrictions. Original source, generated ICANN and full rules, hashes and the MPL-2.0 license are in `third_party/chromium-suffix-list`.", "",
              (ROOT / "third_party/chromium-suffix-list/LICENSE").read_text(encoding="utf-8").rstrip()]
    snapshot = json.loads((ROOT / "third_party/chromium-root-store/snapshot.json").read_text(encoding="utf-8"))
    policy = json.loads((ROOT / "third_party/chromium-certificate-policy/snapshot.json").read_text(encoding="utf-8"))
    lines += ["", "## Chromium certificate policy data", "",
              f"Chromium {policy['chromiumVersion']} certificate blocklist, public CT log list and wire schemas are in `third_party/chromium-certificate-policy`. The generated ct_config.pb supplies the shared verifier's CT baseline; installed browser component packages are not bundled.", "",
              (ROOT / "third_party/chromium-certificate-policy/LICENSE").read_text(encoding="utf-8").rstrip()]
    lines += ["", "## Chromium root-store data", "",
              f"The bundled crs.pb is generated from public Chromium source revision {snapshot['sourceRevision']} (root-store version {snapshot['rootStoreVersion']}). It supplies certificate-based Trust Anchor IDs and public certificates. Network profiles can select these public TLS anchors for verification; OS mode uses them as untrusted chain-building helpers.", "",
              "Source and reproducible generator: `third_party/chromium-root-store` and `scripts/generate-chromium-root-store.py`. Google’s PKIMetadata CRX package is not redistributed.", "",
              (ROOT / "third_party/chromium-root-store/LICENSE").read_text(encoding="utf-8").rstrip()]
    mapi = json.loads((ROOT / "third_party/mapi-inspector/source.json").read_text(encoding="utf-8"))
    mapi_url = mapi["repository"] + "/tree/" + mapi["revision"]
    lines += ["", "## MAPI protocol reference data", "",
              f"The bundled TypeScript visualizer uses enum data from [Microsoft Office Inspectors for Fiddler]({mapi_url}).", "",
              (ROOT / "third_party/mapi-inspector/LICENSE").read_text(encoding="utf-8").rstrip()]
    components = [{"type": "data", "bom-ref": "mapi-reference-data", "name": "Microsoft MAPI protocol enum data",
                   "version": mapi["revision"], "licenses": [{"license": {"id": "MIT"}}],
                   "hashes": [{"alg": "SHA-256", "content": hashlib.sha256((ROOT / "extensions/mapi/typescript/constants.ts").read_bytes()).hexdigest()}],
                   "externalReferences": [{"type": "vcs", "url": mapi_url}]}]
    for package in packages:
        transport = transports.get(package["id"])
        component = {"type": "library" if package.get("source") or transport else "application", "bom-ref": package["id"],
                     "name": package["name"], "version": package["version"],
                     "licenses": license_choices(package.get("license")),
                     "purl": f"pkg:cargo/{quote(package['name'], safe='')}@{quote(package['version'], safe='')}"}
        if package.get("repository"):
            component["externalReferences"] = [{"type": "vcs", "url": package["repository"]}]
        identity = source_identity(package)
        if identity:
            component["purl"] += "?vcs_url=" + quote("git+" + identity["repository"] + "@" + identity["revision"], safe="")
            component["externalReferences"] = [{"type": "vcs", "url": identity["url"]}]
            component["pedigree"] = {"commits": [{"uid": identity["revision"], "url": identity["url"]}]}
            component["properties"] = [{"name": "transmog:cargo:resolvedSource", "value": identity["source"]},
                                       {"name": "transmog:git:revision", "value": identity["revision"]}]
        if transport:
            ancestor = {"type": "library", "name": package["name"], "version": package["version"],
                        "purl": component["purl"],
                        "hashes": [{"alg": "SHA-256", "content": transport["archiveSha256"]}],
                        "externalReferences": [{"type": "distribution", "url": transport["archiveUrl"]}]}
            patch = (ROOT / transport["patchPath"]).read_bytes()
            component["purl"] += "?patch_sha256=" + transport["patchSha256"]
            component["pedigree"] = {"ancestors": [ancestor], "patches": [{"type": "unofficial", "diff": {
                "text": {"contentType": "text/x-diff", "encoding": "base64", "content": base64.b64encode(patch).decode("ascii")}}}]}
            component.setdefault("externalReferences", []).append({"type": "distribution", "url": transport["archiveUrl"]})
            component["properties"] = [{"name": "transmog:transport:" + key, "value": transport[key]}
                                       for key in ("archiveSha256", "patchPath", "patchSha256", "snapshotSha256")]
        components.append(component)
    for package in npm:
        components.append({"type": "library", "bom-ref": f"npm:{package['name']}@{package['version']}",
            "name": package["name"], "version": package["version"],
            "licenses": license_choices(package["license"]),
            "purl": f"pkg:npm/{quote(package['name'], safe='/')}@{quote(package['version'], safe='')}",
            "externalReferences": [{"type": "distribution", "url": package["source"]}]})
    components.append({"type": "data", "bom-ref": "chromium-root-store", "name": "Chromium root-store data",
                       "version": str(snapshot["rootStoreVersion"]),
                       "licenses": [{"license": {"id": "BSD-3-Clause"}}],
                       "hashes": [{"alg": "SHA-256", "content": snapshot["protobufSha256"]}],
                       "externalReferences": [{"type": "vcs", "url": snapshot["source"]}]})
    components += [{"type": "library", "bom-ref": "boringssl-pki", "name": "BoringSSL PKI",
                    "version": pki["revision"], "licenses": [{"license": {"id": "Apache-2.0"}}],
                    "externalReferences": [{"type": "vcs", "url": "https://boringssl.googlesource.com/boringssl/+/" + pki["revision"]}]},
                   {"type": "data", "bom-ref": "chromium-public-suffix-list", "name": "Chromium Public Suffix List",
                    "version": psl["chromiumVersion"], "licenses": [{"license": {"id": "MPL-2.0"}}],
                    "hashes": [{"alg": "SHA-256", "content": psl["sha256"]["effective_tld_names.dat"]}],
                    "externalReferences": [{"type": "vcs", "url": psl["source"]}]}]
    components.append({"type": "data", "bom-ref": "chromium-certificate-policy", "name": "Chromium certificate policy data",
                       "version": policy["chromiumVersion"], "licenses": [{"license": {"id": "BSD-3-Clause"}}],
                       "hashes": [{"alg": "SHA-256", "content": hashlib.sha256((ROOT / "third_party/chromium-certificate-policy/ct_config.pb").read_bytes()).hexdigest()},
                                  {"alg": "SHA-256", "content": policy["sha256"]["cert_verify_proc_blocklist.inc"]}],
                       "externalReferences": [{"type": "vcs", "url": f"https://github.com/chromium/chromium/tree/{policy['chromiumVersion']}/net/cert"}]})
    dependencies = [{"ref": node["id"], "dependsOn": list(node["dependencies"])} for node in metadata["resolve"]["nodes"]]
    for node in dependencies:
        package = next((p for p in packages if p["id"] == node["ref"]), None)
        if package and package["name"] == "transmog-desktop":
            node["dependsOn"].append("mapi-reference-data")
    dependencies.append({"ref": "mapi-reference-data", "dependsOn": []})
    if native is not None:
        if native["boringssl_revision"] != pki["revision"] or native["revision"] != pki["boring_revision"]:
            raise ValueError("Native provenance disagrees with the PKI source receipt")
        base_url = "https://boringssl.googlesource.com/boringssl/+/" + native["boringssl_revision"]
        prefix = native["repository"] + "/blob/" + native["revision"] + "/boring-sys/patches/"
        applied = native["applied_patches"]
        if not set(applied).issubset(native["patches"]):
            raise ValueError("Native provenance omits an applied patch hash")
        components.append({"type": "library", "bom-ref": "boringssl-native", "name": "BoringSSL SSL/crypto",
                           "version": native["boringssl_revision"], "licenses": [{"license": {"id": "Apache-2.0"}}],
                           "externalReferences": [{"type": "vcs", "url": base_url}],
                           "pedigree": {"commits": [{"uid": native["boringssl_revision"], "url": base_url}],
                                        "patches": [{"type": "unofficial", "diff": {"url": prefix + name}} for name in applied]},
                           "properties": [{"name": "transmog:boring:revision", "value": native["revision"]},
                                          {"name": "transmog:boring:enabledFeatures", "value": json.dumps(native["features"], sort_keys=True)}]})
        lines += ["", "## Qualified BoringSSL native source and patches", "",
                  f"SSL/crypto source: {base_url}. Rust bindings: {native['repository']}/tree/{native['revision']}.", "",
                  "Only the patches marked applied below are included in this qualified native build.", "",
                  "| Upstream patch | Applied | SHA-256 |", "|---|---|---|"]
        for name, digest in sorted(native["patches"].items()):
            components.append({"type": "file", "bom-ref": "boringssl-patch:" + name, "name": name,
                               "version": native["revision"], "scope": "required" if name in applied else "excluded",
                               "hashes": [{"alg": "SHA-256", "content": digest}],
                               "externalReferences": [{"type": "vcs", "url": prefix + name}]})
            lines.append(f"| [{name}]({prefix + name}) | {'Yes' if name in applied else 'No'} | {digest} |")
        dependencies += [{"ref": "boringssl-native", "dependsOn": ["boringssl-patch:" + name for name in applied]},
                         {"ref": "boringssl-pki", "dependsOn": ["boringssl-native"]}]
        for node in dependencies:
            package = next((p for p in packages if p["id"] == node["ref"]), None)
            if package:
                if package["name"] == "boring-sys":
                    node["dependsOn"].append("boringssl-native")
                elif package["name"] == "transmog-tls-native":
                    node["dependsOn"].append("boringssl-pki")
    sbom = {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
            "metadata": {"timestamp": datetime.now(timezone.utc).isoformat(),
                         "tools": [{"vendor": "Transmog", "name": "generate-supply-chain-artifacts.py"}],
                         "component": {"type": "application", "name": "transmog", "version": version}},
            "components": components,
            "dependencies": dependencies}
    return "\n".join(line.rstrip() for line in "\n".join(lines).splitlines()) + "\n", sbom


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--notice-path", type=Path, default=ROOT / "THIRD_PARTY_NOTICES.md")
    parser.add_argument("--sbom-path", type=Path, default=ROOT / "artifacts/sbom.cdx.json")
    args = parser.parse_args()
    metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked"], capture=True).stdout)
    version = json.loads((ROOT / "release-version.json").read_text(encoding="utf-8"))["version"]
    lock = ROOT / "apps/desktop/ui/package-lock.json"
    npm = production_npm(json.loads(lock.read_text(encoding="utf-8"))) if lock.is_file() else []
    from tls_dependencies import verify_stack, reject_native_overrides
    import os
    reject_native_overrides(os.environ)
    native = verify_stack(metadata, ROOT)
    notice, sbom = artifacts(metadata, npm, version, native=native)
    args.notice_path.parent.mkdir(parents=True, exist_ok=True)
    args.notice_path.write_text(notice, encoding="utf-8", newline="\n")
    write_json(args.sbom_path, sbom)
    print(f"NOTICE_PATH={args.notice_path.resolve()}")
    print(f"SBOM_PATH={args.sbom_path.resolve()}")


if __name__ == "__main__":
    main_guard(main)
