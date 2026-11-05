import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("../src/debugger-workspace/inspection.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { Inspection, ConsoleHistory } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
globalThis.window = globalThis;
const tick = () => new Promise((resolve) => setImmediate(resolve));
const value = (handle) => ({ kind: "object", display: "Object", handle, truncated: false });

test("history navigation preserves the draft through a surface snapshot", () => {
  const history = new ConsoleHistory();
  history.add("first");
  history.add("second");
  assert.equal(history.move(-1, "unsent draft"), "second");
  const restored = new ConsoleHistory();
  restored.restore(history.snapshot());
  assert.equal(restored.move(-1, "second"), "first");
  assert.equal(restored.move(1, "first"), "second");
  assert.equal(restored.move(1, "second"), "unsent draft");
});

test("source commands preserve history navigation and the unsent draft", () => {
  const history = new ConsoleHistory();
  history.add("first");
  history.add("second");
  assert.equal(history.move(-1, "unsent draft"), "second");
  history.add("from source", { preserveNavigation: true });
  const restored = new ConsoleHistory();
  restored.restore(history.snapshot());
  assert.equal(restored.move(-1, "second"), "first");
  assert.equal(restored.move(1, "first"), "second");
  assert.equal(restored.move(1, "second"), "from source");
  assert.equal(restored.move(1, "from source"), "unsent draft");
});

test("source commands retain a draft when old history is trimmed", () => {
  const history = new ConsoleHistory();
  for (let index = 0; index < 100; index++) history.add(String(index));
  assert.equal(history.move(-1, "draft"), "99");
  history.add("100", { preserveNavigation: true });
  assert.equal(history.move(-1, "99"), "98");
  assert.equal(history.move(1, "98"), "99");
  assert.equal(history.move(1, "99"), "100");
  assert.equal(history.move(1, "100"), "draft");
});

test("scope reads wait for native acceptance of pending surface releases", async () => {
  const sent = [];
  let finishRelease;
  const scope = { session: "s", pause: 1, frame: 0 };
  const registry = new Inspection(
    async (action) => {
      const request = sent.push(action);
      if (action.kind === "release")
        await new Promise((resolve) => {
          finishRelease = resolve;
        });
      return request;
    },
    () => true,
    () => {},
  );
  registry.releaseLeases([{ scope, handles: [1, 2] }]);
  const read = registry.request({ kind: "scopes", pause: 1, frame: 0 }, scope);
  await tick();
  assert.deepEqual(
    sent.map((action) => action.kind),
    ["release"],
  );
  finishRelease();
  await tick();
  assert.equal(sent[1].kind, "scopes");
  registry.receive("s", 2, { kind: "scopes", pause: 1, frame: 0, scopes: [] });
  assert.equal((await read).kind, "scopes");
  registry.dispose();
  await tick();
});
test("clearing a full handle budget sends one bounded release instead of flooding the control queue", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => {
      sent.push(action);
      return sent.length;
    },
    () => true,
    () => {},
  );
  const scope = { session: "s", pause: 1, frame: 0, generation: 1 };
  for (let handle = 1; handle <= 64; handle++)
    registry.set(String(handle), "Result", value(handle), scope);
  assert.equal(registry.leases()[0].handles.length, 64);
  registry.dispose();
  await tick();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].kind, "release");
  assert.equal(new Set(sent[0].handles).size, 64);
});
test("a replaced value releases late child handles without updating its replacement", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => {
      sent.push(action);
      return sent.length;
    },
    () => true,
    () => {},
  );
  const scope = { session: "s", pause: 1, frame: 0, generation: 1 };
  registry.set("result", "Result", value(1), scope);
  const expanding = registry.toggle("result");
  await tick();
  registry.set("result", "Replacement", value(2), scope);
  registry.receive("s", 1, {
    kind: "properties",
    pause: 1,
    handle: 1,
    offset: 0,
    next: null,
    properties: [{ name: "child", value: value(3) }],
  });
  await expanding;
  await tick();
  assert.equal(registry.get("result").label, "Replacement");
  assert.deepEqual(registry.get("result").rows, []);
  assert.deepEqual(
    sent
      .filter((action) => action.kind === "release")
      .flatMap((action) => action.handles)
      .sort(),
    [1, 3],
  );
  registry.dispose();
  await tick();
});
test("idle values expire across a missed pause and resume, and stale leases cannot release current values", async () => {
  let generation = 0;
  const sent = [];
  const registry = new Inspection(
    async (action) => {
      sent.push(action);
      return sent.length;
    },
    (scope) => scope.generation === generation,
    () => {},
  );
  const scope = { session: "s", pause: 0, frame: null, generation };
  registry.set("old", "Result", value(1), scope);
  generation++;
  registry.expire();
  assert.equal(registry.get("old").expired, true);
  registry.releaseLeases([{ scope, handles: [1] }]);
  await tick();
  assert.equal(sent.length, 0);
  registry.dispose();
});

test("loading more appends properties and ignores duplicate reads", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  const scope = { session: "s", pause: 1, frame: 0 };
  registry.set("result", "Result", value(1), scope);
  const first = registry.toggle("result");
  await tick();
  assert.equal(sent[0].kind, "preview-properties");
  registry.receive("s", 1, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 0,
    total: 2,
    next: 1,
    properties: [{ name: "old", value: value(null) }],
  });
  await first;
  const more = registry.get("result").rows.at(-1);
  const loading = registry.toggle("result", more);
  await tick();
  await registry.toggle("result", more);
  assert.equal(sent.length, 2);
  assert.equal(sent[1].offset, 1);
  registry.receive("s", 2, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 1,
    total: 2,
    next: null,
    properties: [{ name: "new", value: value(null) }],
  });
  await loading;
  assert.deepEqual(
    registry.get("result").rows.map((row) => row.name),
    ["old", "new"],
  );
  assert.deepEqual(registry.leases()[0].handles, [1]);
  registry.dispose();
  await tick();
});

test("expansion retains one selected property and collapsing releases it", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 1, frame: 0 });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 0,
    total: 1,
    next: null,
    properties: [{ name: "child", value: value(null) }],
  });
  await first;
  const expanding = registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  assert.deepEqual(sent[1], { kind: "property", pause: 1, handle: 1, index: 0 });
  registry.receive("s", 2, { kind: "evaluated", pause: 1, value: value(2), thrown: false });
  await tick();
  assert.equal(sent[2].kind, "preview-properties");
  assert.equal(sent[2].handle, 2);
  registry.receive("s", 3, {
    kind: "property-preview",
    pause: 1,
    handle: 2,
    offset: 0,
    total: 0,
    next: null,
    properties: [],
  });
  await expanding;
  await registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  assert.deepEqual(sent.find((action) => action.kind === "release").handles, [2]);
  registry.dispose();
  await tick();
});

test("selecting a property refreshes its preview when the live value changed", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 0, frame: null });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "property-preview",
    pause: 0,
    handle: 1,
    offset: 0,
    total: 1,
    next: null,
    properties: [{ name: "child", value: value(null) }],
  });
  await first;
  const expanding = registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  registry.receive("s", 2, {
    kind: "evaluated",
    pause: 0,
    value: { kind: "number", display: "42", handle: null, truncated: false },
    thrown: false,
  });
  await expanding;
  assert.equal(registry.get("result").rows[0].preview, "42");
  assert.equal(registry.get("result").rows[0].expandable, false);
  assert.equal(registry.get("result").issue, "");
  assert.equal(sent.length, 2);
  registry.dispose();
  await tick();
});

test("collapsing while a property is being selected releases the late value", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 1, frame: 0 });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 0,
    total: 1,
    next: null,
    properties: [{ name: "child", value: value(null) }],
  });
  await first;
  const expanding = registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  await registry.toggle("result");
  registry.receive("s", 2, { kind: "evaluated", pause: 1, value: value(2), thrown: false });
  await expanding;
  await tick();
  assert.equal(registry.get("result").expanded, false);
  assert.equal(registry.get("result").loading, false);
  assert.deepEqual(
    sent.map((action) => action.kind),
    ["preview-properties", "property", "release"],
  );
  assert.deepEqual(sent[2].handles, [2]);
  registry.dispose();
  await tick();
});

test("large collections expand into bounded ranges instead of replacing pages", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 1, frame: 0 });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 0,
    total: 5000,
    next: 100,
    properties: [],
  });
  await first;
  assert.equal(registry.get("result").rows.length, 50);
  const last = registry.get("result").rows.at(-1);
  assert.equal(last.name, "Properties 4901–5000");
  const expanding = registry.toggle("result", last);
  await tick();
  assert.equal(sent[1].offset, 4900);
  assert.equal(sent[1].limit, 100);
  assert.equal(sent[1].refresh, false);
  registry.receive("s", 2, {
    kind: "property-preview",
    pause: 1,
    handle: 1,
    offset: 4900,
    total: 5000,
    next: null,
    properties: [{ name: "last", value: value(null) }],
  });
  await expanding;
  assert.equal(registry.get("result").rows.at(-1).name, "last");
  registry.dispose();
  await tick();
});

test("identical preview names expand independently and collapsing releases only descendants", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 1, frame: 0 });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "properties",
    pause: 1,
    handle: 1,
    offset: 0,
    next: null,
    properties: [2, 3].map((handle) => ({ name: "[Symbol key]", value: value(handle) })),
  });
  await first;
  const [a, b] = registry.get("result").rows;
  assert.notEqual(a.key, b.key);
  const expand = registry.toggle("result", a);
  await tick();
  registry.receive("s", 2, {
    kind: "properties",
    pause: 1,
    handle: 2,
    offset: 0,
    next: null,
    properties: [{ name: "first child", value: value(4) }],
  });
  await expand;
  const second = registry.toggle("result", b);
  await tick();
  registry.receive("s", 3, {
    kind: "properties",
    pause: 1,
    handle: 3,
    offset: 0,
    next: null,
    properties: [{ name: "second child", value: value(5) }],
  });
  await second;
  await registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  assert.deepEqual(sent.find((action) => action.kind === "release").handles, [4]);
  assert.deepEqual(
    registry.get("result").rows.map((row) => row.name),
    ["[Symbol key]", "[Symbol key]", "second child"],
  );
  registry.dispose();
  await tick();
});

test("collapsing a pending branch frees both its parent page and late results", async () => {
  const sent = [];
  const registry = new Inspection(
    async (action) => sent.push(action),
    () => true,
    () => {},
  );
  registry.set("result", "Result", value(1), { session: "s", pause: 1, frame: 0 });
  const first = registry.toggle("result");
  await tick();
  registry.receive("s", 1, {
    kind: "properties",
    pause: 1,
    handle: 1,
    offset: 0,
    next: null,
    properties: [{ name: "child", value: value(2) }],
  });
  await first;
  const nested = registry.toggle("result", registry.get("result").rows[0]);
  await tick();
  await registry.toggle("result");
  registry.receive("s", 2, {
    kind: "properties",
    pause: 1,
    handle: 2,
    offset: 0,
    next: null,
    properties: [{ name: "late", value: value(3) }],
  });
  await nested;
  await tick();
  assert.equal(registry.get("result").expanded, false);
  assert.equal(registry.get("result").loading, false);
  assert.deepEqual(
    sent
      .filter((action) => action.kind === "release")
      .flatMap((action) => action.handles)
      .sort(),
    [2, 3],
  );
  registry.dispose();
  await tick();
});
