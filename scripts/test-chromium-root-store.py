"""Offline reproducibility and fail-closed checks for the public source encoder."""
import importlib.util
import hashlib
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("generator", Path(__file__).with_name("generate-chromium-root-store.py"))
generator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generator)


class SnapshotTests(unittest.TestCase):
    def test_reproducible_snapshot_and_pinned_inputs(self):
        receipt = json.loads((generator.DATA / "snapshot.json").read_text(encoding="utf-8"))
        output = (generator.DATA / "crs.pb").read_bytes()
        generator.main()
        self.assertEqual(output, (generator.DATA / "crs.pb").read_bytes())
        self.assertEqual(receipt, json.loads((generator.DATA / "snapshot.json").read_text(encoding="utf-8")))
        self.assertEqual(hashlib.sha256(output).hexdigest(), receipt["protobufSha256"])

    def test_encoder_rejects_unknown_policy_and_syntax(self):
        with self.assertRaises(KeyError):
            generator.encode(generator.parse("future_policy: true"), generator.STORE, {})
        with self.assertRaises(ValueError):
            generator.parse("trust_anchors { sha256_hex: \"missing\"")
        with self.assertRaises(ValueError):
            generator.parse("version_major: 41 @")
        with self.assertRaises(KeyError):
            generator.encode(generator.parse('trust_anchors { sha256_hex: "missing" }'), generator.STORE, {})


if __name__ == "__main__":
    unittest.main()
