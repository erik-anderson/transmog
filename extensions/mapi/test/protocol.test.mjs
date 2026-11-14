import assert from "node:assert/strict";
import { test } from "node:test";
import { decode } from "../dist/decode.js";
import { BinaryReader } from "../dist/reader.js";
import { decompressLz77 } from "../dist/rpc.js";
import { RecordReader } from "../dist/record.js";
import { readRuleActions } from "../dist/rules.js";
import { readSynchronizationBinary } from "../dist/synchronization.js";
import { readAuxiliaryBody } from "../dist/auxiliary.js";
import { enums } from "../dist/constants.js";
import { FastTransferDecoder } from "../dist/fast-transfer.js";
import { readNamedProperty } from "../dist/properties.js";
import { mailboxLogonFolderCount, privateMailboxFlag, wireSizes } from "../dist/wire.js";

function uint32(value) {
  return [value & 255, (value >>> 8) & 255, (value >>> 16) & 255, value >>> 24];
}

function uint16(value) {
  return [value & 255, value >>> 8];
}

function unicode(value) {
  return Array.from(Buffer.from(value + "\0", "utf16le"));
}

function execute(rop, response) {
  const payload = [rop.length + 2, 0, ...rop, ...uint32(0x12345678)];
  const rpc = [0, 0, 4, 0, payload.length, 0, payload.length, 0, ...payload];
  return response
    ? [
        ...Buffer.from("DONE\r\n\r\n"),
        ...uint32(0),
        ...uint32(0),
        ...uint32(0),
        ...uint32(rpc.length),
        ...rpc,
        ...uint32(0),
      ]
    : [...uint32(0), ...uint32(rpc.length), ...rpc, ...uint32(65536), ...uint32(0)];
}

function fields(root) {
  return [root, ...root.children.flatMap(fields)];
}

test("logon responses use their own flags when the paired request is unavailable", () => {
  const opcode = enums.RopIdType.values.RopLogon;
  const guid = [
    0x33, 0x22, 0x11, 0, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
  ];
  for (const flags of [0, privateMailboxFlag]) {
    const privateMailbox = (flags & privateMailboxFlag) !== 0;
    const response = execute(
      [
        opcode,
        0,
        ...uint32(0),
        flags,
        ...Array(mailboxLogonFolderCount * wireSizes.objectId).fill(0),
        ...(privateMailbox ? [0, ...guid] : []),
        ...uint16(7),
        ...guid,
        ...(privateMailbox
          ? [30, 20, 10, 1, 14, 11, ...uint16(2026), ...Array(8).fill(0), ...uint32(5)]
          : guid),
      ],
      true,
    );
    const result = decode(
      [{ id: 1, requestType: "Execute", response: { sourceId: "logon", bytes: response } }],
      1,
      "response",
    );
    assert.ok(fields(result.root).every((field) => !field.issue));
    const parsedGuid = fields(result.root).find(
      (field) => field.name === (privateMailbox ? "MailboxGuid" : "PerUserGuid"),
    );
    assert.ok(
      parsedGuid,
      `Missing GUID for flags ${flags}: ${fields(result.root)
        .map((field) => field.name)
        .join(", ")}`,
    );
    assert.equal(parsedGuid.value.value, "00112233-4455-6677-8899-aabbccddeeff");
    if (privateMailbox) {
      assert.equal(
        fields(result.root).find((field) => field.name === "StoreState").value.value,
        "5",
      );
    }
  }
});

test("first per-user writes require logon context while later fragments do not", () => {
  const logonId = 7;
  const replicaGuid = [
    0x33, 0x22, 0x11, 0, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
  ];
  const data = [0xaa, 0xbb];
  const followingRop = [enums.RopIdType.values.RopRelease, logonId, 0];
  const write = (offset, flags) =>
    execute(
      [
        enums.RopIdType.values.RopWritePerUserInformation,
        logonId,
        0,
        ...Array(wireSizes.longTermId).fill(0),
        1,
        ...uint32(offset),
        ...uint16(data.length),
        ...data,
        ...(offset === 0 && (flags & privateMailboxFlag) !== 0 ? replicaGuid : []),
        ...followingRop,
      ],
      false,
    );
  const selected = (offset, flags) => ({
    id: 2,
    requestType: "Execute",
    request: { sourceId: "write", bytes: write(offset, flags) },
  });
  const missing = decode([selected(0, 0)], 2, "request");
  assert.equal(missing.root.issue?.kind, "missing-context");
  assert.ok(
    fields(missing.root).some(
      (field) => field.name === "Data" && field.range.length === data.length,
    ),
  );
  assert.ok(fields(missing.root).every((field) => field.name !== "RopRelease request"));

  for (const flags of [0, privateMailboxFlag]) {
    const history = {
      id: 1,
      requestType: "Execute",
      request: {
        sourceId: "logon",
        bytes: execute(
          [
            enums.RopIdType.values.RopLogon,
            logonId,
            0,
            flags,
            ...uint32(0),
            ...uint32(0),
            ...uint16(0),
          ],
          false,
        ),
      },
    };
    const result = decode([history, selected(0, flags)], 2, "request");
    const parsed = fields(result.root);
    assert.ok(parsed.every((field) => !field.issue));
    assert.ok(parsed.some((field) => field.name === "RopRelease request"));
    const guid = parsed.find((field) => field.name === "ReplGuid");
    assert.equal(
      guid?.value.value,
      flags === 0 ? undefined : "00112233-4455-6677-8899-aabbccddeeff",
    );
  }
  const continuation = decode([selected(data.length, 0)], 2, "request");
  assert.ok(fields(continuation.root).every((field) => !field.issue));
  assert.ok(fields(continuation.root).some((field) => field.name === "RopRelease request"));
});

test("named properties distinguish the no-name sentinel from unknown kinds", () => {
  const noName = enums.KindEnum.values.NoPropertyName;
  const followingByte = 0xaa;
  const absent = new RecordReader(new BinaryReader([noName, followingByte], "names"), "Name");
  readNamedProperty(absent);
  assert.equal(absent.reader.offset, 1);
  assert.equal(absent.reader.peekUInt8(), followingByte);
  assert.deepEqual(absent.field.children[0].value.names, ["NoPropertyName"]);

  for (const kind of [2, 127, 254]) {
    const record = new RecordReader(new BinaryReader([kind, followingByte], "names"), "Name");
    assert.throws(
      () => readNamedProperty(record),
      (error) => error.kind === "unsupported" && error.message.includes("named property kind"),
    );
    assert.equal(record.reader.offset, 1, "An unknown layout consumed the following field");
    assert.equal(record.reader.peekUInt8(), followingByte);
  }

  const opcode = enums.RopIdType.values.RopGetNamesFromPropertyIds;
  for (const kind of [noName, 2]) {
    const response = execute([opcode, 0, ...uint32(0), ...uint16(1), kind], true);
    const result = decode(
      [{ id: 1, requestType: "Execute", response: { sourceId: "names", bytes: response } }],
      1,
      "response",
    );
    assert.equal(result.root.issue?.kind, kind === noName ? undefined : "unsupported");
  }
});

test("OptionsData reads its real return status and bounded success fields", () => {
  for (const status of [0, 0x8004010f]) {
    const rop = [0x6f, 0, ...uint32(status), ...(status === 0 ? [0, 2, 0, 0xde, 0xad, 0, 0] : [])];
    const result = decode(
      [
        {
          id: 1,
          requestType: "Execute",
          request: { sourceId: "request", bytes: execute([0x6f, 7, 0, 0, 0], false) },
          response: { sourceId: "response", bytes: execute(rop, true) },
        },
      ],
      1,
      "response",
    );
    assert.ok(fields(result.root).every((field) => !field.issue));
    const code = fields(result.root).find((field) => field.name === "ReturnValue");
    assert.equal(code.value.value, String(status));
    if (status === 0) {
      const data = fields(result.root).find((field) => field.name === "OptionsInfo");
      assert.equal(data.range.length, 2);
    }
  }
});

test("LZ77 matches overlap and malformed matches cannot read outside output", () => {
  const input = [0, 0, 0, 16, 65, 66, 67, 19, 0];
  assert.deepEqual(
    decompressLz77(new BinaryReader(input, "encoded"), 9),
    [65, 66, 67, 65, 66, 67, 65, 66, 67],
  );
  assert.throws(() => decompressLz77(new BinaryReader([0, 0, 0, 128, 0, 0], "encoded"), 3));
  assert.throws(() => decompressLz77(new BinaryReader(input, "encoded"), 8));
  assert.throws(() => decompressLz77(new BinaryReader(input, "encoded"), 10));
});

test("a malformed decode cannot provide state to the next decode", () => {
  const malformed = { id: 1, requestType: "Execute", request: { sourceId: "bad", bytes: [0] } };
  assert.ok(decode([malformed], 1, "request").root.issue);
  const bytes = execute([0x54, 7, 0, 0, 0, 1, 7, 0], false);
  const recovered = decode(
    [{ id: 1, requestType: "Execute", request: { sourceId: "good", bytes } }],
    1,
    "request",
  );
  assert.ok(fields(recovered.root).every((field) => !field.issue));
  assert.ok(fields(recovered.root).some((field) => field.name === "RopRelease request"));
});

test("rule actions keep their own lengths and property count widths", () => {
  const action = (type, bytes) => [
    ...uint16(9 + bytes.length),
    type,
    ...uint32(0),
    ...uint32(0),
    ...bytes,
  ];
  const data = [
    ...uint16(3),
    ...action(6, uint32(13)),
    ...action(9, [...uint32(0x0037001f), ...unicode("Rule Ω")]),
    ...action(10, []),
  ];
  const record = new RecordReader(new BinaryReader(data, "rules"), "Rule actions");
  readRuleActions(record, "rop");
  assert.equal(record.reader.remaining, 0);
  assert.ok(fields(record.finish()).some((field) => field.value?.value === "Rule Ω"));
  const extended = [
    ...uint32(1),
    ...uint32(20),
    9,
    ...uint32(0),
    ...uint32(0),
    ...uint32(0x0fff0102),
    ...uint32(3),
    1,
    2,
    3,
  ];
  const second = new RecordReader(new BinaryReader(extended, "extended"), "Rule actions");
  readRuleActions(second, "extended-rule");
  assert.equal(second.reader.remaining, 0);
  assert.ok(
    fields(second.finish()).some((field) => field.name === "Count" && field.range.length === 4),
  );
  const invalid = [...data];
  invalid[2] = 8; // A length cannot fit the mandatory nine-byte action header.
  assert.throws(() =>
    readRuleActions(new RecordReader(new BinaryReader(invalid, "bad"), "Rules"), "rop"),
  );
});

test("synchronization identifier sets enforce stack, range and bitmask boundaries", () => {
  const commands = [5, 1, 2, 3, 4, 5, 0x42, 10, 5, 0x50, 0];
  const data = [...Array(16).fill(0), ...commands];
  const parse = (bytes) => {
    const record = new RecordReader(new BinaryReader(bytes, "ids"), "Identifier set");
    assert.equal(readSynchronizationBinary(record, 0x6796, bytes.length), true);
    assert.equal(record.reader.remaining, 0);
    return record.finish();
  };
  assert.ok(
    fields(parse(data)).some((field) => field.name === "Bitmask" && field.value.value === "5"),
  );
  for (const commands of [
    [0x50, 0],
    [1, 1, 0],
    [0x42, 1, 1, 0],
    [0xff],
    [5, 1, 2, 3, 4, 5, 0x42, 255, 1, 0x50, 0],
  ]) {
    assert.throws(() => parse([...Array(16).fill(0), ...commands]));
  }
});

test("auxiliary strings use declared offsets within their own block", () => {
  const name = unicode("host");
  const dn = unicode("dc=example");
  const bytes = [
    0,
    0,
    1,
    0,
    ...uint16(7),
    ...uint16(1),
    ...uint16(12 + name.length),
    ...uint16(12),
    ...name,
    ...dn,
  ];
  const type = enums.AuxiliaryBlockType_1.values.AUX_TYPE_PERF_SERVERINFO;
  const parse = (bytes) => {
    const reader = new BinaryReader(bytes, "auxiliary");
    const record = new RecordReader(reader, "AUX header");
    reader.skip(4);
    readAuxiliaryBody(record, type, 1);
    return record.finish();
  };
  const values = fields(parse(bytes)).map((field) => field.value?.value);
  assert.ok(values.includes("host") && values.includes("dc=example"));
  const invalid = [...bytes];
  invalid[10] = 255;
  invalid[11] = 255;
  assert.throws(() => parse(invalid));
});

test("FastTransfer variable multivalues carry a length for each encoded value", () => {
  const first = unicode("Ω");
  const second = unicode("two");
  const data = [
    ...uint32(0x0e07101f),
    ...uint32(2),
    ...uint32(first.length),
    ...first,
    ...uint32(second.length),
    ...second,
    ...uint32(0x003784b1),
    ...uint32(4),
    3,
    0xa9,
    0,
    0,
    ...uint32(0x003784e4),
    ...uint32(2),
    0x80,
    0,
  ];
  const record = new RecordReader(new BinaryReader(data, "transfer"), "Stream");
  new FastTransferDecoder().read(record, data.length, "put", 7);
  const parsed = fields(record.finish());
  assert.ok(parsed.every((field) => !field.issue));
  const values = parsed.map((field) => field.value?.value);
  assert.ok(values.includes("Ω") && values.includes("two") && values.includes("€"));
  assert.ok(parsed.some((field) => field.encoding === "utf-16be"));
  assert.ok(parsed.some((field) => field.encoding === "windows-1252"));
});
