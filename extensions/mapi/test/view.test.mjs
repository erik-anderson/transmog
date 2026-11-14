import assert from "node:assert/strict";
import { test } from "node:test";
import { displayLimits } from "../dist/view-limits.js";
import { maximumDisplayStringLength } from "../dist/shared.js";
import { present } from "../dist/view.js";

test("live redraws preserve the expanded section when earlier structure grows", () => {
  const range = { sourceId: "body", offset: 0, length: 1 };
  const field = (name, children = []) => ({ name, type: "Section", range, children });
  const deferredFieldCount = displayLimits.eagerChildren + 1;
  const section = (name) =>
    field(
      name,
      Array.from({ length: deferredFieldCount }, (_, index) => field(`Value ${index}`)),
    );
  const render = (earlier, previous) =>
    present(
      {
        root: field("Packet", [...earlier, section("Keep open"), section("Keep closed")]),
        derivedSources: [],
      },
      [["body", 1]],
      "Packet",
      [],
      previous,
    );
  const children = (view) => view.step.document.roots[0].children;
  const initial = render([field("Earlier")]);
  const opened = children(initial).find((node) => node.name === "Keep open");
  const closed = children(initial).find((node) => node.name === "Keep closed");
  initial.expand(opened.childrenToken);

  const updated = render([field("Earlier", [field("New value")]), field("New section")], initial);
  const retained = children(updated).find((node) => node.name === "Keep open");
  const untouched = children(updated).find((node) => node.name === "Keep closed");
  assert.equal(retained.id, opened.id);
  assert.equal(retained.children.length, deferredFieldCount);
  assert.equal(retained.childrenToken, undefined);
  assert.equal(untouched.id, closed.id);
  assert.equal(untouched.children.length, 0);
  assert.ok(untouched.childrenToken);
  assert.throws(() => updated.expand(closed.childrenToken), /Expired MAPI expansion/);
});

test("repeated sections keep distinct identities and replacements stay collapsed", () => {
  const range = { sourceId: "body", offset: 0, length: 1 };
  const section = (name) => ({
    name,
    type: "Section",
    range,
    children: Array.from({ length: displayLimits.eagerChildren + 1 }, (_, index) => ({
      name: `Value ${index}`,
      type: "Value",
      range,
      children: [],
    })),
  });
  const render = (names, previous) =>
    present(
      {
        root: { name: "Packet", type: "Packet", range, children: names.map(section) },
        derivedSources: [],
      },
      [["body", 1]],
      "Packet",
      [],
      previous,
    );
  const initial = render(["Repeated", "Repeated"]);
  const [first, second] = initial.step.document.roots[0].children;
  assert.notEqual(first.id, second.id);
  initial.expand(first.childrenToken);
  const updated = render(["Replacement", "Repeated", "Repeated"], initial);
  const [replacement, retained, collapsed] = updated.step.document.roots[0].children;
  assert.ok(replacement.childrenToken);
  assert.notEqual(replacement.id, first.id);
  assert.equal(retained.id, first.id);
  assert.equal(retained.childrenToken, undefined);
  assert.equal(collapsed.id, second.id);
  assert.ok(collapsed.childrenToken);
});

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

test("display clipping preserves complete Unicode characters", () => {
  const text = "A".repeat(maximumDisplayStringLength - 1) + "😀";
  const root = {
    name: "Text",
    type: "string",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    value: { kind: "string", value: text },
  };
  const result = present({ root, derivedSources: [] }, [["body", 1]], "Text", []).step.document;
  assert.equal(result.status, "limited");
  assert.equal(result.roots[0].value.value, "A".repeat(maximumDisplayStringLength - 1));
  assert.ok(result.roots[0].value.value.isWellFormed());
});

test("diagnostics fit the host contract and disclose omitted details", () => {
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
  };
  const diagnostics = Array.from({ length: displayLimits.diagnostics + 2 }, (_, index) => ({
    code: "issue-" + index,
    message: "Long diagnostic ".repeat(displayLimits.diagnosticMessage),
  }));
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Packet", diagnostics).step
    .document;
  assert.equal(document.status, "limited");
  assert.ok(document.diagnostics.length <= displayLimits.diagnostics + 1);
  assert.ok(
    document.diagnostics.every((item) => item.message.length <= displayLimits.diagnosticMessage),
  );
  assert.ok(document.diagnostics.some((item) => item.code === "display-limit"));
});

test("combined decoded sources stay within the host source-count limit", () => {
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
  };
  const sources = Array.from({ length: displayLimits.sources + 1 }, (_, index) => ({
    id: "decoded-" + index,
    bytes: [0],
    parents: [root.range],
    transform: "Synthetic transform",
    exactMapping: false,
  }));
  const document = present({ root, derivedSources: sources }, [["body", 1]], "Packet", []).step
    .document;
  assert.equal(document.status, "limited");
  assert.equal(document.sources.length, displayLimits.sources);
});

test("clipped flag names disclose the limit and do not finalize the result", () => {
  const root = {
    name: "Flags",
    type: "Flags",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    value: {
      kind: "enum",
      value: "1",
      flags: true,
      names: Array.from({ length: displayLimits.flagNames + 1 }, (_, index) => `Flag ${index}`),
    },
  };
  const document = present(
    { root, derivedSources: [] },
    [["body", 1]],
    "Flags",
    [],
    undefined,
    true,
  ).step.document;
  assert.equal(document.roots[0].value.value.names.length, displayLimits.flagNames);
  assert.equal(document.status, "limited");
  assert.equal(document.updates, undefined);
  assert.ok(document.diagnostics.some((item) => item.code === "display-limit"));
});

test("omitted short diagnostics explain why the result is limited", () => {
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
  };
  const diagnostics = Array.from({ length: displayLimits.diagnostics + 1 }, (_, index) => ({
    code: `issue-${index}`,
    message: `Issue ${index}`,
  }));
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Packet", diagnostics).step
    .document;
  assert.equal(document.status, "limited");
  assert.equal(document.diagnostics.length, displayLimits.diagnostics + 1);
  assert.ok(document.diagnostics.some((item) => item.code === "display-limit"));
});

test("an already reported issue does not exhaust the diagnostic limit", () => {
  const root = {
    name: "Packet",
    type: "Packet",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    issue: { kind: "unsupported", message: "Issue 0" },
  };
  const diagnostics = Array.from({ length: displayLimits.diagnostics }, (_, index) => ({
    code: `issue-${index}`,
    message: `Issue ${index}`,
  }));
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Packet", diagnostics).step
    .document;
  assert.equal(document.status, "partial");
  assert.equal(document.diagnostics.length, diagnostics.length);
});

test("the same long issue is reported once after display clipping", () => {
  const issue = { kind: "unsupported", message: "X".repeat(displayLimits.diagnosticMessage + 1) };
  const child = {
    name: "Issue",
    type: "Issue",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    issue,
  };
  const root = { ...child, name: "Packet", children: [child] };
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Packet", []).step.document;
  assert.equal(document.status, "limited");
  assert.equal(document.diagnostics.filter((item) => item.code === "mapi-unsupported").length, 1);
});

test("multibyte labels and diagnostics stay within the host's UTF-8 byte limits", () => {
  const root = {
    name: "界".repeat(displayLimits.fieldName),
    type: "界".repeat(displayLimits.protocolName),
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    value: { kind: "enum", value: "1", flags: false, names: ["界".repeat(displayLimits.enumName)] },
    issue: { kind: "unsupported", message: "😀".repeat(displayLimits.diagnosticMessage) },
  };
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Packet", []).step.document;
  const node = document.roots[0];
  assert.equal(document.status, "limited");
  for (const [text, maximumBytes] of [
    [node.name, displayLimits.fieldName],
    [node.format.protocol, displayLimits.protocolName],
    [node.value.value.name, displayLimits.enumName],
    [document.diagnostics[0].message, displayLimits.diagnosticMessage],
  ]) {
    assert.ok(text.length > 0);
    assert.ok(text.isWellFormed());
    assert.ok(Buffer.byteLength(text, "utf8") <= maximumBytes);
  }
});

test("multibyte string values retain their existing character allowance", () => {
  const text = "界😀".repeat(Math.floor(maximumDisplayStringLength / "界😀".length));
  assert.ok(Buffer.byteLength(text, "utf8") > maximumDisplayStringLength);
  const root = {
    name: "Text",
    type: "string",
    range: { sourceId: "body", offset: 0, length: 1 },
    children: [],
    value: { kind: "string", value: text },
  };
  const document = present({ root, derivedSources: [] }, [["body", 1]], "Text", []).step.document;
  assert.equal(document.status, "complete");
  assert.equal(document.roots[0].value.value, text);
});
