"""Freeze minimal parser-shape fixtures from the pinned native ROP dispatchers."""
import base64
import json
from pathlib import Path
import re
import struct
import subprocess
import tempfile

from mapi_reference import ROOT, WORK, ensure_sources, oracle_command


def u16(value): return struct.pack("<H",value)
def u32(value): return struct.pack("<I",value)
def b64(data): return base64.b64encode(data).decode("ascii")


def rpc(rops, handle=0xffffffff):
    payload=u16(2+len(rops))+rops+u32(handle)
    return struct.pack("<HHHH",0,4,len(payload),len(payload))+payload


def request(rops, handle=0xffffffff):
    buffer=rpc(rops,handle)
    return u32(0)+u32(len(buffer))+buffer+u32(65536)+u32(0)


def response(rops):
    buffer=rpc(rops,0x12345678)
    return b"DONE\r\n\r\n"+u32(0)*3+u32(len(buffer))+buffer+u32(0)


DN=b"/o=Example/ou=Synthetic/cn=User\0"
LOGON=bytes([0xfe,7,0,1])+u32(0)+u32(0)+u16(len(DN))+DN


def fixture(item, payload, paired):
    needs_history=item["operation"] in ["RopWritePerUserInformation","RopSetMessageReadFlag"]
    incoming=request((b"" if needs_history else LOGON)+(payload if item["direction"]=="request" else paired))
    outgoing=response(payload) if item["direction"]=="response" else b""
    return {"type":"MAPIInspector.Parsers."+item["parser"],"arguments":item["arguments"],"prepareRequest":True,
            "context":[{"requestType":"Execute","request":b64(request(LOGON,7))}] if needs_history else [],
            "offset":18+(0 if needs_history else len(LOGON)) if item["direction"]=="request" else 34,
            "count":len(payload),
            "message":{"direction":item["direction"],"requestType":"Execute","request":b64(incoming),"response":b64(outgoing)}}


def main():
    files=ensure_sources()/"Parsers/MSOXCROPS"
    enum=(files/"enums/RopIdType.cs").read_text(encoding="utf-8-sig")
    ids={name:int(value,0) for name,value in re.findall(r"\b(Rop\w+)\s*=\s*(0x[0-9a-fA-F]+|\d+)",enum)}
    inventory={item["type"].split(".")[-1]:item for item in json.loads((ROOT/"extensions/mapi/test/reference-inventory.json").read_text())["blocks"]}
    items=[]
    for direction,buffer,suffix in [("request","ROPInputBuffer","Request"),("response","ROPOutputBuffer","Response")]:
        source=(files/"ropBuffers"/(buffer+".cs")).read_text(encoding="utf-8-sig")
        for operation in dict.fromkeys(re.findall(r"case RopIdType\.(\w+)\s*:",source)):
            names=[operation+suffix] if operation+suffix in inventory else [operation+suffix+"_PrivateMailboxes",operation+suffix+"_PublicFolders"]
            for name in names:
                constructors=inventory[name]["constructors"]
                arguments=[]
                if [] not in constructors:
                    arguments=[None if parameter.endswith("[]") else 0 for parameter in constructors[0]]
                    if operation=="RopBufferTooSmall":arguments=[3]
                items.append({"direction":direction,"operation":operation,"id":ids[operation],"parser":name,"arguments":arguments})
    seeds=[]
    for item in items:
        if item["direction"]=="request":
            payload=bytes([item["id"],7,0])+bytes(1024)
            if item["operation"] in ["RopFastTransferDestinationPutBuffer","RopFastTransferDestinationPutBufferExtended","RopSynchronizationImportReadStateChanges"]:
                payload=bytes([item["id"],7,0])+u16(0)
            if item["operation"] in ["RopSetProperties","RopSetPropertiesNoReplicate"]:
                payload=bytes([item["id"],7,0])+u16(2)+u16(0)
        elif item["operation"]=="RopPending":payload=bytes([item["id"]])+u16(1234)
        elif item["operation"]=="RopNotify":payload=bytes([item["id"]])+u32(0x12345678)+bytes([7])+u16(0)+bytes(1024)
        elif item["operation"]=="RopBackoff":payload=bytes([item["id"],7])+u32(1000)+bytes([0])+u16(0)
        elif item["operation"]=="RopBufferTooSmall":payload=bytes([item["id"]])+u16(512)+bytes([1,7,0])
        else:payload=bytes([item["id"],0])+u32(0x8004010f)+bytes(1024)
        seeds.append(payload)
    command=oracle_command()
    inputs=[fixture(item,payload,bytes([1,7,0])) for item,payload in zip(items,seeds)]
    trees=[]
    for item,case in zip(items,inputs):
        with tempfile.TemporaryFile(mode="w+t",encoding="utf-8",newline="\n") as input_file:
            input_file.write(json.dumps(case)+"\n");input_file.seek(0)
            try:
                result=subprocess.run(command,stdin=input_file,text=True,encoding="utf-8",capture_output=True,check=True,timeout=3)
                trees.append(json.loads(result.stdout))
            except (subprocess.TimeoutExpired,subprocess.CalledProcessError) as error:
                trees.append({"failure":type(error).__name__})
        print(item["parser"]+": "+trees[-1].get("failure","decoded"),flush=True)
    failures=[]
    for item,payload,tree in zip(items,seeds,trees):
        def faulty(node):
            return node.get("type")=="BlockParser.BlockException" or any(faulty(child) for child in node.get("children",[]))
        if "failure" in tree or faulty(tree):
            failures.append({"item":item,"tree":tree});continue
        consumed=int(tree["size"])
        if consumed<1 or consumed>len(payload):
            failures.append({"item":item,"tree":tree});continue
        item["bytes"]=b64(payload[:consumed])
    WORK.mkdir(parents=True,exist_ok=True)
    (WORK/"rop-seed-failures.json").write_text(json.dumps(failures,indent=2)+"\n",encoding="utf-8")
    if failures:
        raise SystemExit(f"{len(failures)} native ROP seeds need explicit fixtures; see .tools/mapi-build/rop-seed-failures.json")
    output={"revision":"7a76470574377d14cae612c507f45b34d70e05a0","kind":"structural-dispatch","logon":b64(LOGON),"items":items}
    (ROOT/"extensions/mapi/test/rop-shapes.json").write_text(json.dumps(output,indent=2)+"\n",encoding="utf-8",newline="\n")
    print(f"Recorded {len(items)} ROP parser shapes; scenarios and full dispatcher checks are separate")


if __name__=="__main__":main()
