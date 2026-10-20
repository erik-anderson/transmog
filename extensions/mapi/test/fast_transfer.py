"""Synthetic FastTransfer, synchronization and multi-buffer wire scenarios."""
import base64
import json
from pathlib import Path
import struct


def u16(value): return struct.pack("<H",value)
def u32(value): return struct.pack("<I",value)
def u64(value): return struct.pack("<Q",value)
def wide(value): return (value+"\0").encode("utf-16-le")


def lz77(payload):
    """Small independent encoder exercising literal and 3..9 byte back-references."""
    tokens=[]
    position=0
    while position<len(payload):
        best=(0,0)
        for offset in range(1,min(position,8192)+1):
            length=0
            while length<9 and position+length<len(payload) and payload[position+length]==payload[position+length-offset]:
                length+=1
            if length>=3 and length>best[1]:best=(offset,length)
        offset,length=best
        if length>=3:
            tokens.append((True,u16(((offset-1)<<3)|(length-3))))
            position+=length
        else:
            tokens.append((False,payload[position:position+1]))
            position+=1
    output=b""
    for start in range(0,len(tokens),32):
        group=tokens[start:start+32]
        mask=sum(1<<(31-index) for index,(match,_) in enumerate(group) if match)
        output+=u32(mask)+b"".join(value for _,value in group)
    return output


def cases(case, execute_request, execute_response):
    shapes=json.loads(Path(__file__).with_name("rop-shapes.json").read_text(encoding="utf-8"))
    ids={item["operation"]:item["id"] for item in shapes["items"]}
    guid=bytes.fromhex("33221100554477668899aabbccddeeff")
    string=wide("Synthetic transfer café Ω")
    fixed=u32(0x0e070003)+u32(0x10203040)+u32(0x0e080014)+u64(9007199254740993)
    variable=u32(0x0037001f)+u32(len(string))+string+u32(0x0fff0102)+u32(7)+b"\0\xff\x80DATA"
    named=u32(0x80010003)+guid+b"\0"+u32(0x8501)+u32(123456)
    named+=u32(0x8002001f)+guid+b"\1"+wide("Custom Ω")+u32(len(string))+string
    multiple=u32(0x0e071003)+u32(3)+struct.pack("<iii",-17,1234,2147483647)
    marker_stream=u32(0x400c0003)+u32(0x400f0003)+u32(0x8004010f)+fixed+variable+multiple+named+u32(0x400d0003)
    get_id=ids["RopFastTransferSourceGetBuffer"]

    def get_request(): return execute_request(bytes([get_id,7,0])+u16(4096),0x12345678)
    def get_response(chunk,status=3):
        return execute_response(bytes([get_id,0])+u32(0)+u16(status)+u16(3)+u16(7)+b"\0"+u16(len(chunk))+chunk)
    def put_request(chunk,operation="RopFastTransferDestinationPutBuffer",handle=0xffffffff):
        return execute_request(bytes([ids[operation],7,0])+u16(len(chunk))+chunk,handle)
    def put_response(chunk,operation="RopFastTransferDestinationPutBuffer"):
        # The extended response uses 32-bit counters instead of 16-bit counters.
        count=u32 if operation.endswith("Extended") else u16
        return execute_response(bytes([ids[operation],0])+u32(0)+u16(3)+count(3)+count(7)+b"\0"+u16(len(chunk)))

    result=[]
    for name,stream in [("fixed",fixed),("variable",variable),("markers-named-multivalue",marker_stream)]:
        result.append(case("ft-get-"+name,"fast-transfer","Execute",get_request(),get_response(stream),direction="response"))
        for operation in ["RopFastTransferDestinationPutBuffer","RopFastTransferDestinationPutBufferExtended"]:
            request=put_request(stream,operation)
            response=put_response(stream,operation)
            for direction in ["request","response"]:
                result.append(case("ft-"+operation+"-"+name+"-"+direction,"fast-transfer","Execute",request,response,direction=direction))
    # Continue a binary value and a UTF-16 code unit across retained exchanges.
    for name,stream,split in [("binary",u32(0x0fff0102)+u32(16)+bytes(range(16)),11),
                              ("unicode",u32(0x0037001f)+u32(len(string))+string,13),
                              ("fixed-header",u32(0x0e070003)+u32(123456),4)]:
        for operation in ["get","RopFastTransferDestinationPutBuffer","RopFastTransferDestinationPutBufferExtended"]:
            direction="response" if operation=="get" else "request"
            def exchange(chunk,order,final=False):
                incoming=get_request() if operation=="get" else put_request(chunk,operation,0x12345678)
                outgoing=get_response(chunk,3 if final else 1) if operation=="get" else put_response(chunk,operation)
                return {"order":order,"direction":direction,"requestType":"Execute","request":base64.b64encode(incoming).decode("ascii"),"response":base64.b64encode(outgoing).decode("ascii")}
            fixture=case("ft-partial-"+operation+"-"+name,"fast-transfer-partial","Execute",direction=direction)
            fixture["message"]=exchange(stream[split:],30,True)
            fixture["context"]=[exchange(stream[:split],10)]
            result.append(fixture)
            if name=="unicode":
                reordered=case("ft-out-of-order-"+operation,"fast-transfer-partial","Execute",direction=direction)
                reordered["message"]=exchange(stream[21:],30,True)
                reordered["context"]=[exchange(stream[split:21],20),exchange(stream[:split],10)]
                if operation.endswith("Extended"):
                    reordered["referenceUnsupported"]="extended-upload-continuation-state"
                    reordered["semanticString"]="ic transfer café Ω"
                result.append(reordered)
    exist=b"\x08"+u32(0x0037001f)
    restriction=b"\0"+u16(2)+exist+b"\2"+exist
    configure=bytes([ids["RopSynchronizationConfigure"],7,0,0,1,1])+u16(0x10)+u16(len(restriction))+restriction+u32(1)+u16(2)+u32(0x0037001f)+u32(0x0e070003)
    read_states=u16(4)+b"ID01\1"+u16(4)+b"ID02\0"
    read_request=bytes([ids["RopSynchronizationImportReadStateChanges"],7,0])+u16(len(read_states))+read_states
    for name,operation,payload in [("configure","RopSynchronizationConfigure",configure),("read-states","RopSynchronizationImportReadStateChanges",read_request)]:
        request=execute_request(payload,0x12345678)
        response=execute_response(bytes([ids[operation],0])+u32(0))
        for direction in ["request","response"]:
            result.append(case("sync-"+name+"-"+direction,"synchronization","Execute",request,response,direction=direction))
    for name,payload in [("and-not-exist",restriction),
                         ("content",b"\3"+u16(1)+u16(1)+u32(0x0037001f)+u32(0x0037001f)+wide("café Ω")),
                         ("property",b"\4\4"+u32(0x0e070003)+u32(0x0e070003)+u32(17))]:
        result.append(case("restriction-"+name,"restrictions","Execute",payload,parser="RestrictionType",arguments=[0]))
    for parser,data,arguments in [
        ("PtypMultipleString",u32(2)+wide("café")+wide("Ω"),[False]),
        ("PtypMultipleString8",u32(2)+b"one\0two\0",[False]),
        ("PtypMultipleBinary",u32(2)+u16(3)+b"ONE"+u16(4)+b"\0\xff\x80B",[0,False]),
        ("PtypMultipleCurrency",u32(2)+struct.pack("<qq",-123456789012345,9007199254740993),[]),
    ]:
        result.append(case("multiple-"+parser,"multivalues","Execute",data,parser=parser,arguments=arguments))
    # Multiple RPC buffers: first plain, second compressed/obfuscated. The
    # compressed payload contains actual back-references as well as literals.
    for direction in ["request","response"]:
        for flags in [0,1,2,3]:
            rops=bytes([ids["RopReadStream"],7,0])+u16(17) if direction=="request" else bytes([ids["RopReadStream"],0])+u32(0)+u16(4)+b"DATA"
            payload=u16(2+len(rops))+rops+u32(0x12345678)
            encoded=lz77(payload) if flags&1 else payload
            if flags&2: encoded=bytes(value^0xa5 for value in encoded)
            first=struct.pack("<HHHH",0,0,len(payload),len(payload))+payload
            second=struct.pack("<HHHH",0,flags|4,len(encoded),len(payload))+encoded
            buffers=first+second
            request=u32(0)+u32(len(buffers))+buffers+u32(65536)+u32(0) if direction=="request" else execute_request(bytes([ids["RopReadStream"],7,0])+u16(17),0x12345678)
            response=b"DONE\r\n\r\n"+u32(0)*3+u32(len(buffers))+buffers+u32(0) if direction=="response" else b""
            fixture=case(f"rpc-multiple-{direction}-{flags}","rpc-buffers","Execute",request,response,direction=direction)
            fixture["includeDerived"]=True
            fixture["rpcDerivedExpectations"]=[] if not flags else [{"bytes":list(payload),"offset":str((8 if direction=="request" else 24)+len(first)+8),"length":str(len(encoded)),"exactMapping":not bool(flags&1)}]
            result.append(fixture)
    return result
