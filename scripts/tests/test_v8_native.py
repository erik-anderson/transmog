"""Source/profile boundaries for the custom native V8 engine."""
import copy
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import hashlib
import io
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import threading
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import v8_dependencies
import v8_native
import v8_bindgen


@contextmanager
def source_fixture():
    """Exercise publication and real patch application without network/native tools."""
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        vendor = root / "vendor"
        vendor.mkdir()
        original = b"original binding\n"
        relative = "src/binding.cc"
        archive = root / "v8-1.crate"
        with tarfile.open(archive, "w:gz") as output:
            member = tarfile.TarInfo("v8-1/" + relative)
            member.size = len(original)
            output.addfile(member, io.BytesIO(original))
        pin = {"version": "1", "archive_sha256": v8_native.digest(archive),
               "icuData": {"filteredSha256": hashlib.sha256(b"icu").hexdigest()}}
        receipt = json.dumps({"archive_sha256": pin["archive_sha256"],
                              "files": {relative: hashlib.sha256(original).hexdigest()}}, indent=2) + "\n"
        (vendor / "source-receipt.sha256").write_text(hashlib.sha256(receipt.encode()).hexdigest())
        (vendor / "src").mkdir()
        (vendor / relative).write_bytes(b"patched binding\n")
        (vendor / "native.patch").write_text(
            "--- a/src/binding.cc\n+++ b/src/binding.cc\n@@ -1 +1 @@\n-original binding\n+patched binding\n",
            encoding="utf-8", newline="\n")

        def prepare_icu(source, _pin):
            target = source / "third_party/icu/common/icudtl.dat"
            target.parent.mkdir(parents=True)
            target.write_bytes(b"icu")

        with (patch.object(v8_native, "ROOT", root), patch.object(v8_native, "VENDOR", vendor),
              patch.object(v8_native, "source_pin", return_value=pin),
              patch.object(v8_native, "source_archive", return_value=archive),
              patch.object(v8_native, "prepare_icu_data", side_effect=prepare_icu),
              patch.object(v8_dependencies, "check_repository")):
            yield root, vendor, pin


class NativeSourceTests(unittest.TestCase):
    def test_clean_cache_and_changed_patch_have_separate_immutable_sources(self):
        with source_fixture() as (root, vendor, _pin):
            self.assertFalse((root / ".tools").exists())
            first = v8_native.prepare()
            self.assertEqual(v8_native.prepare(), first)
            (vendor / "src/binding.cc").write_bytes(b"other binding\n")
            patch_file = vendor / "native.patch"
            patch_file.write_text(patch_file.read_text().replace("+patched binding", "+other binding"),
                                  encoding="utf-8", newline="\n")
            second = v8_native.prepare()
            self.assertNotEqual(first, second)
            self.assertEqual((first / "src/binding.cc").read_bytes(), b"patched binding\n")
            self.assertEqual((second / "src/binding.cc").read_bytes(), b"other binding\n")

    def test_concurrent_first_publication_uses_one_complete_source(self):
        with source_fixture():
            barrier = threading.Barrier(2, timeout=30)
            original = v8_native.verify_source

            def verify(source, *args):
                original(source, *args)
                if source.name == "source":
                    barrier.wait()

            with patch.object(v8_native, "verify_source", side_effect=verify):
                with ThreadPoolExecutor(max_workers=2) as pool:
                    results = list(pool.map(lambda _: v8_native.prepare(), range(2)))
            self.assertEqual(results[0], results[1])
            self.assertEqual((results[0] / "src/binding.cc").read_bytes(), b"patched binding\n")

    def test_changed_icu_data_does_not_modify_a_previous_source(self):
        with source_fixture() as (_root, _vendor, pin):
            first = v8_native.prepare()
            pin["icuData"]["filteredSha256"] = hashlib.sha256(b"new icu").hexdigest()

            def changed_icu(source, _pin):
                target = source / "third_party/icu/common/icudtl.dat"
                target.parent.mkdir(parents=True)
                target.write_bytes(b"new icu")

            with patch.object(v8_native, "prepare_icu_data", side_effect=changed_icu):
                second = v8_native.prepare()
            self.assertNotEqual(first, second)
            self.assertEqual((first / "third_party/icu/common/icudtl.dat").read_bytes(), b"icu")
            self.assertEqual((second / "third_party/icu/common/icudtl.dat").read_bytes(), b"new icu")

    def test_existing_source_corruption_is_rejected_without_in_place_repair(self):
        for relative in ("src/binding.cc", "third_party/icu/common/icudtl.dat"):
            with self.subTest(relative=relative), source_fixture():
                source = v8_native.prepare()
                changed = source / relative
                changed.write_bytes(b"corrupt")
                with self.assertRaisesRegex(ValueError, "differs|different"):
                    v8_native.prepare()
                self.assertEqual(changed.read_bytes(), b"corrupt")

    def test_corrupt_registry_source_cannot_be_used(self):
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "registry/cache/fixture/v8-152.2.0.crate"
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b"unreviewed native engine")
            with self.assertRaisesRegex(ValueError, "checksum"):
                v8_native.source_archive(v8_native.source_pin(), cargo_home=temporary)

    def test_source_extraction_rejects_traversal_links_and_duplicates(self):
        for names, kind in [(["v8-1/../escape"], None), (["v8-1/link"], tarfile.SYMTYPE),
                            (["v8-1/source", "v8-1/source"], None)]:
            with self.subTest(names=names, kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / "source.crate"
                with tarfile.open(source, "w:gz") as archive:
                    for name in names:
                        info = tarfile.TarInfo(name)
                        if kind:
                            info.type, info.linkname = kind, "outside"
                            archive.addfile(info)
                        else:
                            info.size = 1
                            archive.addfile(info, io.BytesIO(b"a"))
                destination = root / "source"
                destination.mkdir()
                with self.assertRaises(ValueError):
                    v8_native.unpack(source, destination, "v8-1")
                self.assertFalse((root / "escape").exists())

    def test_profile_fork_has_archive_and_patch_provenance(self):
        package = {"id": "v8", "name": "v8", "version": v8_dependencies.VERSION, "source": None,
                   "manifest_path": str(v8_dependencies.ROOT / v8_dependencies.DIRECTORY / "Cargo.toml")}
        metadata = {"packages": [package]}
        provenance = v8_dependencies.resolved_provenance(metadata)["v8"]
        self.assertEqual(provenance["archiveSha256"], v8_dependencies.ARCHIVE_SHA256)
        self.assertEqual(provenance["propertyPrefix"], "transmog:v8:")
        for change in ({"source": "registry+https://github.com/rust-lang/crates.io-index"},
                       {"version": "0.0.0"}, {"manifest_path": str(Path.cwd() / "Cargo.toml")}):
            altered = copy.deepcopy(metadata)
            altered["packages"][0].update(change)
            with self.assertRaisesRegex(ValueError, "reviewed"):
                v8_dependencies.resolved_provenance(altered)

    def test_changed_native_profile_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            vendor = root / v8_dependencies.DIRECTORY
            shutil.copytree(v8_native.VENDOR, vendor)
            (vendor / "profile.gn").write_text("v8_enable_i18n_support = false\n")
            with self.assertRaisesRegex(ValueError, "snapshot"):
                v8_dependencies.check_repository(root)

    def test_sbom_retains_native_profile_and_source_identity(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("v8_sbom", v8_dependencies.ROOT / "scripts/generate-supply-chain-artifacts.py")
        generator = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(generator)
        package = {"id": "v8", "name": "v8", "version": v8_dependencies.VERSION, "source": None,
                   "manifest_path": str(v8_dependencies.ROOT / v8_dependencies.DIRECTORY / "Cargo.toml"),
                   "license": "MIT AND BSD-3-Clause"}
        notice, bom = generator.artifacts({"packages": [package], "resolve": {"nodes": [{"id": "v8", "dependencies": []}]}}, [], "1")
        component = next(c for c in bom["components"] if c["bom-ref"] == "v8")
        self.assertEqual(component["type"], "library")
        properties = {p["name"]: p["value"] for p in component["properties"]}
        self.assertIn("v8_enable_i18n_support = true", properties["transmog:v8:gnProfile"])
        self.assertIn('"excludedFiles": ["nfkc_scf.nrm"]', properties["transmog:v8:icuData"])
        self.assertEqual(properties["transmog:v8:nativeRevision"], "c4ca1eccb90c5464d826b7713dc27178a53b0cfe")
        self.assertIn("V8 native runtime", notice)
        self.assertIn("Abseil", notice)
        self.assertIn("FP16", properties["transmog:v8:nativeLicenseSources"])


class BindgenConfigurationTests(unittest.TestCase):
    def test_native_adapter_defines_headers_and_msvc_abi_reach_bindgen(self):
        project = {"targets": {"//:rusty_v8": {
            "defines": ["V8_INTL_SUPPORT", "V8_COMPRESS_POINTERS", "_HAS_EXCEPTIONS=0"],
            "include_dirs": ["//v8/include/", "C:/generated headers"],
            "cflags": ["-fmsc-version=1934", "-fcomplete-member-pointers", "/MD", "/O2"],
            "cflags_cc": ["/std:c++23preview", "/GR-"],
        }}}
        args = v8_bindgen.bindgen_arguments(project, Path("native source"), {"INCLUDE": "C:/MSVC;C:/SDK;"})
        for expected in ("-DV8_INTL_SUPPORT", "-DV8_COMPRESS_POINTERS", "-D_MT", "-D_DLL",
                         "-IC:/generated headers", "-fmsc-version=1934", "-std=c++23",
                         "-fno-rtti", "-fno-exceptions", "-isystemC:/MSVC", "-isystemC:/SDK",
                         "-fcomplete-member-pointers"):
            self.assertIn(expected, args)
        self.assertIn("-I" + str(Path("native source/v8/include")), args)
        self.assertNotIn("/O2", args)

    def test_static_crt_and_custom_libcxx_settings_are_retained(self):
        project = {"targets": {"//:rusty_v8": {
            "defines": ["_LIBCPP_ABI_VERSION=2"], "include_dirs": ["//third_party/libc++/src/include"],
            "cflags": ["/MT"], "cflags_cc": ["-std=c++23"],
        }}}
        args = v8_bindgen.bindgen_arguments(project, Path("source"), {})
        self.assertIn("-D_MT", args)
        self.assertNotIn("-D_DLL", args)
        self.assertIn("-D_LIBCPP_ABI_VERSION=2", args)
        self.assertIn("-std=c++23", args)


if __name__ == "__main__":
    unittest.main()
