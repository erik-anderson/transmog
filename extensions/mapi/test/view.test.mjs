import assert from "node:assert/strict";
import { test } from "node:test";
import { present } from "../dist/view.js";

test("only a complete interpretation of final input opts out of future updates", () => {
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
  };
  const decoded = { root, derivedSources: [] };
  assert.equal(present(decoded, [["body", 1]], "Packet", []).step.document.updates, undefined);
  const final = present(decoded, [["body", 1]], "Packet", [], undefined, true);
  assert.equal(final.step.document.updates, "final");
  const partial = present(
    decoded,
    [["body", 1]],
    "Packet",
    [{ code: "missing-context", message: "Earlier data is missing." }],
    undefined,
    true,
  );
  assert.equal(partial.step.document.updates, undefined);
});

test("expanded output stays within the native byte budget, including escaped text", () => {
  const text = "\x01".repeat(4096);
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "original", offset: 0, length: 2 * 1024 * 1024 },
    children: Array.from({ length: 11_000 }, (_, index) => ({
      name: `Field ${index}`,
      type: "string",
      range: { sourceId: "original", offset: 0, length: 1 },
      value: { kind: "string", value: text },
      children: [],
    })),
  };
  const view = present(
    {
      root,
      derivedSources: [
        {
          id: "derived",
          bytes: Array(2 * 1024 * 1024).fill(0),
          parents: [root.range],
          transform: "Synthetic transform",
          exactMapping: false,
        },
      ],
    },
    [["original", root.range.length]],
    "Output budget",
    [],
  );
  const token = view.step.document.roots[0].childrenToken;
  const result = view.expand(token);
  assert.equal(result.document.status, "limited");
  assert.ok(Buffer.byteLength(JSON.stringify(result), "utf8") <= 7 * 1024 * 1024);
  assert.ok(result.document.diagnostics.some((item) => item.code === "display-limit"));
});
