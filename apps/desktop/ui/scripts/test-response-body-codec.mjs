import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";

const output = await build({
  entryPoints: [
    new URL("../src/response-body-codec.ts", import.meta.url).pathname.replace(
      /^\/([A-Za-z]:)/,
      "$1",
    ),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const {
  bytesToHex,
  parseHexBody,
  encodeBody,
  renderBodyBytes,
  changeBodyMode,
  MAX_RESPONSE_BODY_BYTES,
} = await import(
  "data:text/javascript;base64," + Buffer.from(output.outputFiles[0].text).toString("base64")
);

test("binary bodies roundtrip all bytes through editable hex without offsets", () => {
  const bytes = Array.from({ length: 256 }, (_, index) => index),
    hex = bytesToHex(bytes);
  assert.deepEqual(parseHexBody(hex), bytes);
  assert.equal(hex.split("\n").length, 16);
  assert.deepEqual(parseHexBody("00 7F\n80ff"), [0, 127, 128, 255]);
  for (const value of ["0", "00 af f", "00 0x1f", "00000000: ff", "ff |ÿ|"])
    assert.throws(() => parseHexBody(value));
});

test("text and hex preserve Unicode encodings, BOMs, and exact line endings", () => {
  for (const encoding of [
    "utf-8",
    "utf-8-bom",
    "utf-16le",
    "utf-16be",
    "utf-16le-bom",
    "utf-16be-bom",
    "utf-32le",
    "utf-32be",
    "utf-32le-bom",
    "utf-32be-bom",
  ]) {
    const text = "Hi 🌍\r\nNext\n\tline",
      bytes = encodeBody(text, "text", encoding);
    assert.equal(renderBodyBytes(bytes, "text", encoding), text, encoding);
    assert.deepEqual(
      encodeBody(changeBodyMode(text, "text", "hex", encoding), "hex", encoding),
      bytes,
      encoding,
    );
    assert.equal(changeBodyMode(bytesToHex(bytes), "hex", "text", encoding), text, encoding);
  }
});

test("Text refuses invalid, unpresentable, and lossy bytes; Hex still edits them", () => {
  for (const bytes of [[0xff], [0xc0, 0xaf], [0x61, 0], [0xed, 0xa0, 0x80], [0xc2, 0x80]]) {
    assert.throws(() => renderBodyBytes(bytes, "text", "utf-8"), /lossless Unicode/);
    assert.deepEqual(encodeBody(renderBodyBytes(bytes, "hex", null), "hex", null), bytes);
  }
  assert.throws(() => renderBodyBytes([0, 0xd8], "text", "utf-16le"));
  assert.throws(() => renderBodyBytes([0, 0, 0x11, 0], "text", "utf-32le"));
  assert.throws(() => renderBodyBytes([0xc3, 0xa9], "text", "us-ascii"));
  assert.throws(() => encodeBody("\ud800", "text", "utf-8"), /surrogate/);
  assert.throws(() => renderBodyBytes([65, 66], "text", null), /unsupported text encoding/);
  assert.throws(() => encodeBody("é", "text", null), /unsupported text encoding/);
  assert.deepEqual(encodeBody("41 42 e9", "hex", null), [65, 66, 233]);
});

test("the editor limit counts encoded bytes, not UTF-16 characters", () => {
  assert.throws(
    () => encodeBody("é".repeat(MAX_RESPONSE_BODY_BYTES / 2 + 1), "text", "utf-8"),
    /16 MiB/,
  );
});
