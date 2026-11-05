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
const { Inspection } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
globalThis.window = globalThis;
const tick = () => new Promise((resolve) => setImmediate(resolve));
const value = (handle) => ({ kind: "object", display: "Object", handle, truncated: false });
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
