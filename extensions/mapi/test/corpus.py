"""Synthetic protocol cases, independent of compiler/runtime implementation."""
import base64
import json
from pathlib import Path
import struct
import importlib.util


def u16(value):
    return struct.pack("<H", value)


def u32(value):
    return struct.pack("<I", value)


def wide(text):
    return (text + "\0").encode("utf-16-le")


def rpc(rops, handle=0xffffffff):
    payload=u16(2+len(rops))+rops+u32(handle)
    return struct.pack("<HHHH",0,4,len(payload),len(payload))+payload


def execute_request(rops, handle=0xffffffff):
    buffer=rpc(rops,handle)
    return u32(0)+u32(len(buffer))+buffer+u32(65536)+u32(0)


def execute_response(rops, handle=0x12345678):
    buffer=rpc(rops,handle)
    return b"DONE\r\n\r\n"+u32(0)*3+u32(len(buffer))+buffer+u32(0)


def rop_shapes():
    shapes=json.loads(Path(__file__).with_name("rop-shapes.json").read_text(encoding="utf-8"))
    logon=base64.b64decode(shapes["logon"])
    requests={item["operation"]:base64.b64decode(item["bytes"]) for item in shapes["items"] if item["direction"]=="request"}
    result=[]
    for item in shapes["items"]:
        direction=item["direction"]
        payload=base64.b64decode(item["bytes"])
        historical=item["operation"] in ["RopWritePerUserInformation","RopSetMessageReadFlag"]
        paired=payload if direction=="request" else requests.get(item["operation"],bytes([1,7,0]))
        incoming=execute_request((b"" if historical else logon)+paired)
        outgoing=execute_response(payload) if direction=="response" else b""
        if direction=="request" and item["operation"]=="RopSetColumns":
            outgoing=execute_response(bytes([item["id"],0])+u32(0x8004010f))
        context=[{"requestType":"Execute","request":base64.b64encode(execute_request(logon,7)).decode("ascii")} ] if historical else []
        unit=case("shape-"+item["parser"],"rop-shape","Execute",incoming,outgoing,direction=direction,parser=item["parser"],arguments=item["arguments"])
        unit.update(prepareRequest=True,context=context,offset=18+(0 if historical else len(logon)) if direction=="request" else 34,count=len(payload))
        result.append(unit)
        dispatched=case("dispatch-"+item["parser"],"rop-dispatch","Execute",incoming,outgoing,direction=direction)
        dispatched["context"]=context
        result.append(dispatched)
    return result


def case(name, family, operation, request=b"", response=b"", *, direction="request", parser=None, arguments=None):
    result = {"name": name, "family": family, "expected": "complete", "message": {"direction": direction, "requestType": operation,
              "request": base64.b64encode(request).decode("ascii"), "response": base64.b64encode(response).decode("ascii")}}
    if parser:
        result["type"] = "MAPIInspector.Parsers." + parser
    if arguments is not None:
        result["arguments"] = arguments
    return result


def cases():
    result = []
    prefix = b"PROCESSING\r\nX-ServerApplication: Synthetic\r\nDONE\r\n\r\n"
    connect = b"/o=Example/ou=Synthetic/cn=User\0" + struct.pack("<IIIII", 0x1020304, 1252, 1033, 1041, 0)
    connected = prefix + struct.pack("<IIIII", 0, 0, 60, 3, 500) + b"Synthetic\0" + wide("Example user") + u32(0)
    result += [case("connect-request-nonzero", "mailbox", "Connect", connect),
               case("connect-response-nonzero", "mailbox", "Connect", connect, connected, direction="response"),
               case("connect-protocol-failure", "mailbox", "Connect", connect, prefix + u32(0x80004005) + u32(0), direction="response"),
               case("disconnect-request", "mailbox", "Disconnect", u32(0)),
               case("disconnect-response", "mailbox", "Disconnect", u32(0), prefix + u32(0) + u32(0) + u32(0), direction="response"),
               case("notification-request-flags", "notifications", "NotificationWait", u32(1) + u32(0)),
               case("notification-response-pending", "notifications", "NotificationWait", u32(1) + u32(0), prefix + u32(0) + u32(0) + u32(1) + u32(0), direction="response")]
    primitives = {
        "PtypInteger16": [struct.pack("<h", value) for value in [-32768, -17, 32767]],
        "PtypInteger32": [struct.pack("<i", value) for value in [-2147483648, -123456789, 2147483647]],
        "PtypInteger64": [struct.pack("<q", value) for value in [-9223372036854775808, -9007199254740993, 9007199254740993, 9223372036854775807]],
        "PtypTime": [struct.pack("<Q", value) for value in [0, 132537600000000000, 18446744073709551615]],
        "PtypBoolean": [bytes([value]) for value in [0, 1, 255]],
        "PtypBooleanShort": [u16(value) for value in [0, 1, 256]],
        "PtypFloating32": [struct.pack("<f", value) for value in [-0.5, 1.25, 1234.5]],
        "PtypFloating64": [struct.pack("<d", value) for value in [-0.125, 1.25, 1234.5]],
        "PtypCurrency": [struct.pack("<q", value) for value in [-123456789012345, 9007199254740993]],
        "PtypGuid": [bytes(16), bytes.fromhex("33221100554477668899aabbccddeeff")],
        "PtypString8": [b"Synthetic text\0", b"Subject: example\0"],
        "PtypString": [wide("Synthetic subject"), wide("Mail: café Ω")],
        "PropertyTag": [u32(0x0037001f), u32(0x0e070003), u32(0x300b0102)],
    }
    for parser, values in primitives.items():
        for index, data in enumerate(values):
            item=case(f"{parser}-{index}", "properties", "Execute", data, parser=parser)
            if parser=="PtypTime":
                item["timestamp"]=["1601-01-01T00:00:00.0000000Z","2020-12-30T00:00:00.0000000Z",None][index]
            if parser in ["PtypString8","PtypString"]:
                item["stringValue"]=data[:-1].decode("ascii") if parser=="PtypString8" else data[:-2].decode("utf-16-le")
                item["charset"]="us-ascii" if parser=="PtypString8" else "utf-16le"
            result.append(item)
    for parser, element in [("PtypMultipleInteger16",struct.pack("<hhh",-17,0,1234)),("PtypMultipleInteger32",struct.pack("<iii",-123,456,2147483647)),
                            ("PtypMultipleInteger64",struct.pack("<qqq",-9007199254740993,42,9007199254740993)),
                            ("PtypMultipleFloating32",struct.pack("<fff",-0.5,1.25,1234.5)),("PtypMultipleFloating64",struct.pack("<ddd",-0.125,1.25,1234.5)),
                            ("PtypMultipleGuid",bytes(16)+bytes.fromhex("33221100554477668899aabbccddeeff")+bytes.fromhex("ffeeddccbbaa99887766554433221100"))]:
        result.append(case(parser+"-three", "multivalues", "Execute", u32(3)+element, parser=parser))
    for size, address_book in [(0,False),(5,False),(5,True)]:
        data=(b"\x01" if address_book else b"")+u16(size)+bytes(range(size))
        result.append(case(f"binary-{size}-{address_book}", "properties", "Execute", data, parser="PtypBinary",arguments=[0,address_book]))
    for parser, encoded in [("PtypString8",b"Address-book value\0"),("PtypString",wide("Address-book Ω"))]:
        result.append(case(parser+"-address-book", "address-book", "GetProps", b"\x01"+encoded,parser=parser,arguments=[True]))
        absent=case(parser+"-absent", "address-book", "GetProps", b"\x00",parser=parser,arguments=[True])
        absent["intentionalDifference"]="absent-string-layout"
        result.append(absent)
    payload=u16(8)+bytes([1,0x48])+u32(1)
    for compressed,obfuscated in [(False,False),(False,True),(True,False),(True,True)]:
        encoded=(u32(0)+payload) if compressed else payload
        if obfuscated:
            encoded=bytes(value^0xa5 for value in encoded)
        flags=4+int(compressed)+2*int(obfuscated)
        extended=struct.pack('<HHHH',0,flags,len(encoded),len(payload))+encoded
        fixture=case(f"auxiliary-compressed-{compressed}-xor-{obfuscated}","compression","Connect",extended,parser="ExtendedBuffer")
        fixture["includeDerived"]=True
        fixture["derivedExpectation"]={"bytes":list(payload),"encodedBytes":len(encoded),"transformed":compressed or obfuscated,"exactMapping":not compressed}
        result.append(fixture)
    specification=importlib.util.spec_from_file_location("mapi_scenarios",Path(__file__).with_name("scenarios.py"))
    scenarios=importlib.util.module_from_spec(specification)
    specification.loader.exec_module(scenarios)
    return result+rop_shapes()+scenarios.cases(case,execute_request,execute_response)
