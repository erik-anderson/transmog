"""Prepare exact runtime inventory/metadata; never includes undeclared authoring files."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
PACKAGE=ROOT/"extensions/mapi"
RUNTIME=["generated/parser.js","src/protocol.js","src/body.js","src/view.js","src/shared.js","src/headers.js","README.runtime.md"]
NOTICES={
    "Office-MIT.txt":"third_party/mapi-inspector/LICENSE",
    "Transpose-APACHE-2.0.txt":"third_party/transpose/LICENSE",
    "decimal-MIT.txt":"third_party/transpose/licenses/decimal-LICENSE",
    "Long-APACHE-2.0.txt":"third_party/transpose/licenses/long-LICENSE",
    "LINQ-MIT.txt":"third_party/transpose/licenses/linq-LICENSE",
}
OPERATIONS=["Connect","Execute","Disconnect","NotificationWait","Bind","Unbind","CompareMIds","DNToMId","GetMatches","GetPropList","GetProps","GetSpecialTable","GetTemplateInfo","ModLinkAtt","ModProps","QueryRows","QueryColumns","ResolveNames","ResortRestriction","SeekEntries","UpdateStat","GetMailboxUrl","GetAddressBookUrl"]


def access(headers,body="none",decode=False):
    value={"headers":headers,"body":body}
    if decode:value["decodeHttpContent"]=True
    return value


def rule(direction,operation=None):
    return {"direction":direction,"contentTypes":["application/mapi-http"],"headers":[{"direction":"request","name":"x-requesttype","test":{"op":"equals","value":operation} if operation else {"op":"present"}}]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check",action="store_true")
    options=parser.parse_args()
    files={name:(PACKAGE/name).read_bytes() for name in RUNTIME}
    for name,source in NOTICES.items():files["LICENSES/"+name]=(ROOT/source).read_bytes()
    headers={"request":["content-type","x-requesttype","x-clientinfo","content-encoding"],"response":["content-type","x-responsecode","content-encoding"]}
    visualizers=[]
    for direction in ["request","response"]:
        visualizers.append({"id":"body-"+direction,"name":"MAPI/HTTP","entry":"src/body.js","surfaces":["body"],
            "inputs":{side:access(headers[side],"required" if side==direction else "optional",True) for side in headers},
            "matches":[rule(direction,operation) for operation in OPERATIONS],"inputMatches":[rule(side) for side in headers],
            "context":{"headers":[{"direction":"request","name":"x-clientinfo","test":{"op":"present"}}],"maxExchanges":64},
            "liveUpdates":{"events":["request","response"],"bodyProgress":False},
            "limits":{"inputBytes":16*1024*1024,"outputBytes":8*1024*1024,"heapBytes":256*1024*1024,"durationMs":2000,"nodes":20000}})
    visualizers.append({"id":"headers","name":"MAPI/HTTP headers","entry":"src/headers.js","surfaces":["headers"],
        "inputs":{side:access([name for name in headers[side] if name!="content-encoding"]+(["x-requesttype"] if side=="response" else [])) for side in headers},
        "matches":[rule(side) for side in headers]})
    manifest={"formatVersion":1,"apiVersion":1,"runtime":"javascript-es2022","id":"com.transmog.mapi","version":"0.1.0","name":"MAPI/HTTP",
        "visualizers":visualizers,"trafficColumns":[{"id":"protocol","name":"Protocol","inputs":{"request":access(["content-type","x-requesttype"]),"response":access([])},
            "values":[{"value":"MAPI/HTTP","matches":[rule("request")]}]}],
        "files":[{"path":name,"bytes":len(data),"sha256":hashlib.sha256(data).hexdigest()} for name,data in sorted(files.items())]}
    encoded=(json.dumps(manifest,indent=2)+"\n").encode("utf-8")
    for name,data in [(name,data) for name,data in files.items() if name.startswith("LICENSES/")]+[("manifest.json",encoded)]:
        path=PACKAGE/name
        if options.check:
            if not path.is_file() or (path.read_text(encoding="utf-8").replace("\r\n","\n").encode("utf-8") if name=="manifest.json" else path.read_bytes())!=data:
                raise SystemExit("MAPI runtime metadata/notices are stale: "+name)
        else:
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(data)
    print(f"MAPI runtime inventory: {len(files)} exact payload files, {sum(map(len,files.values())):,} bytes")


if __name__=="__main__":main()
