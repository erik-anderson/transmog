import assert from "node:assert/strict";
import test from "node:test";
import * as api from "../src/api.mjs";

test("UTF-8 and base64 preserve bytes and reject ambiguous or invalid encodings", () => {
  for (const text of ["", "héllo 🌍", "\u0000\ufeff", "\ud800x\udfff"]) {
    const expected = Array.from(new TextEncoder().encode(text));
    assert.deepEqual(api.utf8Encode(text), expected);
    assert.equal(
      api.utf8Decode(expected),
      new TextDecoder("utf-8", { ignoreBOM: true }).decode(Uint8Array.from(expected)),
    );
    assert.deepEqual(api.base64Decode(api.base64Encode(expected)), expected);
  }
  for (const bytes of [
    [0xc0, 0x80],
    [0xed, 0xa0, 0x80],
    [0xf4, 0x90, 0x80, 0x80],
    [0xe2, 0x82],
    [0xe2, 65],
  ]) {
    assert.throws(() => api.utf8Decode(bytes));
    assert.equal(
      api.utf8Decode(bytes, { fatal: false }),
      new TextDecoder().decode(Uint8Array.from(bytes)),
    );
  }
  for (const encoded of ["AA", "AB==", "AAB=", "AA==\n", "===="])
    assert.throws(() => api.base64Decode(encoded));
  const bytes = Array.from({ length: 256 }, (_, i) => i);
  assert.equal(api.base64Encode(bytes), Buffer.from(bytes).toString("base64"));
});

test("headers and queries retain duplicates, order and withheld-value identity", () => {
  const headers = [
    { name: "Set-Cookie", value: [65], redacted: false },
    { name: "set-cookie", value: [66], redacted: false },
  ];
  assert.deepEqual(api.headerText(headers, "SET-COOKIE"), ["A", "B"]);
  assert.throws(() => api.headerText([{ ...headers[0], redacted: true }], "set-cookie"));
  assert.deepEqual(api.parseQuery("a=1&a=2&b=a+b&empty="), [
    ["a", "1"],
    ["a", "2"],
    ["b", "a b"],
    ["empty", ""],
  ]);
  assert.equal(api.setQuery("x=1&a=2&a=3&y=4", "a", "hello 🌍"), "x=1&a=hello+%F0%9F%8C%8D&y=4");
  assert.throws(() => api.parseQuery("bad=%FF"));
  assert.equal(
    api.requestUrl({ scheme: "https", host: "::1", port: 443, path: "/x", query: "" }),
    "https://[::1]/x?",
  );
});

test("body helpers make content type and bytes one edit without mutating input", () => {
  const result = api.replaceJson({ enabled: true });
  assert.equal(api.readText({ bytes: result.decision.edits.bytes }), '{"enabled":true}');
  assert.deepEqual(result.decision.edits.headers, [
    api.setHeader("content-type", "application/json; charset=utf-8"),
  ]);
  const response = api.respondJson(201, { ok: true }, [
    ["Content-Type", "text/plain"],
    ["X-Test", "yes"],
  ]);
  assert.deepEqual(response.decision.response.headers, [
    ["content-type", "application/json; charset=utf-8"],
    ["X-Test", "yes"],
  ]);
  const header = (value) => [
    { name: "content-type", value: api.utf8Encode(value), redacted: false },
  ];
  assert.throws(() => api.readText({ bytes: [65] }, header("text/plain; charset=latin1")));
  assert.throws(() =>
    api.readText({ bytes: [65] }, header("text/plain; charset=utf-8; charset=utf-8")),
  );
  assert.deepEqual(api.pass({ state: { count: 1 }, diagnostics: ["observed"] }), {
    decision: { action: "continue" },
    state: { count: 1 },
    diagnostics: ["observed"],
  });
});
