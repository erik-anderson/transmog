"""Verify the pinned protocol oracle before using it for JavaScript parity tests."""
import base64
import json
import shutil
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path

import mapi_reference as reference


def fixture(request_type, body, parser_type=None):
    return {"type": parser_type, "message": {"direction": "request", "requestType": request_type,
            "request": base64.b64encode(body).decode("ascii")}}


def walk(node):
    yield node
    for child in node["children"]:
        yield from walk(child)


class ReferenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.command = reference.oracle_command()

    def run_cases(self, cases):
        result = subprocess.run(self.command, input="".join(json.dumps(case) + "\n" for case in cases),
                                text=True, encoding="utf-8", capture_output=True, check=True, timeout=30)
        return [json.loads(line) for line in result.stdout.splitlines()]

    def test_exact_reference_inventory_and_unmodified_sources(self):
        self.assertEqual(len(reference.verify_sources()["files"]), 651)
        result = subprocess.run(self.command + ["--inventory"], capture_output=True,
                                text=True, encoding="utf-8", check=True, timeout=10)
        expected = json.loads((reference.ROOT / "extensions/mapi/test/reference-inventory.json").read_text(encoding="utf-8"))
        self.assertEqual(json.loads(result.stdout), expected["blocks"])

    def test_hash_and_inventory_drift_fail_before_compiling(self):
        with tempfile.TemporaryDirectory() as temporary:
            vendor = Path(temporary) / "reference"
            shutil.copytree(reference.VENDOR, vendor)
            source = vendor / "Source/BlockParser/BinaryParser.cs"
            original = source.read_bytes()
            source.write_bytes(original + b" ")
            with self.assertRaisesRegex(ValueError, "reference changed"):
                reference.verify_sources(vendor)
            source.write_bytes(original)
            (vendor / "Source/Unexpected.cs").write_bytes(b"")
            with self.assertRaisesRegex(ValueError, "inventory changed"):
                reference.verify_sources(vendor)

    def test_nonzero_fields_wide_integers_and_byte_ranges(self):
        connect = fixture("Connect", b"/o=Example\0" + struct.pack("<IIIII", 0x1020304, 1252, 1033, 1041, 0))
        integer = fixture("Execute", struct.pack("<q", 9007199254740993), "MAPIInspector.Parsers.PtypInteger64")
        guid = fixture("Execute", bytes.fromhex("33221100554477668899aabbccddeeff"), "MAPIInspector.Parsers.PtypGuid")
        nodes = self.run_cases([connect, integer, guid])
        self.assertEqual(nodes[0]["text"], "ConnectRequestBody")
        self.assertEqual(nodes[0]["size"], "31")
        flags = next(node for node in walk(nodes[0]) if node["text"].startswith("Flags:"))
        self.assertEqual((flags["raw"], flags["offset"], flags["size"]), ("16909060", "11", "4"))
        self.assertIn("9007199254740993", {node["raw"] for node in walk(nodes[1])})
        self.assertIn("00112233-4455-6677-8899-aabbccddeeff", {node["raw"] for node in walk(nodes[2])})
        self.assertFalse(any(node["type"] == "BlockParser.BlockException" for tree in nodes for node in walk(tree)))

    def test_unavailable_context_has_no_global_session_fallback(self):
        sources = reference.headless_sources()
        self.assertNotIn("FiddlerApplication", sources["SessionNavigator.cs"])
        self.assertNotIn("ParseCaptureFile", sources["MAPIParser.cs"])
        self.assertNotIn("JsonConvert", sources["Utilities.cs"])


if __name__ == "__main__":
    unittest.main()
