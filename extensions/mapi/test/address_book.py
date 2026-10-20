"""Populated synthetic NSPI/MAPI HTTP operations and nullable wire fields."""
import struct


def u32(value): return struct.pack("<I",value)
def u64(value): return struct.pack("<Q",value)
def wide(value): return (value+"\0").encode("utf-16-le")


def cases(case):
    state=b"\1"+struct.pack("<9I",1,0x1234,0x5678,0xffffffff,3,17,1252,1033,1041)
    columns=u32(3)+u32(0x3001001f)+u32(0x39000003)+u32(0x0e080014)
    values=b"\1"+wide("Synthetic person café Ω")+u32(0x12345678)+u64(9007199254740993)
    row=b"\0"+values
    tagged=u32(3)+u32(0x3001001f)+b"\1"+wide("Synthetic person café Ω")+u32(0x39000003)+u32(0x12345678)+u32(0x0e080014)+u64(9007199254740993)
    ids=u32(2)+u32(0x12345678)+u32(0x87654321)
    rows=b"\1"+columns+u32(2)+row+row
    guid=bytes.fromhex("33221100554477668899aabbccddeeff")
    dn=b"/o=Example/ou=Synthetic/cn=Person\0"
    permanent=bytes(4)+guid+u32(1)+u32(0)+dn
    ephemeral=b"\x87\0\0\0"+guid+u32(1)+u32(1)+u32(0x12345678)
    entries=u32(2)+u32(len(permanent))+permanent+u32(len(ephemeral))+ephemeral
    examples=[
        ("Bind",u32(1)+state,guid),
        ("Unbind",u32(0x12345678),b""),
        ("CompareMIds",u32(0)+state+u32(0x12345678)+u32(0x87654321),struct.pack("<i",-1)),
        ("DNToMId",u32(0)+b"\1"+u32(2)+dn+b"/o=Example/cn=Other\0",b"\1"+ids),
        ("GetMatches",u32(0)+state+b"\1"+ids+u32(1)+b"\1\x08"+u32(0x3001001f)+b"\1"+guid+u32(0x8001)+u32(2)+b"\1"+columns,state+b"\1"+ids+rows),
        ("GetPropList",u32(0x10)+u32(0x12345678)+u32(1252),b"\1"+columns),
        ("GetProps",u32(0x10)+state+b"\1"+columns,u32(1252)+b"\1"+tagged),
        ("GetSpecialTable",u32(1)+state+b"\1"+u32(17),u32(1252)+b"\1"+u32(18)+b"\1"+u32(1)+tagged),
        ("GetTemplateInfo",u32(1)+u32(1)+b"\1"+dn+u32(1252)+u32(1033),u32(1252)+b"\1"+tagged),
        ("ModLinkAtt",u32(1)+u32(0x0fff0102)+u32(0x12345678)+b"\1"+entries,b""),
        ("ModProps",u32(0)+state+b"\1"+columns+b"\1"+tagged,b""),
        ("QueryRows",u32(1)+state+ids+u32(2)+b"\1"+columns,state+rows),
        ("QueryColumns",u32(0)+u32(0x80000000),b"\1"+columns),
        ("ResolveNames",u32(0)+state+b"\1"+columns+b"\1"+u32(2)+wide("Person Ω")+wide("Other café"),u32(1252)+b"\1"+ids+rows),
        ("ResortRestriction",u32(0)+state+b"\1"+ids,state+b"\1"+ids),
        ("SeekEntries",u32(0)+state+b"\1"+u32(0x3001001f)+b"\1"+wide("Person Ω")+b"\1"+ids+b"\1"+columns,state+rows),
        ("UpdateStat",u32(0)+state+b"\1",state+b"\1"+struct.pack("<i",-7)),
        ("GetMailboxUrl",u32(1)+wide("/o=Example/cn=Mailbox"),wide("https://mail.example.test/mapi/emsmdb")),
        ("GetAddressBookUrl",u32(1)+wide("/o=Example/cn=Person"),wide("https://mail.example.test/mapi/nspi")),
    ]
    result=[]
    prefix=b"PROCESSING\r\nX-ServerApplication: Synthetic\r\nDONE\r\n\r\n"
    for operation,incoming,outgoing in examples:
        request=incoming+u32(0)
        response=prefix+u32(0)+u32(0)+outgoing+u32(0)
        for direction in ["request","response"]:
            result.append(case("nspi-"+operation+"-"+direction,"address-book",operation,request,response,direction=direction))
        result.append(case("nspi-"+operation+"-failure","address-book-status",operation,request,prefix+u32(0x80004005)+u32(0),direction="response"))
    for name,operation,incoming,outgoing in [
        ("bind-without-state","Bind",u32(0)+b"\0",guid),
        ("props-without-values","GetProps",u32(0)+b"\0\0",u32(1252)+b"\0"),
        ("rows-without-state-or-rows","QueryRows",u32(0)+b"\0"+u32(0)+u32(1)+b"\0",b"\0\0"),
        ("names-without-arrays","ResolveNames",u32(0)+b"\0\0\0",u32(1252)+b"\0\0"),
    ]:
        for direction in ["request","response"]:
            result.append(case("nspi-"+name+"-"+direction,"address-book-optional",operation,incoming+u32(0),prefix+u32(0)*2+outgoing+u32(0),direction=direction))
    return result
