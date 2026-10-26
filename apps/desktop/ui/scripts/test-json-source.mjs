import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
const bundled = await build({
  entryPoints: [
    new URL("../src/body-source-viewer/json-source.ts", import.meta.url).pathname.replace(
      /^\/([A-Za-z]:)/,
      "$1",
    ),
    new URL("../src/extension-visualizer/models.ts", import.meta.url).pathname.replace(
      /^\/([A-Za-z]:)/,
      "$1",
    ),
  ],
  outdir: "json-source-test",
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { parseJsonSource, formatJsonSource, jsonCopyActions, jsonNodeAt, jsonSnapshot } =
  await import(
    "data:text/javascript;base64," + Buffer.from(bundled.outputFiles[0].text).toString("base64")
  );
const { treeRows, tablePage } = await import(
  "data:text/javascript;base64," + Buffer.from(bundled.outputFiles[1].text).toString("base64")
);
const read = async (document, node, label) =>
  jsonCopyActions(document, node)
    .find((action) => action.label === label)
    .read(() => true);

test("lossless formatting preserves duplicate key order and original numeric and string tokens", async () => {
  const source =
    ')]}\'\n{"x":9007199254740993,"x":1e+02,"negative":-0,"escaped":"\\u0061\\n","nested":{"x":true}}';
  const document = parseJsonSource(source),
    root = document.root;
  assert.deepEqual(
    root.children.map((node) => node.name),
    ["x", "x", "negative", "escaped", "nested"],
  );
  assert.equal(await read(document, root.children[0], "Copy value"), "9007199254740993");
  assert.equal(await read(document, root.children[1], "Copy as JSON"), "1e+02");
  assert.equal(await read(document, root.children[2], "Copy value"), "-0");
  assert.equal(await read(document, root.children[3], "Copy value"), "a\\n");
  assert.equal(await read(document, root.children[3], "Copy as JSON"), '"\\u0061\\n"');
  const pretty = formatJsonSource(document);
  assert.match(pretty, /"x": 9007199254740993,\n  "x": 1e\+02,\n  "negative": -0/);
  assert.match(pretty, /"escaped": "\\u0061\\n"/);
  assert.equal(formatJsonSource(parseJsonSource(pretty)), pretty);
  assert.equal(document.prefixLength, 5);
  assert.equal(parseJsonSource(")]}',\n{}").prefixLength, 6);
});

test("JSON pointers escape keys and suppress ambiguous duplicate paths and their descendants", () => {
  const document = parseJsonSource(
    '{"a/b~":{"": [null, false]},"dup":{"child":1},"dup":{"child":2}}',
  );
  assert.equal(document.root.pointer, "");
  assert.equal(document.root.children[0].children[0].children[1].pointer, "/a~1b~0//1");
  for (const node of document.root.children.slice(1)) {
    assert.equal(node.pointer, null);
    assert.equal(node.children[0].pointer, null);
    assert.equal(
      jsonCopyActions(document, node).some((action) => action.label === "Copy JSON Pointer"),
      false,
    );
  }
});

test("lone surrogates are escaped in scalar copies and unsafe pointer keys are omitted", async () => {
  const document = parseJsonSource(
    '{"\\ud800":{"value":"a\\udc00b"},"\\u0000":1,"\\n":2,"😀":"😀"}',
  );
  assert.equal(await read(document, document.root.children[0], "Copy name"), "\\ud800");
  assert.equal(
    await read(document, document.root.children[0].children[0], "Copy value"),
    "a\\udc00b",
  );
  assert.equal(
    await read(document, document.root.children[0].children[0], "Copy as JSON"),
    '"a\\udc00b"',
  );
  for (const child of document.root.children.slice(0, 3)) assert.equal(child.pointer, null);
  assert.equal(document.root.children[0].children[0].pointer, null);
  assert.equal(document.root.children[3].pointer, "/😀");
});

test("readable copies retain escaped scalar conventions while JSON copies remain valid literals", async () => {
  const document = parseJsonSource(
    '{"empty":"","line\\n":"a\\nb\\t\\\\","items":[null,false,{},[]]}',
  );
  assert.equal(
    await read(document, document.root.children[1], "Copy name and value"),
    "line\\n: a\\nb\\t\\\\",
  );
  assert.equal(
    await read(document, document.root, "Copy subtree"),
    "JSON\n  empty: \n  line\\n: a\\nb\\t\\\\\n  items\n    [0]: null\n    [1]: false\n    [2]\n    [3]",
  );
  assert.deepEqual(JSON.parse(await read(document, document.root.children[2], "Copy as JSON")), [
    null,
    false,
    {},
    [],
  ]);
  const snapshot = jsonSnapshot(document, "snapshot", () => undefined);
  assert.equal(snapshot.document.roots[0].value.count, 3);
  assert.equal(
    await snapshot
      .copyActions("json-1")
      .find((action) => action.label === "Copy value")
      .read(() => true),
    "",
  );
  assert.equal(snapshot.document.roots[0].children[2].children[0].value.kind, "null");
});

test("source ranges count UTF-16 code units and remain reliable across whitespace normalization", () => {
  const source = '{\r\n "😀": "hello",\n "next": -1.00E+3\r\n}';
  const document = parseJsonSource(source),
    first = document.root.children[0],
    last = document.root.children[1];
  assert.equal(source.slice(first.start, first.end), '"hello"');
  assert.equal(jsonNodeAt(document, source.indexOf("😀")).id, first.id);
  assert.equal(jsonNodeAt(document, source.indexOf("-1.00")).id, last.id);
  assert.equal(jsonNodeAt(document, source.length - 1).id, document.root.id);
  const normalized = parseJsonSource(source.replaceAll("\r\n", "\n"));
  assert.equal(normalized.nodes.get(last.id).scalar, last.scalar);
  assert.equal(
    normalized.source.slice(normalized.nodes.get(last.id).start, normalized.nodes.get(last.id).end),
    "-1.00E+3",
  );
});

test("empty and whitespace JSON labels are display-only, with exact identity and copy data", async () => {
  const source =
      '{"":""," \\t":" \\t","\\n":"line\\nvalue","\\\"\\\" (empty name)":"\\\"\\\" (empty string)"}',
    document = parseJsonSource(source),
    snapshot = jsonSnapshot(document, "blank-labels", () => undefined),
    nodes = snapshot.document.roots[0].children;
  assert.equal(nodes[0].name, "");
  assert.deepEqual(nodes[0].value, { kind: "string", value: "" });
  assert.deepEqual(snapshot.nodePresentation(nodes[0].id), {
    name: '"" (empty name)',
    value: '"" (empty string)',
  });
  assert.match(snapshot.nodePresentation(nodes[1].id).name, /\\u0020\\t.*whitespace name/);
  assert.match(snapshot.nodePresentation(nodes[1].id).value, /whitespace string/);
  assert.equal(snapshot.nodePresentation(nodes[2].id).name, '"\\n" (whitespace name, 1 character)');
  assert.equal(snapshot.nodePresentation(nodes[2].id).value, '"line\\nvalue"');
  assert.equal(snapshot.nodePresentation(nodes[3].id).name, JSON.stringify(nodes[3].name));
  assert.equal(snapshot.nodePresentation(nodes[3].id).value, JSON.stringify(nodes[3].value.value));
  const readSnapshot = (id, label) =>
    snapshot
      .copyActions(id)
      .find((action) => action.label === label)
      .read(() => true);
  assert.equal(await readSnapshot(nodes[0].id, "Copy name"), "");
  assert.equal(await readSnapshot(nodes[0].id, "Copy value"), "");
  assert.equal(await readSnapshot(nodes[0].id, "Copy name and value"), ": ");
  assert.equal(await readSnapshot(nodes[0].id, "Copy as JSON"), '""');
  assert.equal(await readSnapshot(nodes[0].id, "Copy JSON Pointer"), "/");
  assert.equal(await readSnapshot(nodes[1].id, "Copy name and value"), " \\t:  \\t");
  assert.equal(await readSnapshot(nodes[3].id, "Copy value"), '"" (empty string)');
  assert.equal(await readSnapshot(document.root.id, "Copy as JSON"), source);
  const rendered = treeRows(
    snapshot.document.roots[0],
    new Set([document.root.id]),
    "",
    "test",
    snapshot.nodePresentation,
  );
  assert.equal(rendered[1].name, '"" (empty name)');
  assert.equal(rendered[1].value, '"" (empty string)');
  const ordinary = treeRows(snapshot.document.roots[0], new Set([document.root.id]), "", "test");
  assert.equal(ordinary[1].name, "");
  assert.equal(ordinary[1].value, "");
  const tableDocument = parseJsonSource('[{"":""},{},{"\\\"\\\" (empty name)":"literal"}]'),
    tableSnapshot = jsonSnapshot(tableDocument, "table", () => undefined),
    table = tablePage(tableSnapshot.document.roots[0], 0, tableSnapshot.nodePresentation),
    emptyColumn = table.columns.findIndex((column) => column.rawName === "");
  assert.equal(table.columns.filter((column) => column.rawName !== undefined).length, 2);
  assert.equal(table.columns[emptyColumn].name, '"" (empty name)');
  assert.equal(
    table.rows[0].copyCells[emptyColumn].nodeId,
    tableDocument.root.children[0].children[0].id,
  );
  assert.equal(table.rows[0].copyCells[emptyColumn].value, '"" (empty string)');
  assert.equal(table.rows[1].copyCells[emptyColumn].name, "");
  assert.equal(table.rows[1].copyCells[emptyColumn].nodeId, "");
});

test("strict grammar rejects incomplete and malformed JSON instead of inventing fields", () => {
  for (const source of [
    '{"x":',
    '{"x":1,}',
    "[1,]",
    "[01]",
    "[+1]",
    "[.1]",
    "[1e]",
    "[1.]",
    "true false",
    '"\\x00"',
    '"\\u012"',
    '"bad\nstring"',
    "x)]}'\n{}",
  ]) {
    assert.throws(
      () => parseJsonSource(source),
      (error) => error.reason === "invalid",
      source,
    );
  }
  for (const source of ["null", "false", "true", '""', "-0", "1e400", "[]", "{}"])
    assert.equal(formatJsonSource(parseJsonSource(source)), source);
});

test("structured budgets fail as a whole at node/depth/input limits", () => {
  assert.throws(
    () => parseJsonSource("[".repeat(34) + "0" + "]".repeat(34)),
    (error) => error.reason === "limit",
  );
  assert.throws(
    () => parseJsonSource("[" + "0,".repeat(10000) + "0]"),
    (error) => error.reason === "limit",
  );
  assert.throws(
    () => parseJsonSource('"' + "x".repeat(16 * 1024 * 1024) + '"'),
    (error) => error.reason === "limit",
  );
});

test("representative bounded preview sizes preserve token spelling without timing thresholds", (context) => {
  for (const bytes of [256 * 1024, 1024 * 1024, 16 * 1024 * 1024]) {
    const source = '"' + "x".repeat(bytes - 2) + '"',
      start = performance.now();
    const document = parseJsonSource(source);
    assert.equal(formatJsonSource(document), source);
    assert.equal(document.nodes.size, 1);
    context.diagnostic(`${bytes} byte JSON preview: ${(performance.now() - start).toFixed(1)} ms`);
  }
});
