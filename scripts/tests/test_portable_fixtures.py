"""Check K0 corpus facts. Future package acceptance is not implemented here."""
import base64
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path
import struct
import sys
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import portable_fixtures as fixtures


class PortableDesignFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = fixtures.CORPUS
        cls.catalog = json.loads((cls.directory / "catalog.json").read_text(encoding="utf-8"))

    def manifest(self, name):
        return json.loads((self.directory / "manifests" / f"{name}.json").read_text(encoding="utf-8"))

    def object_bytes(self, sha):
        return (self.directory / fixtures.member(sha)).read_bytes()

    def test_generated_corpus_and_seed_content_identities_are_current(self):
        recipes = fixtures.build()
        expected_names = set(recipes) | {"README.md"} | {"seeds/" + name for name in fixtures.SEED_NAMES}
        self.assertEqual({path.relative_to(self.directory).as_posix() for path in self.directory.rglob("*") if path.is_file()}, expected_names)
        for name, expected in recipes.items():
            with self.subTest(name=name):
                self.assertEqual((self.directory / name).read_bytes(), expected)
        for name, metadata in self.catalog["files"].items():
            body = (self.directory / name).read_bytes()
            self.assertEqual(str(len(body)), metadata["bytes"])
            self.assertEqual(hashlib.sha256(body).hexdigest(), metadata["sha256"])
        for name, metadata in self.catalog["captureSeeds"].items():
            self.assertEqual(self.object_bytes(metadata["sha256"]), (self.directory / "seeds" / name).read_bytes())

    def test_plain_package_contains_only_manifest_and_declared_stored_objects(self):
        path = self.directory / "packages/simple.tmcase"
        with zipfile.ZipFile(path) as archive:
            manifest = json.loads(archive.read("manifest.json"))
            self.assertEqual(archive.namelist(), ["manifest.json"] + [fixtures.member(item["sha256"]) for item in manifest["objects"]])
            for item in archive.infolist():
                self.assertEqual(item.compress_type, zipfile.ZIP_STORED)
                self.assertEqual(item.flag_bits & 1, 0)
            for item in manifest["objects"]:
                body = archive.read(fixtures.member(item["sha256"]))
                self.assertEqual(str(len(body)), item["bytes"])
                self.assertEqual(hashlib.sha256(body).hexdigest(), item["sha256"])
        revision = manifest["experiments"][0]["variants"][1]["revisions"][0]["rules"][0]["revision"]
        self.assertIsInstance(revision, str)
        self.assertGreater(int(revision), 2**53)

    def test_encoded_entity_and_ordered_duplicate_binary_headers_are_preserved(self):
        manifest = self.manifest("coded-duplicate-headers")
        css, script = manifest["assets"]
        headers = [(row["name"], base64.b64decode(row["valueBase64"], validate=True)) for row in css["headers"]]
        self.assertEqual(headers[-3:], [("X-Duplicate", b"one"), ("x-duplicate", b"two"), ("X-Bytes", b"\x80\xff")])
        body = self.object_bytes(script["body"]["sha256"])
        self.assertEqual(gzip.decompress(body), b'globalThis.portableFixture = "synthetic";\n')
        self.assertNotEqual(hashlib.sha256(gzip.decompress(body)).hexdigest(), script["body"]["sha256"])
        self.assertEqual(script["body"]["representation"], "encoded-entity")

    def test_zip64_streamed_archive_preserves_bytes_with_real_size_fields_and_descriptors(self):
        path = self.directory / "packages/zip64-streamed.tmcase"
        raw = path.read_bytes()
        with zipfile.ZipFile(path) as streamed, zipfile.ZipFile(self.directory / "packages/simple.tmcase") as plain:
            self.assertEqual(streamed.namelist(), plain.namelist())
            for item in streamed.infolist():
                with self.subTest(member=item.filename):
                    self.assertEqual(streamed.read(item), plain.read(item.filename))
                    self.assertEqual(item.compress_type, zipfile.ZIP_STORED)
                    self.assertEqual(item.extract_version, 45)
                    self.assertEqual(item.flag_bits & 8, 8)
                    offset = item.header_offset
                    self.assertEqual(struct.unpack_from("<II", raw, offset + 18), (0xffffffff, 0xffffffff))
                    name_bytes, extra_bytes = struct.unpack_from("<HH", raw, offset + 26)
                    extra_offset = offset + 30 + name_bytes
                    self.assertEqual(extra_bytes, 20)
                    self.assertEqual(struct.unpack_from("<HHQQ", raw, extra_offset), (1, 16, 0, 0))
                    descriptor = extra_offset + extra_bytes + item.file_size
                    self.assertEqual(raw[descriptor:descriptor + 4], b"PK\x07\x08")
                    self.assertEqual(struct.unpack_from("<IQQ", raw, descriptor + 4), (item.CRC, item.file_size, item.file_size))

    def test_missing_is_not_an_empty_or_present_asset(self):
        manifest = self.manifest("missing-asset")
        body = manifest["assets"][0]["body"]
        self.assertEqual(body["state"], "missing")
        self.assertTrue(body["reason"])
        self.assertNotIn("sha256", body)
        self.assertNotIn("bytes", body)
        self.assertNotIn(body["expectedSha256"], {item["sha256"] for item in manifest["objects"]})

    def test_linked_export_snapshot_cannot_stand_in_for_earlier_run_bytes(self):
        manifest = self.manifest("linked-file-changed")
        asset = manifest["assets"][0]
        historical = manifest["runs"][0]["responses"][0]
        self.assertEqual(self.object_bytes(asset["body"]["sha256"]), b"h1 { color: blue; }\n")
        self.assertEqual(self.object_bytes(historical["body"]["sha256"]), b"h1 { color: green; }\n")
        self.assertEqual(asset["provenance"]["kind"], "export-snapshot")
        self.assertEqual(historical["provenance"], "retained-client-response")
        self.assertNotIn("path", asset["provenance"])
        self.assertEqual(manifest["runs"][0]["outcome"]["origin"], "authored")

    def test_same_labels_and_original_ids_are_disambiguated_by_source_bytes(self):
        manifest = self.manifest("source-collision")
        first, second = [note["source"] for note in manifest["notes"]]
        self.assertNotEqual(first["artifactSha256"], second["artifactSha256"])
        for key in ("sourceKey", "exchangeId", "occurrence", "boundary"):
            self.assertEqual(first[key], second[key])
        self.assertEqual(manifest["sources"][0]["label"], manifest["sources"][1]["label"])
        self.assertEqual(first, self.manifest("simple-css")["notes"][0]["source"])

    def test_derived_content_keeps_an_explicit_unavailable_parent(self):
        manifest = self.manifest("derived-evidence")
        source = manifest["sources"][0]
        parent = source["lineage"]["parents"][0]
        self.assertNotEqual(source["artifactSha256"], parent["artifactSha256"])
        self.assertEqual(parent["state"], "not-exported")
        self.assertNotIn(parent["artifactSha256"], {item["sha256"] for item in manifest["objects"]})
        self.assertEqual(manifest["notes"][0]["source"]["artifactSha256"], source["artifactSha256"])

    def test_encrypted_native_source_is_distinct_from_a_case_envelope(self):
        manifest = self.manifest("encrypted-capture")
        source = manifest["sources"][0]
        body = self.object_bytes(source["artifactSha256"])
        self.assertEqual(body[:9], b"TMCAP001\0")
        length = struct.unpack("<I", body[9:13])[0]
        header = json.loads(body[13:13+length])
        self.assertEqual(header["cipher"], "aes-256-gcm")
        self.assertEqual(source["dependency"]["state"], "locked")
        self.assertNotIn(self.catalog["fixturePassword"], json.dumps(manifest))

    def test_configuration_snapshot_preserves_origin_partial_output_and_inert_labels(self):
        manifest = self.manifest("network-context")
        snapshot = manifest["contexts"][0]
        self.assertEqual(snapshot["origin"]["kind"], "collected-for-case")
        self.assertEqual(snapshot["origin"]["purpose"], "case-save")
        self.assertEqual(snapshot["origin"]["machineLabel"], "SYNTHETIC-SAVE-MACHINE")
        self.assertNotIn("artifactSha256", snapshot["origin"])
        self.assertEqual(snapshot["completeness"], "partial")
        self.assertTrue(snapshot["issues"])
        content = self.object_bytes(snapshot["content"]["sha256"])
        self.assertEqual(str(len(content)), snapshot["content"]["bytes"])
        output = json.loads(content)
        self.assertEqual(output["command"], "ipconfig /all")
        self.assertIn("synthetic-café\r\nIPv4 Address: 192.0.2.10\r\n", output["output"])
        self.assertIn("SYNTHETIC", output["notes"][0])
        self.assertEqual(manifest["sources"], self.manifest("simple-css")["sources"])

    def test_future_outcomes_and_match_unknowns_are_explicit_design_examples(self):
        scenarios = {item["id"]: item for item in self.catalog["scenarios"]}
        self.assertEqual(self.manifest("future-version")["schemaVersion"], 1)
        self.assertEqual(scenarios["future-version"]["expected"]["mutations"], "none")
        self.assertIn("fixture.future-required/999", self.manifest("future-required")["requiredFeatures"])
        self.assertEqual(self.manifest("future-optional")["experiments"][-1]["kind"], "fixture.future/999")
        results = json.loads((self.directory / "examples/match-boundaries.json").read_text(encoding="utf-8"))
        self.assertEqual([row["result"] for row in results["data"]["samples"]], ["match", "unknown", "unknown"])
        self.assertEqual(results["data"]["networkRequests"], "0")
        rules = self.manifest("rules-only")
        self.assertEqual(rules["kind"], "rules")
        self.assertFalse({"sources", "notes", "runs", "contexts"}.intersection(rules))

    def test_hostile_corpus_contains_real_unsafe_names_collisions_and_corruption(self):
        for name, unsafe in (("traversal", "../outside"), ("drive-path", "C:/outside"), ("unlisted-member", "unexpected")):
            with zipfile.ZipFile(self.directory / f"packages/{name}.tmcase") as archive:
                self.assertIn(unsafe, archive.namelist())
        with zipfile.ZipFile(self.directory / "packages/duplicate-member.tmcase") as archive:
            self.assertEqual(Counter(archive.namelist())["manifest.json"], 2)
        with zipfile.ZipFile(self.directory / "packages/digest-mismatch.tmcase") as archive:
            manifest = json.loads(archive.read("manifest.json"))
            sha = manifest["assets"][0]["body"]["sha256"]
            self.assertNotEqual(hashlib.sha256(archive.read(fixtures.member(sha))).hexdigest(), sha)

    def test_corpus_covers_k0_promises_with_only_declared_synthetic_secrets(self):
        coverage = {value for scenario in self.catalog["scenarios"] for value in scenario["covers"]}
        self.assertTrue({"css-experiment", "duplicate-headers", "gzip", "missing-is-not-empty", "export-snapshot", "notes-privacy", "encrypted-source", "sse", "loss", "retained-prefix", "same-original-id", "future-version", "unknown-required-feature", "derived-source-identity", "configuration-output", "partial-context", "inert-tool-labels"}.issubset(coverage))
        self.assertIn("SYNTHETIC SECRET", self.manifest("sensitive-notes")["notes"][0]["text"])


if __name__ == "__main__":
    unittest.main()
