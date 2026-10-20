"""Compare synthetic MAPI cases with the independent pinned native reference."""
import argparse
import sys
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile

from mapi_reference import ROOT, WORK, oracle_command

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
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--write-reference", action="store_true")
    modes.add_argument("--golden-only", action="store_true")
    arguments = parser.parse_args()
    fixtures = corpus.cases()
    target = ROOT / "extensions/mapi/test/reference-cases.json"
    saved = json.loads(target.read_text(encoding="utf-8"))
    if [item["fixture"] for item in saved] != fixtures:
        if arguments.golden_only:
            raise SystemExit("Synthetic reference cases are stale")
    if arguments.golden_only:
        candidate = target
    else:
        results = iter(run(oracle_command(), [fixture for fixture in fixtures if not fixture.get("referenceUnsupported")]))
        fresh = [{"fixture": fixture, "tree": None if fixture.get("referenceUnsupported") else normalize(next(results))} for fixture in fixtures]
        candidate = WORK / "candidate-reference.json"
        candidate.write_text(json.dumps(fresh, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        if not arguments.write_reference and fresh != saved:
            raise SystemExit("Native reference results changed; review .tools/mapi-build/candidate-reference.json")
    subprocess.run([sys.executable, str(ROOT / "scripts/test-mapi-typescript.py"), "--reference", str(candidate)], cwd=ROOT, check=True)
    if arguments.write_reference:
        target.write_text(candidate.read_text(encoding="utf-8"), encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
