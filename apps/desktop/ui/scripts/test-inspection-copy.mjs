import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
const output = await build({
  entryPoints: [
    new URL("../src/inspection-copy.ts", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { copyScalar, copyTree, boundedCopy, fieldCopyActions, COPY_TEXT_BYTES } = await import(
  "data:text/javascript;base64," + Buffer.from(output.outputFiles[0].text).toString("base64")
);
test("clipboard scalar serialization preserves empty, Unicode, wide integers and escaped controls", () => {
  assert.equal(copyScalar("hello 🌍"), "hello 🌍");
  assert.equal(copyScalar("a\n\t\\n\r\0"), "a\\n\\t\\\\n\\r\\u0000");
  assert.equal(copyScalar("9007199254740993"), "9007199254740993");
  assert.equal(copyScalar("a\ud800b\udc00c😀"), "a\\ud800b\\udc00c😀");
});
test("subtrees keep duplicate names, order, empty values and two-space nesting", () => {
  assert.equal(
    copyTree({
      name: "Root",
      children: [
        { name: "A", value: "" },
        { name: "A", children: [{ name: "Bytes", value: "00 0A FF" }] },
      ],
    }),
    "Root\n  A: \n  A\n    Bytes: 00 0A FF",
  );
});
test("copy fails as a whole on output and recursive limits", () => {
  assert.throws(() => boundedCopy("é".repeat(COPY_TEXT_BYTES / 2 + 1)), /limit/);
  assert.throws(
    () =>
      copyTree({ name: "root", children: Array.from({ length: 10000 }, () => ({ name: "x" })) }),
    /node limit/,
  );
  let root = { name: "leaf" };
  for (let i = 0; i < 34; i++) root = { name: "parent", children: [root] };
  assert.throws(() => copyTree(root), /depth/);
});
test("field actions read the real value on demand and disable unavailable values", async () => {
  let reads = 0;
  const actions = fieldCopyActions("Name", () => {
    reads++;
    return "full\nvalue";
  });
  assert.equal(reads, 0);
  assert.equal(await actions[0].read(() => true), "Name");
  assert.equal(reads, 0);
  assert.equal(await actions[2].read(() => true), "Name: full\\nvalue");
  assert.equal(reads, 1);
  assert.deepEqual(
    fieldCopyActions("Redacted field", null).map((action) => !!action.disabled),
    [false, true, true],
  );
});

test("subtree copy rejects eviction loops and allows progressive children", async () => {
  const bundled = await build({
    entryPoints: [
      new URL("../src/extension-visualizer/copy.ts", import.meta.url).pathname.replace(
        /^\/([A-Za-z]:)/,
        "$1",
      ),
    ],
    bundle: true,
    platform: "node",
    format: "esm",
    write: false,
    plugins: [
      {
        name: "copy-host",
        setup(builder) {
          builder.onResolve({ filter: /^@tauri-apps\/api\/core$/ }, () => ({
            path: "host",
            namespace: "copy-host",
          }));
          builder.onLoad({ filter: /.*/, namespace: "copy-host" }, () => ({
            contents: "export const invoke=(...args)=>globalThis.__copyRangeHost(...args)",
            loader: "js",
          }));
        },
      },
    ],
  });
  const { visualizerCopyTree } = await import(
    "data:text/javascript;base64," + Buffer.from(bundled.outputFiles[0].text).toString("base64")
  );
  const range = (id, loaded = false) => ({
    id,
    name: id,
    spans: [],
    children: loaded
      ? [
          {
            id: id + "-value",
            name: "Value",
            value: { kind: "integer", value: "7" },
            spans: [],
            children: [],
          },
        ]
      : [],
    ...(loaded ? {} : { childrenToken: id }),
  });
  const view = (loaded) => ({
    handle: {},
    document: {
      title: "Paged body",
      roots: [
        {
          id: "root",
          name: "Root",
          spans: [],
          children: [range("A", loaded === "A"), range("B", loaded === "B")],
        },
      ],
      sources: [],
    },
  });
  let expansions = 0,
    latest;
  globalThis.__copyRangeHost = async (command, args) => {
    assert.equal(command, "expand_visualizer");
    expansions++;
    return view(args.token);
  };
  try {
    await assert.rejects(
      visualizerCopyTree(
        "root",
        view(),
        () => true,
        (result) => {
          latest = result;
        },
      ),
      /loads in ranges.*smaller field range/,
    );
    assert.equal(expansions, 2);
    assert.equal(latest.document.roots[0].children[1].children[0].value.value, "7");
    assert.match(
      await visualizerCopyTree(
        "A",
        view(),
        () => true,
        () => {},
      ),
      /Value: 7/,
    );
    let pages = 0;
    const progressive = (count) => {
      const result = view();
      result.document.roots[0].children = Array.from({ length: count }, (_, index) => ({
        id: `child-${index}`,
        name: `Child ${index}`,
        value: { kind: "integer", value: String(index) },
        spans: [],
        children: [],
      }));
      result.document.roots[0].childrenToken = count < 2 ? "more" : null;
      return result;
    };
    globalThis.__copyRangeHost = async (command, args) => {
      assert.equal(command, "expand_visualizer");
      assert.equal(args.token, "more");
      return progressive(++pages);
    };
    assert.equal(
      await visualizerCopyTree(
        "root",
        progressive(0),
        () => true,
        () => {},
      ),
      "Root\n  Child 0: 0\n  Child 1: 1",
    );
    assert.equal(pages, 2);
  } finally {
    delete globalThis.__copyRangeHost;
  }
});
