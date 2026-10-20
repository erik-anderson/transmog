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


def typescript_facts(root, derived=()):
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
            offset = span["offset"]
            if span["sourceId"].startswith("mapi-derived."):
                source = next(source for source in derived if source["id"] == span["sourceId"])
                offset += source["parents"][0]["offset"]
            result.add((offset, span["length"], kind, str(raw)))
        for child in node["children"]:
            walk(child)
    walk(root)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", choices=["properties-http", "rops", "all"], default="all")
    arguments = parser.parse_args()
    saved = json.loads((ROOT / "extensions/mapi/test/reference-cases.json").read_text(encoding="utf-8"))
    if arguments.stage == "properties-http":
        saved = [item for item in saved if
                 (item["fixture"]["message"]["requestType"] != "Execute" or
                  item["fixture"].get("type", "").rsplit(".", 1)[-1].startswith(("Ptyp", "PropertyTag", "RestrictionType")))
                 and "auxiliary" not in item["fixture"]["name"]]
    if arguments.stage == "rops":
        saved = [item for item in saved if not item["fixture"]["name"].startswith("ft-") and
                 item["fixture"]["name"] not in ["shape-RopOptionsDataResponse", "dispatch-RopOptionsDataResponse"]]
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
        # The pinned inspector's UI omits these parsed fields. Check their
        # protocol-defined positions directly against the independent bytes.
        hidden_logons = {"RopSetReceiveFolder", "RopPublicFolderIsGhosted", "RopReadPerUserInformation",
                         "RopWritePerUserInformation", "RopGetRulesTable", "RopUpdateDeferredActionMessages",
                         "RopSynchronizationGetTransferState", "RopSynchronizationUploadStateStreamContinue",
                         "RopMoveCopyMessages"}
        hidden_fields = {name: [(1, 1, False)] for name in hidden_logons}
        hidden_fields.update({"RopOpenMessage": [(4, 2, True)], "RopOptionsData": [(2, 1, False)],
                              "RopSaveChangesMessage": [(2, 1, False)], "RopSaveChangesAttachment": [(2, 1, False)],
                              "RopSetMessageReadFlag": [(2, 1, False)], "RopGetAttachmentTable": [(3, 1, False)]})
        for node in parity.nodes(item["tree"]):
            operation = node["type"].rsplit(".", 1)[-1].removesuffix("Request")
            if node["type"].endswith("Request") and operation in hidden_fields:
                for relative, size, signed in hidden_fields[operation]:
                    offset = int(node["offset"]) + relative
                    expected.add((offset, size, "number", str(int.from_bytes(payload[offset:offset + size], "little", signed=signed))))
        root = actual.get("root")
        derived = actual.get("derivedSources", [])
        observed = typescript_facts(root, derived) if root else set()
        issues = [node["issue"] for node in walk_fields(root) if "issue" in node] if root else [actual]
        specifications = fixture.get("rpcDerivedExpectations")
        if fixture.get("derivedExpectation", {}).get("transformed"):
            specification = fixture["derivedExpectation"]
            specifications = [{"bytes": specification["bytes"], "offset": "8", "length": str(specification["encodedBytes"]), "exactMapping": specification["exactMapping"]}]
        if specifications is not None:
            if len(derived) != len(specifications):
                issues.append({"kind": "provenance", "message": "Derived source count differs"})
            for source, specification in zip(derived, specifications):
                expected_parent = {"sourceId": f"body.{fixture['message'].get('order', len(fixture.get('context', [])) + 1)}.{fixture['message']['direction']}",
                                   "offset": int(specification["offset"]), "length": int(specification["length"])}
                if source["bytes"] != specification["bytes"] or source["exactMapping"] != specification["exactMapping"] or source["parents"] != [expected_parent]:
                    issues.append({"kind": "provenance", "message": "Decoded bytes or enclosing evidence differs"})
        lengths = {f"body.{fixture['message'].get('order', len(fixture.get('context', [])) + 1)}.{fixture['message']['direction']}": len(payload)}
        lengths.update({source["id"]: len(source["bytes"]) for source in derived})
        for node in walk_fields(root):
            span = node["range"]
            if span["sourceId"] not in lengths or not 0 <= span["offset"] <= span["offset"] + span["length"] <= lengths[span["sourceId"]]:
                issues.append({"kind": "provenance", "message": "Field range escaped its source"})
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
