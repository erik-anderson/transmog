import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
const output = await build({
  entryPoints: [fileURLToPath(new URL("../src/traffic-filters.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { parseFilterValues, trafficFilterChips } = await import(
  "data:text/javascript;base64," + Buffer.from(output.outputFiles[0].text).toString("base64")
);
test("Host batches canonicalize the whole paste and preserve wildcard identity", () => {
  assert.deepEqual(
    parseFilterValues(
      "host",
      "API.EXAMPLE.TEST., api.example.test\n*.BÜCHER.example, 127.0.0.1,2001:db8::1,,",
    ),
    [
      { kind: "text", value: "api.example.test" },
      { kind: "host-subdomains", value: "xn--bcher-kva.example" },
      { kind: "text", value: "127.0.0.1" },
      { kind: "text", value: "[2001:db8::1]" },
    ],
  );
  for (const value of [
    "bad:80",
    "bad:443",
    "[::1]:80",
    "https://a.test",
    "a.test/x",
    "a@b.test",
    "*",
    "a.*.test",
    "*.127.0.0.1",
    "*.[::1]",
    "a..test",
    "a.test..",
    "a%2etest",
  ])
    assert.throws(() => parseFilterValues("host", "ok.test," + value), undefined, value);
});
test("typed choices and literal commas keep their original identities", () => {
  assert.deepEqual(parseFilterValues("protocol", "http/2.0, HTTP/2"), [
    { kind: "text", value: "http/2" },
  ]);
  assert.deepEqual(parseFilterValues("status", "2xx, 0200, Other, Unavailable status"), [
    { kind: "status-family", value: 2 },
    { kind: "number", value: "200" },
    { kind: "other" },
    { kind: "unavailable" },
  ]);
  assert.deepEqual(parseFilterValues("process", "one,two.exe"), [
    { kind: "text", value: "one,two.exe" },
  ]);
  assert.deepEqual(parseFilterValues("content-type", "Text/Plain; charset=utf-8"), [
    { kind: "text", value: "text/plain" },
  ]);
  assert.deepEqual(parseFilterValues("host", " ,\n,"), []);
  assert.throws(() => parseFilterValues("method", "*.test"));
});
test("chips describe the effective role combination and omit paused saved-only membership", () => {
  const group = {
    dimension: "host",
    values: [],
    savedValues: [{ kind: "text", value: "a.test" }],
    selectionMode: "ignore-saved",
  };
  assert.deepEqual(trafficFilterChips([], [group], []), []);
  group.selectionMode = "both";
  group.values.push({ kind: "text", value: "b.test" });
  assert.match(trafficFilterChips([], [group], [])[0].label, /1 saved and 1 picked/);
});
