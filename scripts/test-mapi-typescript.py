"""Compare independent TypeScript wire facts with pinned synthetic reference data."""
import argparse
import base64
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
specification = importlib.util.spec_from_file_location("mapi_parity", ROOT / "scripts/test-mapi-parity.py")
parity = importlib.util.module_from_spec(specification)
specification.loader.exec_module(parity)


def native_facts(root, payload):
    result = set()
    for node in parity.nodes(root):
        offset, size = int(node["offset"]), int(node["size"])
        if size == 0:
            continue  # Synthetic headings and historical tag labels are not wire reads.
        value = node.get("raw")
        if value is not None:
            kind = "boolean" if node["valueType"] == "System.Boolean" else "guid" if node["valueType"] == "System.Guid" else "number"
            result.add((offset, size, kind, value.lower() if kind in ["boolean", "guid"] else value))
        elif node["type"] == "BlockParser.BlockBytes":
            result.add((offset, size, "bytes", ""))
        elif node["type"] in ["BlockParser.BlockStringA", "BlockParser.BlockStringW", "MAPIInspector.Parsers.PtypString", "MAPIInspector.Parsers.PtypString8"]:
            wide = node["type"] in ["BlockParser.BlockStringW", "MAPIInspector.Parsers.PtypString"]
            present = next((child for child in node.get("children", []) if child.get("valueType") == "System.Boolean"), None)
            if present:
                if present["raw"] == "False":
                    continue
                offset += 1
                size -= 1
            data = payload[offset:offset + size]
            text = data.decode("utf-16-le" if wide else "ascii", errors="replace")
            if not wide:
                text = text.replace("\ufffd", "?")
            text = text.removesuffix("\r\n").removesuffix("\0")
            result.add((offset, size, "string", text))
    return result


def typescript_facts(root):
    result = set()
    def walk(node):
        value = node.get("value")
        span = node["range"]
        if value is not None and span["length"]:
            kind = value["kind"]
            raw = value.get("value", "")
            if kind in ["integer", "enum", "float"]:
                kind = "number"
            if isinstance(raw, bool):
                raw = str(raw).lower()
            result.add((span["offset"], span["length"], kind, str(raw)))
        for child in node["children"]:
            walk(child)
    walk(root)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", choices=["properties-http", "all"], default="all")
    arguments = parser.parse_args()
    saved = json.loads((ROOT / "extensions/mapi/test/reference-cases.json").read_text(encoding="utf-8"))
    if arguments.stage == "properties-http":
        saved = [item for item in saved if
                 (item["fixture"]["message"]["requestType"] != "Execute" or
                  item["fixture"].get("type", "").rsplit(".", 1)[-1].startswith(("Ptyp", "PropertyTag", "RestrictionType")))
                 and "auxiliary" not in item["fixture"]["name"]]
    outcomes = parity.run(["node", "--experimental-vm-modules", str(ROOT / "extensions/mapi/test/run-typescript.mjs")], [item["fixture"] for item in saved])
    failures = []
    for item, actual in zip(saved, outcomes):
        fixture = item["fixture"]
        payload = base64.b64decode(fixture["message"].get(fixture["message"]["direction"], ""))
        expected = native_facts(item["tree"], payload)
        # These are independently asserted wire corrections, not reference-tree
        # substitutions. The inspector reads but hides Connect's auxiliary size,
        # and skips it on failed ResortRestriction responses. All these fixtures
        # have the explicit four-byte empty auxiliary trailer.
        if fixture["message"]["direction"] == "response" and (
            fixture["message"]["requestType"] == "Connect" or
            fixture["name"] == "nspi-ResortRestriction-failure"
        ):
            assert payload[-4:] == bytes(4)
            expected.add((len(payload) - 4, 4, "number", "0"))
        if fixture.get("intentionalDifference") == "absent-string-layout":
            assert payload == bytes(1)
            expected = {(0, 1, "boolean", "false")}
        root = actual.get("root")
        observed = typescript_facts(root) if root else set()
        issues = [node["issue"] for node in walk_fields(root) if "issue" in node] if root else [actual]
        if expected != observed or issues:
            failures.append({"name": fixture["name"], "missing": sorted(expected - observed), "extra": sorted(observed - expected), "issues": issues})
    output = ROOT / ".tools/mapi-build/typescript-failures.json"
    output.write_text(json.dumps(failures, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if failures:
        raise SystemExit(f"TypeScript parity: {len(failures)} of {len(saved)} cases failed; see {output}")
    print(f"Independent TypeScript MAPI: {len(saved)} synthetic wire-fact comparisons passed")


def walk_fields(root):
    if root:
        yield root
        for child in root["children"]:
            yield from walk_fields(child)


if __name__ == "__main__":
    main()
