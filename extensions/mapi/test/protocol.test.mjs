import assert from "node:assert/strict";
import { test } from "node:test";
import { decode } from "../dist/decode.js";
import { BinaryReader } from "../dist/reader.js";
import { decompressLz77 } from "../dist/rpc.js";

function uint32(value) {
  return [value & 255, (value >>> 8) & 255, (value >>> 16) & 255, value >>> 24];
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
