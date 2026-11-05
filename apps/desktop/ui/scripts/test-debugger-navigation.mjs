import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("../src/debugger-workspace/navigation.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { matchingFiles, searchSources } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const file = {
  id: "source",
  artifact: "revision",
  package: "sample",
  path: "main.ts",
  source: "file:///transmog-scripts/sample/main.ts",
  state: "Unloaded",
};

test("quick open resolves file positions and current-file positions", () => {
  for (const [query, line, column] of [
    ["main.ts:12", 12, 1],
    ["main:12:9", 12, 9],
    [":3:4", 3, 4],
    ["file:///transmog-scripts/sample/main.ts:2:9", 2, 9],
  ]) {
    const [result] = matchingFiles([file], query, file);
    assert.equal(result.file, file);
    assert.equal(result.line, line);
    assert.equal(result.column, column);
    assert.equal(result.length, 0);
  }
  assert.equal(matchingFiles([file], "main.ts")[0].line, 0);
  assert.deepEqual(matchingFiles([file], ":3"), []);
  for (const query of ["main.ts:0", "main.ts:2:0", "main.ts:9999999999999999999999"]) {
    assert.deepEqual(matchingFiles([file], query), []);
  }
});

test("search preserves original UTF-16 positions and finds every occurrence on a line", async () => {
  const text = "😀 value + VALUE\nvalue";
  const found = await searchSources(
    [file],
    "value",
    async () => text,
    () => true,
  );
  assert.equal(found.limited, false);
  assert.deepEqual(
    found.results.map(({ line, column, length }) => ({ line, column, length })),
    [
      { line: 1, column: 4, length: 5 },
      { line: 1, column: 12, length: 5 },
      { line: 2, column: 1, length: 5 },
    ],
  );
  assert.equal(new Set(found.results.map((result) => result.key)).size, 3);
  const literal = await searchSources(
    [file],
    "[a].*",
    async () => "[a].* [a]abc",
    () => true,
  );
  assert.equal(literal.results.length, 1, "Search text must not become a regular expression");
});

test("the match limit is disclosed only when a match is omitted", async () => {
  const find = (count) =>
    searchSources(
      [file],
      "hit",
      async () => "hit ".repeat(count),
      () => true,
    );
  assert.equal((await find(100)).limited, false);
  const overflow = await find(101);
  assert.equal(overflow.limited, true);
  assert.equal(overflow.results.length, 100);
});
