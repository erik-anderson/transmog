import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const output = await build({
  entryPoints: [
    fileURLToPath(new URL("../src/extension-settings/presentation.ts", import.meta.url)),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { contributionRows, matchingMessages } = await import(
  "data:text/javascript;base64," + Buffer.from(output.outputFiles[0].text).toString("base64")
);
const manifest = async (name) =>
  JSON.parse(
    await readFile(new URL(`../../../../extensions/${name}/manifest.json`, import.meta.url)),
  );
const binary = await manifest("binary");
const mapi = await manifest("mapi");

test("reordering a direction pair preserves its displayed identity and declarations", () => {
  const [request, response] = structuredClone(binary.visualizers.slice(0, 2));
  const other = structuredClone(binary.visualizers[2]);
  const before = contributionRows([request, other, response]);
  const after = contributionRows([response, other, request]);
  assert.deepEqual(after, before);
});

test("bundled direction pairs share an entry without changing their declarations", () => {
  const original = JSON.stringify([binary, mapi]);
  const rows = contributionRows(binary.visualizers);
  assert.equal(rows.length, 5);
  assert.deepEqual(
    rows.map((row) => row.name),
    ["Protobuf", "Bond Compact Binary", "MessagePack", "CBOR", "BSON"],
  );
  assert.deepEqual(rows[0].targets, [
    { id: "protobuf-request", direction: "Request" },
    { id: "protobuf-response", direction: "Response" },
  ]);
  assert.equal(rows[0].inputs[0].value, "Request viewer: content-type, content-encoding");
  assert.equal(rows[0].inputs[1].value, "Request viewer: Required · HTTP decoding on demand");
  assert.match(rows[0].matches[0], /^Request and response · application\/protobuf/u);
  const mapiRows = contributionRows(mapi.visualizers);
  assert.equal(mapiRows.length, 2);
  assert.equal(mapiRows[0].matches.length, 23);
  assert.match(
    mapiRows[0].matchingSummary,
    /Request and response · application\/mapi-http · 23 matching rules/u,
  );
  assert.match(mapiRows[0].access, /Selected body required · Paired body optional/u);
  assert.match(mapiRows[0].access, /Up to 64 related exchanges/u);
  assert.equal(
    mapiRows[0].inputs[0].value,
    "content-type, x-requesttype, x-clientinfo, content-encoding",
  );
  assert.equal(
    mapiRows[0].inputs[1].value,
    [
      "Request viewer: Required · HTTP decoding on demand",
      "Response viewer: Optional · HTTP decoding on demand",
    ].join("\n"),
  );
  assert.equal(mapiRows[0].inputMatches.length, 1);
  assert.equal(mapiRows[1].targets[0].id, "headers");
  assert.equal(JSON.stringify([binary, mapi]), original);
});

test("empty opposite-message inputs are omitted while paired reads remain explicit", () => {
  const pair = structuredClone(binary.visualizers.slice(0, 2));
  pair[0].inputs.response.headers = ["x-context"];
  pair[0].inputs.response.body = "optional";
  const row = contributionRows(pair)[0];
  assert.equal(
    row.inputs[2].value,
    "Request viewer: x-context\nResponse viewer: content-type, content-encoding",
  );
  assert.equal(
    row.inputs[3].value,
    "Request viewer: Optional\nResponse viewer: Required · HTTP decoding on demand",
  );
  const single = contributionRows([binary.visualizers[0]])[0];
  assert.deepEqual(
    single.inputs.map((input) => input.name),
    ["Request headers", "Request body"],
  );
});

test("merging compares complete structured conditions and keeps header direction and value exact", () => {
  const request = {
    direction: "request",
    contentTypes: ["application/example", "application/other"],
    headers: [{ direction: "request", name: "x-operation", test: { op: "equals", value: "Read" } }],
  };
  const response = { ...structuredClone(request), direction: "response" };
  response.contentTypes.reverse();
  assert.equal(matchingMessages([request, response]).length, 1);
  assert.match(matchingMessages([request, response])[0], /^Request and response/u);
  for (const change of [
    (rule) => {
      rule.headers[0].direction = "response";
    },
    (rule) => {
      rule.headers[0].test.value = "read";
    },
    (rule) => {
      rule.headers[0].test.op = "not-equals";
    },
    (rule) => {
      rule.statuses = [200];
    },
    (rule) => {
      rule.methods = ["POST"];
    },
    (rule) => {
      rule.pathPrefixes = ["/rpc"];
    },
    (rule) => {
      rule.contentTypes = ["application/example"];
    },
  ]) {
    const changed = structuredClone(response);
    change(changed);
    assert.equal(matchingMessages([request, changed]).length, 2);
  }
  // Identical display text from different predicate fields is not equivalence.
  assert.deepEqual(
    matchingMessages([
      { direction: "request", methods: ["POST"] },
      { direction: "response", pathPrefixes: ["POST"] },
    ]),
    ["Request · POST", "Response · POST"],
  );
});

test("partial overlaps merge only the common rules", () => {
  const rule = { direction: "request", contentTypes: ["application/example"] };
  assert.deepEqual(
    matchingMessages([
      rule,
      { ...rule, direction: "response" },
      { ...rule, direction: "response", statuses: [500] },
    ]),
    ["Request and response · application/example", "Response · application/example · 500"],
  );
});

test("grouping is conservative about identity, behavior, and ambiguous pairs", () => {
  const pair = structuredClone(binary.visualizers.slice(0, 2));
  pair[0].id = "incoming";
  pair[1].id = "outgoing";
  assert.equal(contributionRows(pair).length, 1, "Pairing must not depend on ID suffixes");
  for (const change of [
    (value) => {
      value.name = "Another viewer";
    },
    (value) => {
      value.entry = "other.js";
    },
    (value) => {
      value.surfaces = ["headers"];
    },
    (value) => {
      value.automatic = false;
    },
    (value) => {
      value.liveUpdates = { events: ["request"], bodyProgress: false };
    },
    (value) => {
      value.context = { headers: [], maxExchanges: 1 };
    },
    (value) => {
      value.inputMatches = [{ direction: "request", methods: ["GET"] }];
    },
    (value) => {
      value.limits = { inputBytes: 1 };
    },
    (value) => {
      value.matches = [{ direction: "request", contentTypes: ["application/protobuf"] }];
    },
  ]) {
    const changed = structuredClone(pair);
    change(changed[1]);
    assert.equal(contributionRows(changed).length, 2);
  }
  assert.equal(contributionRows([pair[0]]).length, 1);
  assert.equal(contributionRows([...pair, { ...pair[1], id: "another" }]).length, 3);
  const differentRules = structuredClone(pair);
  differentRules[1].matches[0].methods = ["POST"];
  assert.equal(contributionRows(differentRules).length, 1);
  assert.equal(contributionRows(differentRules)[0].matches.length, 2);
});
