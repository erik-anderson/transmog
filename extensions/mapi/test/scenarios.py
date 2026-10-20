"""Nonzero synthetic mailbox scenarios, encoded independently of the parser."""
import base64
import json
from pathlib import Path
import struct


def u16(value): return struct.pack("<H",value)
def u32(value): return struct.pack("<I",value)
def u64(value): return struct.pack("<Q",value)
def wide(value): return (value+"\0").encode("utf-16-le")


def cases(case, execute_request, execute_response):
    shapes=json.loads(Path(__file__).with_name("rop-shapes.json").read_text(encoding="utf-8"))
    ids={item["operation"]:item["id"] for item in shapes["items"]}
    logon=base64.b64decode(shapes["logon"])
    folder=bytes.fromhex("3412010203040506")
    message=bytes.fromhex("7856060504030201")
    tags=u32(0x0037001f)+u32(0x0e070003)+u32(0x0e080014)
    values=wide("Synthetic café Ω")+u32(0x10203040)+u64(9007199254740993)
    tagged=u32(0x0037001f)+wide("Synthetic café Ω")+u32(0x0e070003)+u32(0x10203040)+u32(0x0e080014)+u64(9007199254740993)
    stream=b"Synthetic stream\0\xff\x80"
    # InputHandleIndex and OutputHandleIndex deliberately reuse a single handle.
    examples=[
        ("RopOpenFolder",b"\0"+folder+b"\1",b"\1\1"+u16(2)+u16(1)+b"server-one\0server-two\0"),
        ("RopCreateFolder",b"\0\1\1\1\0"+wide("Synthetic folder Ω")+wide("Example comment"),folder+b"\1\1\1"+u16(1)+u16(1)+b"server-one\0"),
        ("RopGetHierarchyTable",b"\0\4",u32(123)),
        ("RopGetContentsTable",b"\0\2",u32(456)),
        ("RopCreateMessage",b"\0"+u16(1252)+folder+b"\1",b"\1"+message),
        ("RopOpenMessage",b"\0"+u16(1252)+folder+b"\3"+message,b"\1\2RE: \0\3"+wide("Synthetic message Ω")+u16(0)+u16(0)+b"\0"),
        ("RopOpenStream",b"\0"+u32(0x10000102)+b"\1",u32(4097)),
        ("RopReadStream",u16(0xbabe)+u32(65536),u16(len(stream))+stream),
        ("RopWriteStream",u16(len(stream))+stream,u16(len(stream))),
        ("RopSeekStream",b"\2"+u64(9007199254740993),u64(9007199254741017)),
        ("RopGetPropertiesAll",u16(4096)+u16(1),u16(3)+tagged),
        ("RopGetPropertiesSpecific",u16(4096)+u16(1)+u16(3)+tags,b"\0"+values),
        ("RopSetProperties",u16(2+len(tagged))+u16(3)+tagged,u16(0)),
        ("RopSynchronizationGetTransferState",b"\0",b""),
    ]
    result=[]
    for operation,request_fields,response_fields in examples:
        opcode=ids[operation]
        incoming=execute_request(logon+bytes([opcode,7,0])+request_fields,0x12345678)
        outgoing=execute_response(bytes([opcode,0])+u32(0)+response_fields)
        for direction in ["request","response"]:
            result.append(case("success-"+operation+"-"+direction,"mailbox-scenario","Execute",incoming,outgoing,direction=direction))
    # SetColumns and QueryRows in one exchange and in retained earlier traffic.
    set_columns=bytes([ids["RopSetColumns"],7,0,0])+u16(3)+tags
    query=bytes([ids["RopQueryRows"],7,0,0,1])+u16(2)
    columns_response=bytes([ids["RopSetColumns"],0])+u32(0)+b"\0"
    rows_response=bytes([ids["RopQueryRows"],0])+u32(0)+b"\0"+u16(2)+b"\0"+values+b"\0"+values
    result.append(case("success-table-columns-and-rows","table-context","Execute",execute_request(set_columns+query,0x12345678),execute_response(columns_response+rows_response),direction="response"))
    retained=case("success-table-retained-columns","table-context","Execute",execute_request(query,0x12345678),execute_response(rows_response),direction="response")
    retained["context"]=[{"order":1,"requestType":"Execute","request":base64.b64encode(execute_request(set_columns,0x12345678)).decode("ascii"),
                          "response":base64.b64encode(execute_response(columns_response)).decode("ascii")}]
    retained["message"]["order"]=3
    result.append(retained)
    return result
