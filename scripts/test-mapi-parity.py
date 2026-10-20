"""Compare synthetic MAPI cases with the independent pinned native reference."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile

from mapi_reference import ROOT, VENDOR, WORK, oracle_command

SPEC = importlib.util.spec_from_file_location("mapi_corpus", ROOT / "extensions/mapi/test/corpus.py")
corpus = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpus)


def normalize(node):
    if node is None or "failure" in node:
        return node
    result = {key: node[key] for key in ["type", "text", "parsed", "offset", "size", "valueType", "raw"]}
    if node["type"] == "MAPIInspector.Parsers.PtypTime":
        # Presentation is host-owned UTC FILETIME, not the reference machine's locale/time zone.
        result["text"] = None
    if node["type"] == "BlockParser.BlockException":
        result["children"] = []  # Native and JavaScript stack traces have different frames.
    else:
        result["children"] = [normalize(child) for child in node["children"]]
    return result


def nodes(node):
    if node:
        yield node
        for child in node.get("children", []):
            yield from nodes(child)


def shift_coordinates(node, delta):
    result=dict(node)
    result["offset"]=str(int(node["offset"])-delta)
    result["children"]=[shift_coordinates(child,delta) for child in node["children"]]
    return result


def decoded_coordinates(expected,actual,sources):
    result=dict(expected)
    source=sources.get(actual.get("sourceId"))
    if source:
        result["offset"]=str(int(expected["offset"])-int(source["parents"][0]["offset"]))
    result["children"]=[decoded_coordinates(left,right,sources) for left,right in zip(expected.get("children",[]),actual.get("children",[]))]
    if len(expected.get("children",[]))!=len(actual.get("children",[])):
        return expected  # Preserve a structural mismatch for ordinary comparison.
    return result


def run(command, fixtures):
    # Windows communicate writes stdin synchronously before honoring its timeout.
    # Use an inherited file so a stalled parser cannot block the test's input writer.
    with tempfile.TemporaryFile(mode="w+t",encoding="utf-8",newline="\n") as input_file:
        for fixture in fixtures:input_file.write(json.dumps(fixture)+"\n")
        input_file.seek(0)
        result = subprocess.run(command, stdin=input_file, cwd=ROOT,
                                text=True, encoding="utf-8", capture_output=True, check=True, timeout=120)
    output = [json.loads(line) for line in result.stdout.splitlines()]
    if len(output) != len(fixtures):
        raise AssertionError("Parity runner lost test results")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes=parser.add_mutually_exclusive_group()
    modes.add_argument("--write-reference", action="store_true")
    modes.add_argument("--golden-only",action="store_true",help="Use committed reference trees without requiring .NET")
    arguments = parser.parse_args()
    fixtures = corpus.cases()
    if len({item["name"] for item in fixtures})!=len(fixtures):
        raise AssertionError("Synthetic fixture names must be unique")
    dispatched=set(re.findall(r'case "(\w+)":',(VENDOR/"Source/MAPIParser.cs").read_text(encoding="utf-8-sig")))
    covered={(item["message"]["requestType"],item["message"]["direction"]) for item in fixtures if "type" not in item}
    if not {(operation,direction) for operation in dispatched for direction in ["request","response"]}<=covered:
        raise AssertionError("Synthetic corpus no longer covers every MAPI HTTP dispatcher in both directions")
    reference_path=ROOT / "extensions/mapi/test/reference-cases.json"
    if arguments.golden_only:
        saved=json.loads(reference_path.read_text(encoding="utf-8"))
        if [item["fixture"] for item in saved]!=fixtures:
            raise SystemExit("Synthetic reference cases are stale; regenerate with the pinned oracle")
        reference=[item["tree"] for item in saved]
    else:
        native_fixtures=[fixture for fixture in fixtures if not fixture.get("referenceUnsupported")]
        native_results=iter(run(oracle_command(),native_fixtures))
        reference=[None if fixture.get("referenceUnsupported") else next(native_results) for fixture in fixtures]
    javascript = run(["node", "--max-old-space-size=192", "--experimental-vm-modules", str(ROOT / "extensions/mapi/test/run-protocol.mjs")], fixtures)
    failures = []
    for fixture, expected, actual in zip(fixtures, reference, javascript):
        if fixture.get("referenceUnsupported"):
            if fixture["referenceUnsupported"]!="extended-upload-continuation-state":
                raise AssertionError("Unrecognized reference correction")
            # The pinned reference loops on its third extended-upload chunk:
            # it saves ordinary-upload fields instead of extended-upload fields.
            # This is an explicit semantic regression check, never counted as a
            # differential pass or substituted with an implementation gold master.
            values=list(nodes(actual))
            valid=actual and "failure" not in actual and not any(node["type"]=="BlockParser.BlockException" for node in values)
            strings=[node for node in values if node.get("stringValue")==fixture["semanticString"]]
            valid=valid and len(strings)==1 and strings[0]["sourceId"]=="body.30.request" and strings[0]["offset"]=="24" and strings[0]["size"]==str(len((fixture["semanticString"]+"\0").encode("utf-16-le")))
            if not valid:failures.append({"name":fixture["name"],"reason":"extended upload continuation regression","javascript":actual})
            continue
        if "timestamp" in fixture and actual.get("timestamp")!=fixture["timestamp"]:
            failures.append({"name":fixture["name"],"reason":"UTC FILETIME metadata mismatch","reference":expected,"javascript":actual})
            continue
        if "stringValue" in fixture and (actual.get("stringValue"),actual.get("charset"))!=(fixture["stringValue"],fixture["charset"]):
            failures.append({"name":fixture["name"],"reason":"string/encoding metadata mismatch","reference":expected,"javascript":actual})
            continue
        if "rpcDerivedExpectations" in fixture:
            outcome=actual
            actual=outcome.get("tree")
            derived=outcome.get("derived",[])
            specifications=fixture["rpcDerivedExpectations"]
            valid=len(derived)==len(specifications)
            for source,specification in zip(derived,specifications):
                valid=valid and source["bytes"]==specification["bytes"] and source["exactMapping"]==specification["exactMapping"]
                valid=valid and source["parents"]==[{"sourceId":"body.1."+fixture["message"]["direction"],"offset":specification["offset"],"length":specification["length"]}]
                valid=valid and all(0<=int(item["offset"])<=int(item["offset"])+int(item["size"])<=len(source["bytes"]) for item in nodes(actual) if item.get("sourceId")==source["id"])
            if not valid:
                failures.append({"name":fixture["name"],"reason":"RPC decoded provenance mismatch","reference":expected,"javascript":outcome})
                continue
            expected=decoded_coordinates(expected,actual,{source["id"]:source for source in derived})
        elif fixture.get("includeDerived"):
            outcome=actual
            actual=outcome.get("tree")
            specification=fixture["derivedExpectation"]
            derived=outcome.get("derived",[])
            if specification["transformed"]:
                valid=len(derived)==1 and derived[0]["bytes"]==specification["bytes"] and derived[0]["exactMapping"]==specification["exactMapping"]
                valid=valid and derived[0]["parents"]==[{"sourceId":"body.1.request","offset":"8","length":str(specification["encodedBytes"])}]
                valid=valid and all(item["sourceId"]==derived[0]["id"] and 0<=int(item["offset"])<=int(item["offset"])+int(item["size"])<=len(specification["bytes"])
                                    for item in nodes(actual["children"][1]))
                if not valid:
                    failures.append({"name":fixture["name"],"reason":"derived provenance mismatch","reference":expected,"javascript":outcome})
                    continue
                # The reference shifts decoded fields into encoded coordinates.
                # Correct this one documented presentation defect, preserving exact decoded ranges.
                expected=dict(expected)
                expected["children"]=list(expected["children"])
                expected["children"][1]=shift_coordinates(expected["children"][1],8)
            elif derived:
                failures.append({"name":fixture["name"],"reason":"unnecessary derived source","reference":expected,"javascript":outcome})
                continue
        if fixture.get("intentionalDifference") == "absent-string-layout":
            native_fault = any(item["type"] == "BlockParser.BlockException" and item["text"] == "Tree Layout Exception" for item in nodes(expected))
            repaired = actual.get("text") == "No value" and actual.get("size") == "1" and len(actual.get("children", [])) == 1
            if repaired:
                value = actual["children"][0]
                repaired = (value["valueType"], value["raw"], value["offset"], value["size"]) == ("System.Boolean", "False", "0", "1")
            if not native_fault or not repaired:
                failures.append({"name": fixture["name"], "reason": "absent-value layout regression", "reference": expected, "javascript": actual})
            continue
        if normalize(expected) != normalize(actual):
            failures.append({"name": fixture["name"], "reason": "differential mismatch", "reference": expected, "javascript": actual})
        elif fixture["expected"] == "complete" and ("failure" in expected or any(item["type"] == "BlockParser.BlockException" for item in nodes(expected))):
            failures.append({"name": fixture["name"], "reason": "valid fixture did not decode", "reference": expected})
    WORK.mkdir(exist_ok=True, parents=True)
    (WORK / "parity-failures.json").write_text(json.dumps(failures, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if failures:
        raise SystemExit(f'{len(failures)} of {len(fixtures)} cases failed; see {WORK / "parity-failures.json"}')
    if arguments.write_reference:
        reference_path.write_text(json.dumps([{"fixture": fixture, "tree": normalize(expected)} for fixture, expected in zip(fixtures, reference)], indent=2, ensure_ascii=False) + "\n", encoding="utf-8",newline="\n")
    corrected=sum(bool(fixture.get("referenceUnsupported")) for fixture in fixtures)
    print(f"MAPI differential parity: {len(fixtures)-corrected} valid synthetic cases passed; {corrected} explicit correction regression(s) passed")


if __name__ == "__main__":
    main()
