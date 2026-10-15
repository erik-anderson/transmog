"""Build/check the K0 design corpus. This is not a production package validator."""
import argparse
import base64
import copy
import gzip
import hashlib
import io
import json
from pathlib import Path
import warnings
import zipfile

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "verification/fixtures/portable"
CAPTURE_MEDIA = "application/vnd.transmog.tmcap"
SEED_NAMES = ("source-a.tmcap", "source-b.tmcap", "encrypted.tmcap")


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def field(name, value):
    return {"name": name, "valueBase64": base64.b64encode(value).decode("ascii")}


def member(sha):
    return "objects/sha256/" + sha


class StreamingBuffer(io.BytesIO):
    """Force ZIP data descriptors, as with writing directly to an encryptor."""

    def seek(self, *args):
        raise io.UnsupportedOperation("Synthetic non-seekable output")


def archive(entries, *, streamed=False, zip64=False):
    """Small deterministic Stored ZIP fixtures; never extract their names."""
    target = StreamingBuffer() if streamed else io.BytesIO()
    with zipfile.ZipFile(target, "w", allowZip64=True) as output:
        for name, data in entries:
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = 0o100600 << 16
            with warnings.catch_warnings():
                warnings.filterwarnings("ignore", message="Duplicate name:.*", category=UserWarning)
                with output.open(info, "w", force_zip64=zip64) as member_output:
                    member_output.write(data)
    return target.getvalue()


def build():
    """Recipes describe expected future outcomes, not implemented validation."""
    files, objects = {}, {}

    def obj(data, role, media_type):
        sha = digest(data)
        objects[sha] = data
        return {"sha256": sha, "bytes": str(len(data)), "role": role, "mediaType": media_type}

    captures = {}
    for name in SEED_NAMES:
        path = CORPUS / "seeds" / name
        if not path.is_file() or path.stat().st_size > 1024 * 1024:
            raise ValueError(f"Missing/oversized synthetic capture seed: {path}")
        captures[name] = obj(path.read_bytes(), "capture", CAPTURE_MEDIA)
    green = obj(b"h1 { color: green; }\n", "response-body", "text/css")
    blue = obj(b"h1 { color: blue; }\n", "response-body", "text/css")
    script = b'globalThis.portableFixture = "synthetic";\n'
    coded = bytearray(gzip.compress(script, mtime=0))
    coded[9] = 255  # Normalize gzip OS marker across supported Python hosts.
    compressed = obj(bytes(coded), "response-body", "text/javascript")
    source = captures["source-a.tmcap"]

    def reference(capture=source):
        return {"artifactSha256": capture["sha256"], "sourceKey": "root",
                "exchangeId": "00000000000000000000000000000001", "occurrence": "0",
                "boundary": "client-response"}

    def present(body):
        return {"state": "present", "sha256": body["sha256"], "bytes": body["bytes"]}

    rule = {"id": "rule-css", "revision": "9007199254740993", "label": "CSS hypothesis",
            "kind": "auto-response", "evaluation": "original-client-request", "enabledIntent": True,
            "match": {"method": "GET", "address": {"kind": "exact", "value": "https://portable.invalid/style.css"},
                      "requestHeaders": [], "examples": [{"method": "GET", "url": "https://portable.invalid:443/style.css", "headers": [], "expected": "match"}]},
            "action": {"assetId": "asset-css", "revision": "1"}}
    asset = {"id": "asset-css", "revision": "1", "status": 200,
             "headers": [field("Content-Type", b"text/css"), field("Cache-Control", b"max-age=60"),
                         field("X-Duplicate", b"one"), field("x-duplicate", b"two"), field("X-Bytes", b"\x80\xff")],
             "body": {**present(blue), "representation": "encoded-entity"},
             "provenance": {"kind": "authored", "capturedSource": reference()}}
    base = {"format": "transmog-investigation", "schemaVersion": 0, "kind": "case",
            "id": "case-css", "revision": "1", "title": "Synthetic CSS investigation · café",
            "createdAt": "2026-10-15T00:00:00Z", "producer": {"name": "transmog-design-fixtures", "version": "0.1.4"},
            "requiredFeatures": ["response-assets/1", "native-evidence/1"],
            "objects": [source, blue],
            "sources": [{"id": "source-a", "artifactSha256": source["sha256"], "sourceKey": "root", "label": "customer.tmcap",
                         "dependency": {"state": "present"}, "lineage": {"kind": "original"}, "evidenceCompleteness": "incomplete"}],
            "assets": [asset],
            "experiments": [{"id": "experiment-css", "kind": "response-variants", "requiredFeatures": ["response-assets/1"], "label": "Synthetic color change", "pausedIntent": False,
                             "variants": [{"id": "baseline", "label": "Baseline", "revisions": [{"revision": "1", "rules": []}]},
                                          {"id": "edited", "label": "Blue", "revisions": [{"revision": "1", "rules": [rule]}]}]}],
            "notes": [{"id": "note-steps", "text": "SYNTHETIC ONLY: select the stylesheet, compare, then explicitly preview. Observed issue marker is an authored observation.", "source": reference()}],
            "runs": [], "extensions": {"transmog.fixture": {"synthetic": True}}}
    catalog = {"corpusVersion": 0, "contract": "docs/portable-investigations.md", "scenarios": [],
               "captureSeeds": {name: value for name, value in captures.items()},
               "fixturePassword": "portable-fixture-password"}

    def scenario(name, manifest, expected, covers):
        path = f"manifests/{name}.json"
        files[path] = json_bytes(manifest)
        catalog["scenarios"].append({"id": name, "manifest": path, "expected": expected, "covers": covers})

    ordinary = {"read": "ok", "integrity": "valid", "evidence": "incomplete", "automationImport": "disabled", "preview": "unavailable-missing-html"}
    scenario("simple-css", base, ordinary, ["css-experiment", "passive-import", "large-integer", "retained-prefix", "loss", "sse"])
    encoded = copy.deepcopy(base)
    encoded["objects"].append(compressed)
    encoded["assets"].append({**copy.deepcopy(asset), "id": "asset-script", "headers": [field("Content-Type", b"text/javascript"), field("Content-Encoding", b"gzip"), field("X-Duplicate", b"one"), field("x-duplicate", b"two")], "body": {**present(compressed), "representation": "encoded-entity"}})
    scenario("coded-duplicate-headers", encoded, ordinary, ["encoded-entity", "gzip", "duplicate-headers", "binary-header-value"])
    missing = copy.deepcopy(base)
    missing["objects"] = [source]
    missing["assets"][0]["body"] = {"state": "missing", "reason": "Synthetic source removed before export", "expectedSha256": blue["sha256"], "representation": "encoded-entity"}
    scenario("missing-asset", missing, {**ordinary, "automationImport": "unavailable-missing-asset"}, ["missing-is-not-empty", "inspection-only"])
    linked = copy.deepcopy(base)
    linked["objects"].append(green)
    linked["assets"][0]["provenance"] = {"kind": "export-snapshot", "acquiredAt": "2026-10-15T00:00:00Z"}
    linked["runs"] = [{"id": "run-before-edit", "experimentId": "experiment-css", "variantId": "edited", "revision": "1",
                       "responses": [{"source": reference(), "body": {**present(green), "representation": "encoded-entity"}, "provenance": "retained-client-response"}],
                       "outcome": {"origin": "authored", "text": "Synthetic earlier file bytes differ from the later export snapshot."}}]
    scenario("linked-file-changed", linked, ordinary, ["historical-run-bytes", "export-snapshot", "no-filesystem-authority"])
    sensitive = copy.deepcopy(base)
    sensitive["notes"][0]["text"] = "SYNTHETIC SECRET: bearer=portable-demo; URL=https://portable.invalid/?token=demo; source path C:\\Synthetic\\café\\style.css. Review notes as well as headers."
    scenario("sensitive-notes", sensitive, {**ordinary, "privacyReview": "required"}, ["notes-privacy", "unicode", "no-secret-scrubber-promise"])
    context = copy.deepcopy(base)
    output = obj(json_bytes({"command": "ipconfig /all", "output": "SYNTHETIC ONLY\r\nHost Name: synthetic-café\r\nIPv4 Address: 192.0.2.10\r\n",
                             "notes": ["SYNTHETIC: route tool unavailable; context is partial."]}), "context", "application/json")
    context["objects"].append(output)
    context["contexts"] = [{"id": "context-save-network", "kind": "network-configuration",
                             "origin": {"kind": "collected-for-case", "collectedAt": "2026-10-15T00:00:00Z",
                                        "collector": {"name": "transmog-design-fixtures", "version": "0.1.4"},
                                        "platform": "windows", "purpose": "case-save", "machineLabel": "SYNTHETIC-SAVE-MACHINE"},
                             "content": present(output), "completeness": "partial", "issues": ["Synthetic unavailable route tool"]}]
    scenario("network-context", context, {**ordinary, "context": "partial", "privacyReview": "required"},
             ["configuration-output", "context-source-purpose", "partial-context", "inert-tool-labels", "notes-privacy"])
    encrypted = copy.deepcopy(base)
    encrypted_source = captures["encrypted.tmcap"]
    encrypted["objects"][0] = encrypted_source
    encrypted["sources"][0].update(artifactSha256=encrypted_source["sha256"], dependency={"state": "locked", "reason": "Separate source password required"})
    encrypted["notes"][0]["source"] = reference(encrypted_source)
    encrypted["assets"][0]["provenance"]["capturedSource"] = reference(encrypted_source)
    scenario("encrypted-capture", encrypted, {**ordinary, "evidence": "locked"}, ["encrypted-source", "separate-password-roles"])
    collision = copy.deepcopy(base)
    other = captures["source-b.tmcap"]
    collision["objects"].append(other)
    collision["sources"].append({**copy.deepcopy(base["sources"][0]), "id": "source-b", "artifactSha256": other["sha256"]})
    collision["notes"].append({"id": "other-source", "text": "Same source label/root/exchange ID, distinct artifact bytes", "source": reference(other)})
    scenario("source-collision", collision, ordinary, ["same-label", "same-original-id", "distinct-artifact-digests", "repeat-import-identity"])
    derived = copy.deepcopy(base)
    derived["objects"] = [other, blue]
    derived["sources"][0].update(artifactSha256=other["sha256"], lineage={"kind": "derived", "operations": ["synthetic-response-replacement"], "parents": [{"artifactSha256": source["sha256"], "state": "not-exported", "reason": "Synthetic original omitted"}]})
    derived["assets"][0]["provenance"]["capturedSource"] = reference(other)
    derived["notes"][0]["source"] = reference(other)
    scenario("derived-evidence", derived, ordinary, ["derived-source-identity", "omitted-parent", "no-source-backfill"])
    future = copy.deepcopy(base)
    future["schemaVersion"] = 1
    scenario("future-version", future, {"read": "unsupported-format", "mutations": "none"}, ["future-version", "no-migration"])
    required = copy.deepcopy(base)
    required["requiredFeatures"].append("fixture.future-required/999")
    scenario("future-required", required, {"read": "unsupported-feature", "mutations": "none"}, ["unknown-required-feature"])
    optional = copy.deepcopy(base)
    optional["experiments"].append({"id": "future-optional", "label": "Opaque future experiment", "kind": "fixture.future/999", "requiredFeatures": ["fixture.future/999"], "extensions": {"fixture.future": {"opaque": True}}})
    scenario("future-optional", optional, {**ordinary, "futureExperiment": "unavailable-unsupported"}, ["unknown-optional-experiment", "inspect-known-evidence"])
    rules = copy.deepcopy(base)
    rules["kind"] = "rules"
    rules["requiredFeatures"] = ["response-assets/1"]
    rules["objects"] = [blue]
    for name in ("sources", "notes", "runs"):
        del rules[name]
    rules["assets"][0]["provenance"] = {"kind": "authored"}
    scenario("rules-only", rules, {"read": "ok", "integrity": "valid", "automationImport": "disabled"}, ["rule-collection", "no-capture-required"])

    files["examples/match-boundaries.json"] = json_bytes({"resultVersion": 0, "command": "rules.test", "status": "ok", "diagnostics": [], "data": {
        "samples": [{"kind": "auto-response", "evaluation": "original-client-request", "result": "match"},
                    {"kind": "header-edit", "evaluation": "effective-response-head", "result": "unknown", "reason": "Only original request retained; prior head edits unavailable"},
                    {"kind": "literal-response", "evaluation": "final-effective-response-heads", "result": "unknown", "reason": "Recorded final effective response head unavailable"}], "networkRequests": "0", "scriptsExecuted": "0"}})
    manifest = json_bytes(base)
    entries = [("manifest.json", manifest)] + [(member(item["sha256"]), objects[item["sha256"]]) for item in base["objects"]]
    files["packages/simple.tmcase"] = archive(entries)
    files["packages/zip64-streamed.tmcase"] = archive(entries, streamed=True, zip64=True)
    catalog["scenarios"].append({"id": "zip64-streamed", "package": "packages/zip64-streamed.tmcase",
                                 "expected": ordinary, "covers": ["zip64-local-size-fields", "streaming-data-descriptors", "nonseekable-zip-output"]})
    attacks = {"traversal": [("../outside", b"synthetic")], "drive-path": [("C:/outside", b"synthetic")],
               "duplicate-member": [("manifest.json", manifest)], "unlisted-member": [("unexpected", b"synthetic")],
               "digest-mismatch": [(member(blue["sha256"]), b"corrupt synthetic bytes")]}
    for name, extra in attacks.items():
        selected = entries if name != "digest-mismatch" else [entry for entry in entries if entry[0] != member(blue["sha256"])]
        files[f"packages/{name}.tmcase"] = archive(selected + extra)
        catalog["scenarios"].append({"id": name, "package": f"packages/{name}.tmcase", "expected": {"read": "invalid-integrity" if name == "digest-mismatch" else "invalid-package", "mutations": "none"}, "covers": [name]})
    for sha, data in objects.items():
        files[member(sha)] = data
    catalog["files"] = {name: {"sha256": digest(data), "bytes": str(len(data))} for name, data in sorted(files.items())}
    files["catalog.json"] = json_bytes(catalog)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", action="store_true", help="Regenerate only the fixed synthetic design corpus")
    group.add_argument("--check", action="store_true", help="Check fixture bytes against the recipes without mutation")
    args = parser.parse_args()
    recipes = build()
    expected_names = set(recipes) | {"README.md"} | {"seeds/" + name for name in SEED_NAMES}
    unexpected = sorted(path.relative_to(CORPUS).as_posix() for path in CORPUS.rglob("*")
                        if path.is_file() and path.relative_to(CORPUS).as_posix() not in expected_names)
    if unexpected:
        raise SystemExit("Unlisted portable design fixtures (review/remove obsolete generated files explicitly): " + ", ".join(unexpected))
    stale = []
    for name, data in recipes.items():
        path = CORPUS / name
        if args.write:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        elif not path.is_file() or path.read_bytes() != data:
            stale.append(name)
    if stale:
        raise SystemExit("Stale portable design fixtures: " + ", ".join(stale))
    print("Portable design fixtures " + ("written." if args.write else "consistent; production case validation remains unimplemented."))


if __name__ == "__main__":
    main()
