"""Compare independent TypeScript wire facts with pinned synthetic reference data."""
import argparse
import base64
import importlib.util
import json
import subprocess
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


def typescript_facts(root, derived=(), skip_transfer=False):
    result = set()
    def walk(node):
        value = node.get("value")
        span = node["range"]
        if value is not None and span["length"] and not (skip_transfer and span["sourceId"].startswith("mapi-transfer.")):
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


def continuation_evidence(fixture, actual):
    """Independently reconstruct the fixture's one split element from wire bytes."""
    direction = fixture["message"]["direction"]
    start = 49 if direction == "response" else 23
    fragments = []
    parents = []
    for message in sorted([*fixture.get("context", []), fixture["message"]], key=lambda message: message["order"]):
        body = base64.b64decode(message[direction])
        size = int.from_bytes(body[start - 2:start], "little")
        assert start + size <= len(body)
        fragments.append(body[start:start + size])
        parents.append({"sourceId": f"body.{message['order']}.{direction}", "offset": start, "length": size})
    combined = b"".join(fragments)
    sources = [source for source in actual["derivedSources"] if source["id"].startswith("mapi-transfer.")]
    assert len(sources) == 1
    source = sources[0]
    assert source["bytes"] == list(combined)
    assert source["parents"] == parents and source["exactMapping"] is False
    values = list(walk_fields(actual["root"]))
    if "unicode" in fixture["name"] or "out-of-order" in fixture["name"]:
        expected = combined[8:].decode("utf-16-le").removesuffix("\0")
        strings = [node for node in values if node.get("value") == {"kind": "string", "value": expected}]
        assert len(strings) == 1
        assert strings[0]["range"] == {"sourceId": source["id"], "offset": 8, "length": len(combined) - 8}
    elif "binary" in fixture["name"]:
        assert any(node.get("value") == {"kind": "bytes"} and node["range"] == {"sourceId": source["id"], "offset": 8, "length": 16} for node in values)
    else:
        assert any(node.get("value") == {"kind": "integer", "value": "123456"} and node["range"] == {"sourceId": source["id"], "offset": 4, "length": 4} for node in values)
    return start, parents[-1]["length"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", choices=["properties-http", "rops", "all"], default="all")
    parser.add_argument("--reference", type=Path, default=ROOT / "extensions/mapi/test/reference-cases.json")
    arguments = parser.parse_args()
    subprocess.run(["node", str(ROOT / "extensions/mapi/build.mjs")], cwd=ROOT, check=True)
    saved = json.loads(arguments.reference.read_text(encoding="utf-8"))
    if [item["fixture"] for item in saved] != parity.corpus.cases():
        raise SystemExit("Synthetic reference cases are stale")
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
        compare_facts = True
        payload = base64.b64decode(fixture["message"].get(fixture["message"]["direction"], ""))
        if fixture["name"] in ["shape-RopOptionsDataResponse", "dispatch-RopOptionsDataResponse"]:
            assert actual.get("root", {}).get("issue"), "The invalid reference probe must not be a complete parse"
            continue
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
            if node["type"].endswith("MetaPropValueGetPartial"):
                offset = int(node["offset"])
                if payload[offset:offset + 4] == bytes.fromhex("03000f40"):
                    raw = payload[offset + 4:offset + 8]
                    expected.discard((offset + 4, 4, "number", str(int.from_bytes(raw, "little", signed=True))))
                    expected.add((offset + 4, 4, "number", str(int.from_bytes(raw, "little"))))
        root = actual.get("root")
        derived = actual.get("derivedSources", [])
        partial = fixture["family"] == "fast-transfer-partial"
        observed = typescript_facts(root, derived, skip_transfer=partial) if root else set()
        issues = [node["issue"] for node in walk_fields(root) if "issue" in node] if root else [actual]
        if partial and root and not issues:
            start, length = continuation_evidence(fixture, actual)
            expected = {fact for fact in expected if not start <= fact[0] < start + length}
            # The extended-upload reference loops here, so it supplies no tree.
            # The complete value and exact concatenated evidence above are an
            # independent semantic check, never a substituted reference golden.
            if fixture.get("referenceUnsupported"):
                compare_facts = False
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
        for index, message in enumerate(fixture.get("context", []), 1):
            for side in ["request", "response"]:
                lengths[f"body.{message.get('order', index)}.{side}"] = len(base64.b64decode(message.get(side, "")))
        for source in derived:
            for parent in source["parents"]:
                assert 0 <= parent["offset"] <= parent["offset"] + parent["length"] <= lengths[parent["sourceId"]]
        for node in walk_fields(root):
            span = node["range"]
            if span["sourceId"] not in lengths or not 0 <= span["offset"] <= span["offset"] + span["length"] <= lengths[span["sourceId"]]:
                issues.append({"kind": "provenance", "message": "Field range escaped its source"})
        if (compare_facts and expected != observed) or issues:
            failures.append({"name": fixture["name"], "missing": sorted(expected - observed), "extra": sorted(observed - expected), "issues": issues})
    output = ROOT / ".tools/mapi-build/typescript-failures.json"
    output.write_text(json.dumps(failures, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if failures:
        raise SystemExit(f"TypeScript parity: {len(failures)} of {len(saved)} cases failed; see {output}")
    malformed = sum(item["fixture"]["name"] in ["shape-RopOptionsDataResponse", "dispatch-RopOptionsDataResponse"] for item in saved)
    continuations = sum(item["fixture"]["family"] == "fast-transfer-partial" for item in saved)
    print(f"Independent TypeScript MAPI: {len(saved) - malformed - continuations} wire-fact comparisons, "
          f"{continuations} continuation evidence checks, {malformed} invalid reference probes passed")


def walk_fields(root):
    if root:
        yield root
        for child in root["children"]:
            yield from walk_fields(child)


if __name__ == "__main__":
    main()
