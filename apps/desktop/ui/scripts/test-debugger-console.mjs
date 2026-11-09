import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
const compiled = await build({
  entryPoints: [
    fileURLToPath(new URL("../src/debugger-workspace/console-output.ts", import.meta.url)),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { ConsoleOutput } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const presentation = await build({
  entryPoints: [
    fileURLToPath(new URL("../src/debugger-workspace/console-presentation.ts", import.meta.url)),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { restoreLevels, levelsLabel, resultText } = await import(
  "data:text/javascript;base64," + Buffer.from(presentation.outputFiles[0].text).toString("base64")
);
test("string results distinguish empty strings, escaped content and shortened output", () => {
  const value = { kind: "string", display: "", handle: null, truncated: false };
  assert.equal(resultText(value), '""');
  assert.equal(resultText({ ...value, display: "first\nsecond" }), '"first\\nsecond"');
  assert.equal(
    resultText({ ...value, display: "partial", truncated: true }),
    '"partial"\n[Shortened]',
  );
});
test("saved level choices preserve an empty mask and recover unsupported legacy filters", () => {
  assert.deepEqual(restoreLevels([], "warn"), []);
  assert.deepEqual(restoreLevels(undefined, "trace"), ["info"]);
  assert.deepEqual(restoreLevels(undefined, "debug"), ["verbose"]);
  assert.equal(levelsLabel(restoreLevels(undefined, "unknown")), "Default levels");
  assert.equal(levelsLabel(restoreLevels(undefined, "all")), "Default levels");
});
const run = () => ({
  context: { worker: "worker", invocation: 1, callback: "onRequestHead" },
  records: [{ sequence: 1 }, { sequence: 2 }],
  dropped: 5,
  incomplete: false,
});
const view = (logs, session = "session") => ({
  info: { session },
  stream: { logs, active: false, operation: null },
  outcome: null,
});
test("clearing survives a surface move and preserves later output and loss from the same callback", () => {
  const logs = run();
  const target = view([logs]);
  const output = new ConsoleOutput();
  output.clear([target]);
  const restored = new ConsoleOutput();
  restored.restore(output.snapshot());
  assert.deepEqual(restored.records(target, logs), []);
  logs.records.push({ sequence: 3 });
  logs.dropped = 8;
  assert.deepEqual(restored.records(target, logs), [{ sequence: 3 }]);
  assert.deepEqual(restored.loss(target, logs), { dropped: 3, incomplete: false });
});
test("clearing active output does not conceal a later incomplete capture", () => {
  const logs = run();
  logs.incomplete = true;
  const target = view([logs]);
  target.stream.active = true;
  target.stream.operation = { invocation: 1 };
  const output = new ConsoleOutput();
  output.clear([target]);
  assert.equal(output.loss(target, logs).incomplete, false);
  target.stream.active = false;
  assert.equal(output.loss(target, logs).incomplete, true);
  output.clear([target]);
  assert.equal(output.loss(target, logs).incomplete, false);
});
test("clear positions stay scoped to their retained worker runs", () => {
  const logs = run();
  const target = view([logs]);
  const other = view([logs], "other");
  const output = new ConsoleOutput();
  output.clear([target]);
  assert.equal(output.records(other, logs).length, 2);
  output.clear([other]);
  output.retain([target]);
  assert.equal(output.snapshot().length, 1);
  assert.deepEqual(output.records(target, logs), []);
});
