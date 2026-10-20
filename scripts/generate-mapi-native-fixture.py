"""Write synthetic binary SAZ fixtures for the real desktop MAPI acceptance flow."""
import argparse
import base64
import copy
import gzip
import importlib.util
import json
from pathlib import Path
import struct
import zipfile

ROOT=Path(__file__).resolve().parents[1]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output",type=Path)
    output=parser.parse_args().output.resolve()
    specification=importlib.util.spec_from_file_location("mapi_corpus",ROOT/"extensions/mapi/test/corpus.py")
    corpus=importlib.util.module_from_spec(specification);specification.loader.exec_module(corpus)
    cases={item["name"]:item for item in corpus.cases()}
    table=cases["success-table-retained-columns"]
    messages=[
        ("connect",cases["connect-response-nonzero"]["message"],"mapi.invalid",False),
        ("properties",cases["success-RopGetPropertiesAll-response"]["message"],"mapi.invalid",False),
        ("columns",table["context"][0],"mapi.invalid",False),
        ("foreign-columns",copy.deepcopy(table["context"][0]),"other.invalid",False),
        ("rows",table["message"],"mapi.invalid",False),
        ("transformed",cases["rpc-multiple-response-3"]["message"],"mapi.invalid",False),
        ("http-gzip",cases["connect-response-nonzero"]["message"],"mapi.invalid",True),
    ]
    # A later foreign-origin table uses incompatible columns. If native scope
    # checks leak it, the selected rows no longer contain the expected values.
    wrong=bytes([0x12,7,0,0])+struct.pack("<H",1)+struct.pack("<I",0x3001000b)
    messages[3][1]["request"]=base64.b64encode(corpus.execute_request(wrong,0x12345678)).decode()
    specifications=[]
    def write(path,items):
        with zipfile.ZipFile(path,"w",compression=zipfile.ZIP_DEFLATED) as archive:
            for number,(name,message,host,compressed) in enumerate(items,1):
                request=base64.b64decode(message.get("request",""));response=base64.b64decode(message.get("response",""))
                if compressed:response=gzip.compress(response,mtime=0)
                request_head=(f"POST https://{host}/mapi/emsmdb?MailboxId=synthetic HTTP/1.1\r\nContent-Type: application/mapi-http\r\nX-RequestType: {message['requestType']}\r\nX-ClientInfo: 00112233-4455-6677-8899-aabbccddeeff\r\nContent-Length: {len(request)}\r\n\r\n").encode("ascii")
                response_head=(f"HTTP/1.1 200 OK\r\nContent-Type: application/mapi-http\r\nX-ResponseCode: 0\r\n"+("Content-Encoding: gzip\r\n" if compressed else "")+f"Content-Length: {len(response)}\r\n\r\n").encode("ascii")
                archive.writestr(f"raw/{number}_c.txt",request_head+request)
                archive.writestr(f"raw/{number}_s.txt",response_head+response)
                archive.writestr(f"raw/{number}_m.xml",f'<Session SID="{number}"><SessionFlags><SessionFlag N="x-processinfo" V="synthetic.exe:42" /></SessionFlags></Session>')
                if path==output:specifications.append({"name":name,"requestBytes":len(request),"responseBytes":len(response),"host":host})
    output.parent.mkdir(parents=True,exist_ok=True)
    write(output,messages)
    write(output.with_name(output.stem+"-other.saz"),[("other-capture-columns",messages[3][1],"mapi.invalid",False)])
    output.with_suffix(".json").write_text(json.dumps({"cases":specifications},indent=2)+"\n",encoding="utf-8")
    print(f"Wrote {len(messages)} synthetic MAPI exchanges and an independent capture")


if __name__=="__main__":main()
