import assert from "node:assert/strict";
import { test } from "node:test";
import { decode } from "../dist/decode.js";
import { enums } from "../dist/constants.js";
import { present } from "../dist/view.js";
import { parserLimits } from "../dist/limits.js";
import { BinaryReader } from "../dist/reader.js";
import { RecordReader } from "../dist/record.js";
import { readRestriction } from "../dist/properties.js";
import { decompressLz77 } from "../dist/rpc.js";

const u16 = (value) => [value & 255, value >>> 8];
const u32 = (value) => [value & 255, (value >>> 8) & 255, (value >>> 16) & 255, value >>> 24];
const opcode = enums.RopIdType.values.RopGetPropertiesAll;
const requestRop = [opcode, 7, 0, ...u16(65535), ...u16(1)];
const fields = (root) => [root, ...root.children.flatMap(fields)];

function rpcPayload(rop) {
  return [...u16(rop.length + 2), ...rop, ...u32(0x12345678)];
}

function plainRpc(rop) {
  const payload = rpcPayload(rop);
  return [...u16(0), ...u16(4), ...u16(payload.length), ...u16(payload.length), ...payload];
}

function execute(rpc, response) {
  return response
    ? [13, 10, ...u32(0), ...u32(0), ...u32(0), ...u32(rpc.length), ...rpc, ...u32(0)]
    : [...u32(0), ...u32(rpc.length), ...rpc, ...u32(65536), ...u32(0)];
}

function responseRop(characters) {
  return [
    opcode,
    0,
    ...u32(0),
    ...u16(1),
    ...u32(0x0037001f),
    ...Buffer.from("A".repeat(characters) + "\0", "utf16le"),
  ];
}

function selected(id = 1, characters = 7600) {
  return {
    id,
    requestType: "Execute",
    request: { sourceId: "selected-request", bytes: execute(plainRpc(requestRop), false) },
    response: {
      sourceId: "selected-response",
      bytes: execute(plainRpc(responseRop(characters)), true),
    },
  };
}

function history(count, length) {
  return Array.from({ length: count }, (_, index) => ({
    id: index + 1,
    requestType: "Connect",
    request: {
      sourceId: "context-" + index,
      bytes: [...Array(length).fill(65), 0, ...Array(20).fill(0)],
    },
  }));
}

test("a 14.9 KiB Execute response remains complete after replaying earlier messages", () => {
  const message = selected(33);
  assert.ok(
    message.response.bytes.length > 14.8 * 1024 && message.response.bytes.length < 15 * 1024,
  );
  const result = decode([...history(32, 4096), message], message.id, "response");
  assert.ok(fields(result.root).every((field) => !field.issue));
  assert.ok(fields(result.root).some((field) => field.value?.value === "A".repeat(7600)));
  const view = present(
    result,
    [["selected-response", message.response.bytes.length]],
    "Execute",
    [],
  ).step;
  assert.equal(
    view.document.status,
    "complete",
    "Retained context changed the selected parse status",
  );
  assert.deepEqual(view.document.diagnostics, []);
});

test("small compressed messages retain complete decoded values without work accounting", () => {
  const characters = 30000;
  const payload = rpcPayload(responseRop(characters));
  const prefix = payload.slice(0, payload.length - characters * 2 - 6);
  prefix.push(65, 0);
  const length = characters * 2 - 2;
  // Independently encode literal prefix, one distance-two overlapping match,
  // UTF-16 terminator and handle table. The match uses all length extensions.
  const compressed = [
    ...u32(2 ** (31 - prefix.length)),
    ...prefix,
    15,
    0,
    15,
    255,
    ...u16(length - 3),
    0,
    0,
    ...u32(0x12345678),
  ];
  const buffers = Array.from({ length: 4 }, (_, index) => [
    ...u16(0),
    ...u16(index === 3 ? 5 : 1),
    ...u16(compressed.length),
    ...u16(payload.length),
    ...compressed,
  ]).flat();
  const message = {
    id: 1,
    requestType: "Execute",
    request: {
      sourceId: "request",
      bytes: execute(plainRpc(Array.from({ length: 4 }, () => requestRop).flat()), false),
    },
    response: { sourceId: "response", bytes: execute(buffers, true) },
  };
  assert.ok(message.response.bytes.length < 15 * 1024);
  const result = decode([message], 1, "response");
  assert.ok(
    fields(result.root).every((field) => !field.issue),
    JSON.stringify(
      fields(result.root)
        .filter((field) => field.issue)
        .map((field) => field.issue),
    ),
  );
  assert.equal(result.derivedSources.length, 4);
  assert.equal(
    fields(result.root).filter((field) => field.value?.value === "A".repeat(characters)).length,
    4,
  );
});

test("field-heavy context cannot consume selected-message structural limits", () => {
  const message = selected(65, 50);
  const context = Array.from({ length: 64 }, (_, index) => ({
    id: index + 1,
    requestType: "Execute",
    request: {
      sourceId: "context-" + index,
      bytes: execute(plainRpc(Array.from({ length: 1000 }, () => requestRop).flat()), false),
    },
  }));
  const result = decode([...context, message], message.id, "response");
  assert.ok(fields(result.root).some((field) => field.value?.value === "A".repeat(50)));
  assert.ok(
    fields(result.root).some(
      (field) => field.issue?.kind === "limit" && field.issue.message.includes("Related context"),
    ),
  );
});

test("LZ77 rejects oversized output before reading or allocating it", () => {
  const reader = new BinaryReader([], "compressed");
  assert.throws(
    () => decompressLz77(reader, parserLimits.derivedBytes + 1),
    (error) => error.kind === "limit",
  );
  assert.equal(reader.offset, 0);
});

test("deep restrictions stop at the nesting safeguard and unwind their scopes", () => {
  const nestedRestrictions = Array(parserLimits.depth + 1).fill(2); // NOT restriction.
  const record = new RecordReader(
    new BinaryReader([...nestedRestrictions, 8, ...u32(0)], "restriction"),
    "Restriction",
  );
  assert.throws(
    () => readRestriction(record),
    (error) => error.kind === "limit" && error.message.includes("nesting"),
  );
  // A failed nested parse must not strand the shared reader at its maximum depth.
  record.structure("Recovery", () => {});
});

test("RPC source exhaustion refers to the enclosing encoded payload", () => {
  const payload = rpcPayload(responseRop(1));
  const xorFlag = 2;
  const lastBufferFlag = 4;
  const xorMask = 0xa5;
  const buffers = Array.from({ length: parserLimits.derivedSources + 1 }, (_, index) => [
    ...u16(0),
    ...u16(xorFlag | (index === parserLimits.derivedSources ? lastBufferFlag : 0)),
    ...u16(payload.length),
    ...u16(payload.length),
    ...payload.map((byte) => byte ^ xorMask),
  ]).flat();
  const message = selected(1, 1);
  message.response.bytes = execute(buffers, true);
  const result = decode([message], 1, "response");
  const issue = fields(result.root).find((field) => field.type === "Parse issue");
  assert.equal(issue?.issue?.kind, "limit");
  const executeResponsePrefixBytes = 18;
  const rpcHeaderBytes = 8;
  assert.deepEqual(issue.range, {
    sourceId: "selected-response",
    offset:
      executeResponsePrefixBytes +
      parserLimits.derivedSources * (rpcHeaderBytes + payload.length) +
      rpcHeaderBytes,
    length: payload.length,
  });
});
