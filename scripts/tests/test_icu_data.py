"""Keep ICU data filtering independent of language and locale behavior."""
import struct
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from icu_data import filter_package, read_package, write_package
from v8_native import source_pin
import v8_native


def fixture():
    header = bytearray(32)
    struct.pack_into("<H", header, 0, len(header))
    header[2:4] = b"\xda\x27"
    header[4:6] = b"\x14\0"
    header[8:11] = b"\0\0\2"
    header[12:17] = b"CmnD\1"
    names = ["de.res", "coll/sv.res", "brkitr/thaidict.dict", "brkitr/word.brk",
             "zoneinfo64.res", "nfkc.nrm", "nfkc_cf.nrm", "nfkc_scf.nrm",
             "uts46.nrm", "uemoji.icu", "pnames.icu", "windows-1252.cnv"]
    resources = {"icudt78l/" + name: bytes([index]) * 32 for index, name in enumerate(names)}
    return write_package(bytes(header), resources)


class IcuDataTests(unittest.TestCase):
    def test_filter_preserves_every_other_resource_byte_and_the_header(self):
        data = fixture()
        policy = source_pin()["icuData"]["filter"]
        self.assertEqual(policy, {"excludedSuffixes": [".cnv"], "excludedFiles": ["nfkc_scf.nrm"]})
        header, original = read_package(data)
        result_header, retained = read_package(filter_package(data, policy))
        self.assertEqual(result_header, header)
        expected = {name: content for name, content in original.items()
                    if name not in {"icudt78l/windows-1252.cnv", "icudt78l/nfkc_scf.nrm"}}
        self.assertEqual(retained, expected)

    def test_bad_format_offsets_and_names_are_rejected(self):
        data = fixture()
        for mutation in ("magic", "format", "endian", "count", "name", "offset", "order"):
            with self.subTest(mutation=mutation):
                changed = bytearray(data)
                if mutation == "magic":
                    changed[2] = 0
                elif mutation == "format":
                    changed[12:16] = b"ToCP"
                elif mutation == "endian":
                    changed[8] = 1
                elif mutation == "count":
                    struct.pack_into("<I", changed, 32, 65537)
                elif mutation == "name":
                    struct.pack_into("<I", changed, 36, len(data))
                elif mutation == "offset":
                    struct.pack_into("<I", changed, 40, 3)
                else:
                    first_name = struct.unpack_from("<I", changed, 36)[0]
                    struct.pack_into("<I", changed, 44, first_name)
                with self.assertRaises(ValueError):
                    read_package(bytes(changed))

    def test_unknown_and_empty_filter_policies_are_rejected(self):
        for policy in ({"locales": ["en"]}, {"excludedSuffixes": [""], "excludedFiles": []}):
            with self.subTest(policy=policy), self.assertRaises(ValueError):
                filter_package(fixture(), policy)

    def test_cached_data_is_verified_and_filtered_output_is_pinned(self):
        original = fixture()
        policy = source_pin()["icuData"]["filter"]
        filtered = filter_package(original, policy)
        pin = {"revision": "fixture", "bytes": len(original), "sha256": hashlib.sha256(original).hexdigest(),
               "filter": policy, "filteredBytes": len(filtered), "filteredSha256": hashlib.sha256(filtered).hexdigest()}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            cache = root / ".tools/downloads/icu-fixture-icudtl.dat"
            cache.parent.mkdir(parents=True)
            cache.write_bytes(original)
            with patch.object(v8_native, "ROOT", root):
                v8_native.prepare_icu_data(source, pin)
                target = source / "third_party/icu/common/icudtl.dat"
                self.assertEqual(target.read_bytes(), filtered)
                target.unlink()
                with self.assertRaisesRegex(ValueError, "Filtered ICU"):
                    v8_native.prepare_icu_data(source, {**pin, "filteredSha256": "0" * 64})
                cache.write_bytes(b"unreviewed ICU data")
                with self.assertRaisesRegex(ValueError, "Cached ICU"):
                    v8_native.prepare_icu_data(source, pin)


if __name__ == "__main__":
    unittest.main()
