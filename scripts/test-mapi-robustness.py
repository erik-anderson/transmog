"""Bounded malformed-input and recovery checks; hostile inputs never run in the native oracle."""
import base64
import importlib.util
import json
from pathlib import Path
import struct

ROOT=Path(__file__).resolve().parents[1]
specification=importlib.util.spec_from_file_location("mapi_parity",ROOT/"scripts/test-mapi-parity.py")
parity=importlib.util.module_from_spec(specification)
specification.loader.exec_module(parity)


def u16(value): return struct.pack("<H",value)
def u32(value): return struct.pack("<I",value)


def main():
    corpus=parity.corpus
    valid=next(item for item in corpus.cases() if item["name"]=="connect-request-nonzero")
    saved=json.loads((ROOT/"extensions/mapi/test/reference-cases.json").read_text(encoding="utf-8"))
    expected=next(item["tree"] for item in saved if item["fixture"]["name"]==valid["name"])
    payload=base64.b64decode(valid["message"]["request"])
    probes=[corpus.case("truncated-connect-"+str(count),"malformed","Connect",payload[:count]) for count in range(len(payload))]
    execute=corpus.execute_request(bytes([1,7,0]))
    probes += [corpus.case("truncated-execute-"+str(count),"malformed","Execute",execute[:count]) for count in range(len(execute))]
    for name,data in [
        ("oversized-envelope",u32(0)+u32(0xffffffff)+bytes(16)),
        ("oversized-rpc",u32(0)+u32(8)+struct.pack("<HHHH",0,4,65535,65535)+u32(0)*2),
        ("empty-rpc-no-progress",u32(0)+u32(16)+bytes(8)+u32(0)*2),
        ("negative-rop-size",u32(0)+u32(10)+struct.pack("<HHHH",0,4,2,2)+u16(1)+u32(0)*2),
        ("unknown-rop",corpus.execute_request(bytes([0xff,7,0]))),
        ("unknown-upload-element",corpus.execute_request(bytes([0x54,7,0])+u16(4)+u32(0xffffffff))),
    ]:
        probes.append(corpus.case(name,"malformed","Execute",data))
    empty=corpus.case("empty-upload-followed-by-release","repaired-boundary","Execute",corpus.execute_request(bytes([0x54,7,0])+u16(0)+bytes([1,7,0])))
    probes.append(empty)
    too_many=corpus.case("context-limit","limit","Connect",payload)
    too_many["context"]=[{"requestType":"Connect"} for _ in range(257)]
    probes.append(too_many)
    fixtures=[]
    for probe in probes:fixtures.extend([probe,valid])
    results=parity.run(["node","--max-old-space-size=192","--experimental-vm-modules",str(ROOT/"extensions/mapi/test/run-protocol.mjs")],fixtures)
    for index,probe in enumerate(probes):
        actual,recovered=results[index*2:index*2+2]
        if parity.normalize(recovered)!=expected:
            raise AssertionError("Parser state leaked after "+probe["name"])
        if probe["family"]=="limit":
            if actual.get("failure")!="Error" or actual.get("message")!="Context limit":raise AssertionError("Context cap was not enforced")
            continue
        if actual and "failure" in actual:continue
        length=len(base64.b64decode(probe["message"]["request"]))
        for node in parity.nodes(actual):
            if node.get("sourceId") and not 0<=int(node["offset"])<=int(node["offset"])+int(node["size"])<=length:
                raise AssertionError("Out-of-range evidence span after "+probe["name"]+": "+str(node)[:300])
        if probe is empty:
            types={node["type"] for node in parity.nodes(actual)}
            if "MAPIInspector.Parsers.RopReleaseRequest" not in types or "BlockParser.BlockException" in types:
                raise AssertionError("An empty upload consumed the following ROP")
    print(f"MAPI robustness: {len(probes)} malformed/boundary probes and {len(probes)} same-worker recovery checks passed")


if __name__=="__main__":main()
