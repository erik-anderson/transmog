"""Guard source-driven SDK schema evolution, including failure on unsupported shapes."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from sdk_models import Generator, Models, ROOTS


def field(name, type_name):
    return {"name": name, "public": True, "docs": [], "type": {"path": [type_name], "types": []}}


def schema():
    return {"modules": {"transmog_app": {"public": True, "imports": []}}, "types": {
        "transmog_app::" + name: {"name": name, "module": "transmog_app", "public": True,
                                 "kind": "struct", "fields": [], "docs": []}
        for name in ROOTS
    }}


class ModelEvolutionTests(unittest.TestCase):
    def test_added_rust_field_flows_into_every_projection(self):
        source = schema()
        before = Generator(Models(deepcopy(source)))
        source["types"]["transmog_app::ProxyStartRequest"]["fields"] += [field("new_count", "u64")]
        after = Generator(Models(source))
        self.assertNotIn("new_count", before.header())
        self.assertIn("uint64_t new_count", after.header())
        self.assertIn("self.new_count.project(arena)?", after.rust_code())
        self.assertIn("new_count: unsafe { FromAbi::read(&value.new_count, budget) }?", after.rust_code())
        self.assertIn("ulong NewCount", after.csharp())
        self.assertIn("NewCount = value.NewCount", after.csharp())

    def test_cpp_keywords_are_escaped_without_renaming_rust_fields(self):
        source = schema()
        source["types"]["transmog_app::ProductState"]["fields"] = [field("default", "bool")]
        generated = Generator(Models(source))
        self.assertIn("uint32_t default_", generated.header())
        self.assertIn("self.default.project", generated.rust_code())
        self.assertIn("bool Default", generated.csharp())

    def test_conditional_and_private_fields_fail_closed(self):
        for change in ({"public": False}, {"conditional": True}):
            source = schema()
            source["types"]["transmog_app::ProductState"]["fields"] = [dict(field("value", "u64"), **change)]
            with self.assertRaisesRegex(ValueError, "Conditional/private SDK field"):
                Models(source)

    def test_unknown_rust_type_cannot_silently_drop_a_field(self):
        source = schema()
        source["types"]["transmog_app::ProductState"]["fields"] = [field("future", "Unsupported")]
        with self.assertRaisesRegex(ValueError, "Cannot resolve Unsupported"):
            Models(source)


if __name__ == "__main__":
    unittest.main()
