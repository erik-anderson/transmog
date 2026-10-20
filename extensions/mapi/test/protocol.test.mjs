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
