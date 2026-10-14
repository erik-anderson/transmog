"""Git/native provenance must identify the actual build, including patches."""
import copy
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from test_tools import load


class ProvenanceTests(unittest.TestCase):
    def fixture(self):
        module = load("generate-supply-chain-artifacts")
        receipt = json.loads((module.ROOT / "third_party/boringssl-pki/manifest.json").read_text(encoding="utf-8"))
        revision = receipt["boring_revision"]
        package = {"id": "sys", "name": "boring-sys", "version": receipt["boring_version"], "license": "MIT OR Apache-2.0",
                   "source": f"git+https://github.com/cloudflare/boring?rev={revision}#{revision}"}
        metadata = {"packages": [package, {"id": "native", "name": "transmog-tls-native", "version": "1"}],
                    "resolve": {"nodes": [{"id": "sys", "dependencies": []}, {"id": "native", "dependencies": ["sys"]}]}}
        native = {"repository": "https://github.com/cloudflare/boring", "revision": revision,
                  "boringssl_revision": receipt["revision"], "features": {"boring-sys": ["default"]},
                  "patches": {"applied.patch": "a" * 64, "inactive.patch": "b" * 64}, "applied_patches": ["applied.patch"]}
        return module, metadata, native

    def test_git_purl_and_notices_identify_commit_instead_of_registry_version(self):
        module, metadata, native = self.fixture()
        original = copy.deepcopy(metadata)
        notice, sbom = module.artifacts(metadata, [], "1", native=native)
        self.assertEqual(metadata, original)
        package = next(c for c in sbom["components"] if c["bom-ref"] == "sys")
        self.assertIn("?vcs_url=git%2Bhttps%3A", package["purl"])
        self.assertIn(native["revision"], package["purl"])
        self.assertIn("/tree/" + native["revision"], notice)
        self.assertEqual(package["licenses"], [{"expression": "MIT OR Apache-2.0"}])
        self.assertIn({"name": "transmog:git:revision", "value": native["revision"]}, package["properties"])
        metadata["packages"][0]["source"] = "registry+https://github.com/rust-lang/crates.io-index"
        _, registry = module.artifacts(metadata, [], "1")
        self.assertNotEqual(package["purl"], next(c for c in registry["components"] if c["bom-ref"] == "sys")["purl"])

    def test_native_base_applied_patch_hashes_and_dependency_edges_are_explicit(self):
        module, metadata, native = self.fixture()
        notice, sbom = module.artifacts(metadata, [], "1", native=native)
        components = {c["bom-ref"]: c for c in sbom["components"]}
        base = components["boringssl-native"]
        self.assertEqual(base["version"], native["boringssl_revision"])
        self.assertEqual(len(base["pedigree"]["patches"]), 1)
        self.assertTrue(base["pedigree"]["patches"][0]["diff"]["url"].endswith("/applied.patch"))
        for name, scope in [("applied.patch", "required"), ("inactive.patch", "excluded")]:
            patch = components["boringssl-patch:" + name]
            self.assertEqual(patch["scope"], scope)
            self.assertEqual(patch["hashes"][0]["content"], native["patches"][name])
        self.assertIn("| No | " + "b" * 64, notice)
        edges = {d["ref"]: d["dependsOn"] for d in sbom["dependencies"]}
        self.assertIn("boringssl-native", edges["sys"])
        self.assertIn("boringssl-pki", edges["native"])
        self.assertEqual(edges["boringssl-pki"], ["boringssl-native"])
        self.assertEqual(edges["boringssl-native"], ["boringssl-patch:applied.patch"])
        for source, targets in edges.items():
            self.assertIn(source, components)
            for target in targets:
                self.assertIn(target, components)

    def test_inconsistent_or_incomplete_provenance_is_rejected(self):
        module, metadata, native = self.fixture()
        native["applied_patches"].append("missing.patch")
        with self.assertRaisesRegex(ValueError, "applied patch hash"):
            module.artifacts(metadata, [], "1", native=native)
        native["revision"] = "f" * 40
        with self.assertRaisesRegex(ValueError, "source receipt"):
            module.artifacts(metadata, [], "1", native=native)
        metadata["packages"][0]["source"] = "git+https://github.com/cloudflare/boring#branch-name"
        with self.assertRaisesRegex(ValueError, "immutable resolved revision"):
            module.artifacts(metadata, [], "1")


if __name__ == "__main__":
    unittest.main()
