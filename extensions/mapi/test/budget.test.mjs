import assert from "node:assert/strict";
import { test } from "node:test";
import { decode } from "../dist/decode.js";
import { enums } from "../dist/constants.js";
import { present } from "../dist/view.js";
import { ParseBudget } from "../dist/model.js";
import { BinaryReader } from "../dist/reader.js";
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
  return [opcode, 0, ...u32(0), ...u16(1), ...u32(0x0037001f),
    ...Buffer.from("A".repeat(characters) + "\0", "utf16le")];
}

function selected(id = 1, characters = 7600) {
  return {
    id, requestType:"Execute",
    request:{sourceId:"selected-request", bytes:execute(plainRpc(requestRop), false)},
    response:{sourceId:"selected-response", bytes:execute(plainRpc(responseRop(characters)), true)},
  };
}

function history(count, length) {
  return Array.from({length:count}, (_, index) => ({
    id:index + 1, requestType:"Connect",
    request:{sourceId:"context-" + index, bytes:[...Array(length).fill(65), 0, ...Array(20).fill(0)]},
  }));
}

test("a 14.9 KiB Execute response does not inherit reader work spent replaying earlier messages", () => {
  const message = selected(33);
  assert.ok(message.response.bytes.length > 14.8 * 1024 && message.response.bytes.length < 15 * 1024);
  const result = decode([...history(32, 4096), message], message.id, "response");
  assert.ok(fields(result.root).every((field) => !field.issue));
  assert.ok(fields(result.root).some((field) => field.value?.value === "A".repeat(7600)));
  const report = result.budgets.find((budget) => budget.phase === "Selected response");
  assert.equal(report.inputBytes, message.response.bytes.length);
  assert.ok(report.work < 100000);
  const view = present(result, [["selected-response", message.response.bytes.length]], "Execute", []).step;
  assert.equal(view.document.status, "complete", "Informational counters changed parse status");
  const summaries = view.document.diagnostics.filter((item) => item.code === "mapi-parser-budget");
  assert.equal(summaries.length, 3);
  assert.ok(summaries.every((item) => item.message.length < 2048 && !item.message.includes("AAAAAA")));
});

test("a small compressed Execute can legitimately require more than 100,000 reader operations", () => {
  const characters = 30000;
  const payload = rpcPayload(responseRop(characters));
  const prefix = payload.slice(0, payload.length - characters * 2 - 6);
  prefix.push(65, 0);
  const length = characters * 2 - 2;
  // Independently encode literal prefix, one distance-two overlapping match,
  // UTF-16 terminator and handle table. The match uses all length extensions.
  const compressed = [...u32(2 ** (31 - prefix.length)), ...prefix, 15, 0, 15, 255,
    ...u16(length - 3), 0, 0, ...u32(0x12345678)];
  const buffers = Array.from({length:4}, (_, index) => [
    ...u16(0), ...u16(index === 3 ? 5 : 1), ...u16(compressed.length), ...u16(payload.length), ...compressed,
  ]).flat();
  const message = {
    id:1, requestType:"Execute",
    request:{sourceId:"request", bytes:execute(plainRpc(Array.from({length:4}, () => requestRop).flat()), false)},
    response:{sourceId:"response", bytes:execute(buffers, true)},
  };
  assert.ok(message.response.bytes.length < 15 * 1024);
  const result = decode([message], 1, "response");
  assert.ok(fields(result.root).every((field) => !field.issue), JSON.stringify(fields(result.root).filter((field) => field.issue).map((field) => field.issue)));
  assert.equal(result.derivedSources.length, 4);
  const report = result.budgets.find((budget) => budget.phase === "Selected response");
  assert.equal(report.expandedBytes, payload.length * 4);
  assert.equal(report.encodedBytes, compressed.length * 4);
  assert.ok(report.work > 100000 && report.work < report.maximumWork);
  assert.equal(fields(result.root).filter((field) => field.value?.value === "A".repeat(characters)).length, 4);
});

test("bounded history exhaustion retains selected fields and identifies the exhausted phase", () => {
  const message = selected(65, 50);
  const result = decode([...history(64, 20000), message], message.id, "response");
  assert.ok(fields(result.root).some((field) => field.value?.value === "A".repeat(50)));
  assert.ok(fields(result.root).some((field) => field.issue?.kind === "limit" && field.issue.message.includes("Related context")));
});

test("LZ77 output copying consumes the finite work budget before performing the copy", () => {
  const budget = new ParseBudget(100, 48, 20);
  // Three literals followed by an overlapping match require 30 output bytes.
  const bytes = [0, 0, 0, 16, 65, 66, 67, 23, 0, 10];
  assert.throws(() => decompressLz77(new BinaryReader(bytes, "compressed", budget), 30),
    (error) => error.kind === "limit" && error.message.includes("20/20"));
  assert.equal(budget.report().work, 20);
});
