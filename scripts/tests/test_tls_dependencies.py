"""Qualified TLS builds check resolved sources, features and native overrides."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import tls_dependencies as tls


class TlsDependencyTests(unittest.TestCase):
    def fixture(self, root):
        revision = "a" * 40
        receipt = root / "third_party/boringssl-pki/manifest.json"
        receipt.parent.mkdir(parents=True)
        receipt.write_text(json.dumps({"revision": "b" * 40, "boring_revision": revision, "boring_version": "5.2.0"}))
        text = "[workspace.dependencies]\n" + "".join(f'{n} = "=5.2.0"\n' for n in tls.FAMILY)
        text += "[patch.crates-io]\n" + "".join(f'{n} = {{ git = "{tls.REPOSITORY}", rev = "{revision}" }}\n' for n in tls.FAMILY)
        (root / "Cargo.toml").write_text(text)
        packages = [{"id": n, "name": n, "version": "5.2.0", "manifest_path": str(root / f"binding/{n}/Cargo.toml"),
                     "source": f"git+{tls.REPOSITORY}?rev={revision}#{revision}"} for n in tls.FAMILY]
        return {"packages": packages, "resolve": {"nodes": [{"id": n, "features": []} for n in tls.FAMILY]}}

    def test_same_version_registry_or_other_git_source_cannot_replace_the_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            tls.verify_stack(metadata, root, verify_checkout=False)
            for source in [None, "registry+https://github.com/rust-lang/crates.io-index",
                           f"git+{tls.REPOSITORY}?rev={'c'*40}#{'c'*40}"]:
                metadata["packages"][1]["source"] = source
                with self.subTest(source=source), self.assertRaisesRegex(ValueError, "qualified Git revision"):
                    tls.verify_stack(metadata, root, verify_checkout=False)

    def test_unqualified_native_features_and_quiche_provider_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            metadata = self.fixture(root)
            for feature in tls.UNSUPPORTED_FEATURES:
                metadata["resolve"]["nodes"][1]["features"] = [feature]
                with self.subTest(feature=feature), self.assertRaisesRegex(ValueError, "Unqualified"):
                    tls.verify_stack(metadata, root, verify_checkout=False)
            metadata["resolve"]["nodes"][1]["features"] = []
            metadata["packages"].append({"id": "quiche", "name": "quiche"})
            metadata["resolve"]["nodes"].append({"id": "quiche", "features": ["boringssl-boring-crate", "boringssl-vendored"]})
            with self.assertRaisesRegex(ValueError, "quiche"):
                tls.verify_stack(metadata, root, verify_checkout=False)

    def test_native_path_overrides_include_host_target_and_fips_spellings(self):
        for suffix in tls.OVERRIDES:
            for name in [f"BORING_BSSL_{suffix}", f"HOST_BORING_BSSL_{suffix}",
                         f"BORING_BSSL_FIPS_{suffix}_x86_64_pc_windows_msvc"]:
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, "qualified native"):
                    tls.reject_native_overrides({name: "arbitrary"})
        tls.reject_native_overrides({"CC": "clang", "BORING_BSSL_SYSROOT": "sdk"})

    def test_preflight_preserves_config_features_and_multiple_target_selection(self):
        metadata = {"packages": []}
        completed = type("Result", (), {"stdout": json.dumps(metadata)})()
        with patch.object(tls.subprocess, "run", return_value=completed) as run:
            tls.preflight("cargo", ["+stable", "test", "--config", "patch.crates-io.boring.path='other'", "-Fboring/fips",
                          "--target=x86_64-pc-windows-msvc", "--target", "x86_64-unknown-linux-gnu", "--", "--target=not-a-rust-target"], ".", {})
            self.assertEqual(run.call_count, 2)
            arguments = run.call_args_list[0].args[0]
            self.assertIn("--filter-platform", arguments)
            self.assertEqual(arguments[1], "+stable")
            self.assertNotIn("not-a-rust-target", arguments)
            self.assertNotIn("--target", arguments)
            self.assertIn("boring/fips", arguments)
            self.assertIn("patch.crates-io.boring.path='other'", arguments)

    def test_checkout_must_match_both_pinned_revisions(self):
        expected = {"revision": "a" * 40, "boringssl_revision": "b" * 40}
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            with patch.object(tls, "git", side_effect=["a" * 40, "?? .cargo-ok", f"160000 commit {'b'*40}\tboring-sys/deps/boringssl", "c" * 40]):
                with self.assertRaisesRegex(ValueError, "submodule has the wrong revision"):
                    tls.verify_source(source, expected)

    def test_patch_provenance_requires_review_when_selection_logic_changes(self):
        function = 'fn ensure_patches_applied(config: &Config) {\n    apply_patch(config, "example.patch");\n}\n'
        digest = hashlib.sha256(function.encode()).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = root / "boring-sys/build/main.rs"
            script.parent.mkdir(parents=True)
            script.write_text(function, encoding="utf-8")
            with patch.object(tls, "REVIEWED_PATCH_PROFILES", {digest: ["example.patch"]}):
                self.assertEqual(tls.applied_patches(root), ["example.patch"])
                # Same patch name, different condition: filename inventories
                # cannot tell us whether the native build still applies it.
                script.write_text(function.replace("    apply_patch", "    if false { return; }\n    apply_patch"), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "patch selection changed"):
                    tls.applied_patches(root)


if __name__ == "__main__":
    unittest.main()
