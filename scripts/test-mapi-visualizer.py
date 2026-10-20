"""Exercise production MAPI contributions with bounded retained-input continuations."""
import base64
import copy
import importlib.util
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
specification=importlib.util.spec_from_file_location("mapi_parity",ROOT/"scripts/test-mapi-parity.py")
parity=importlib.util.module_from_spec(specification);specification.loader.exec_module(parity)


def nodes(document):
    def walk(node):
        yield node
        for child in node.get("children",[]):yield from walk(child)
    for node in document["roots"]:yield from walk(node)


def run(fixtures,entry="body"):
    return parity.run(["node","--max-old-space-size=192","--experimental-vm-modules",str(ROOT/"extensions/mapi/test/run-visualizer.mjs"),entry],fixtures)


def main():
    corpus=parity.corpus.cases()
    fixtures=[fixture for fixture in corpus if "type" not in fixture]
    outcomes=run(fixtures)
    failures=[]
    for fixture,outcome in zip(fixtures,outcomes):
        document=outcome.get("document")
        expected="partial" if fixture["name"]=="dispatch-RopBufferTooSmallResponse" else "complete"
        if not document or document["status"]!=expected:
            failures.append({"name":fixture["name"],"outcome":outcome});continue
        identities=set()
        for node in nodes(document):
            if node["id"] in identities:raise AssertionError("Duplicate output identity")
            identities.add(node["id"])
            for span in node.get("spans",[]):
                if int(span["offset"])<0 or int(span["length"])<0:raise AssertionError("Invalid evidence span")
    parity.WORK.mkdir(parents=True,exist_ok=True)
    (parity.WORK/"visualizer-failures.json").write_text(json.dumps(failures,indent=2,ensure_ascii=False),encoding="utf-8")
    if failures:raise SystemExit(f"{len(failures)} production visualizer cases failed; see .tools/mapi-build/visualizer-failures.json")
    connect=next(fixture for fixture in corpus if fixture["name"]=="connect-request-nonzero")
    headers=run([connect],"headers")[0]
    if headers["reads"] or headers["parserLoaded"]:raise AssertionError("Header contribution loaded the body decoder")
    if not any(node.get("value")=={"kind":"string","value":"Connect"} for node in nodes(headers["document"])):raise AssertionError("Missing request operation")
    encoded=copy.deepcopy(connect);encoded["message"]["requestHeaders"]={"content-encoding":"gzip"};encoded["decodedBytes"]=encoded["message"]["request"]
    encoded["message"]["request"]=base64.b64encode(b"synthetic HTTP encoded bytes").decode()
    decoded,failed=run([encoded,dict(encoded,decodedFailure="unsupported")])
    if decoded.get("document",{}).get("status")!="complete" or decoded["decodedReads"]!=1:raise AssertionError("HTTP decoded source was not used")
    if failed.get("document",{}).get("status")!="unavailable" or failed["reads"]:raise AssertionError("Encoded bytes substituted for failed decoding")
    oversized=copy.deepcopy(connect);oversized["message"]["requestLength"]=8*1024*1024+1
    limited=run([oversized])[0]
    if limited.get("document",{}).get("status")!="limited" or limited["reads"]:raise AssertionError("Oversized selected body was read")
    incomplete=copy.deepcopy(connect);incomplete["message"]["request"]="";incomplete["message"]["requestAvailability"]="capturing"
    incomplete["updates"]=[dict(connect["message"],revision=1)]
    live=run([incomplete])[0]
    if live.get("document",{}).get("status")!="complete":raise AssertionError("Selected live body did not update")
    unchanged=copy.deepcopy(connect);unchanged["missedHints"]=True
    missed=run([unchanged])[0]
    if not any(item["code"]=="missed-live-data" for item in missed["document"].get("diagnostics",[])):raise AssertionError("Missed live hints were not shown")
    print(f"MAPI production visualizers: {len(fixtures)} corpus cases, header-only loading, HTTP decoding/failure and selected live updates passed")


if __name__=="__main__":main()
