import assert from "node:assert/strict";
import test from "node:test";
import { createBrowserCheckRunner, reportBrowserCheckFailure } from "./browser-check-report.mjs";

test("check progress retains the failing name and original failure", async () => {
  const events = [];
  const run = createBrowserCheckRunner((event) => events.push(event));
  const original = new Error("selection assertion failed");
  async function selection() {
    throw original;
  }
  await assert.rejects(run(selection), (error) => error === original);
  assert.deepEqual(
    events.map(({ name, result }) => ({ name, result })),
    [
      { name: "selection", result: "started" },
      { name: "selection", result: "failed" },
    ],
  );
  assert.ok(events[1].durationSeconds >= 0);
});

test("successful checks retain their result and arguments", async () => {
  const events = [];
  const run = createBrowserCheckRunner((event) => events.push(event));
  async function fixture(value) {
    return value;
  }
  assert.equal(await run(fixture, "kept"), "kept");
  assert.equal(events[1].result, "passed");
});

test("closed-page screenshot and state failures cannot replace the original error", async () => {
  const original = new Error("original browser assertion");
  const messages = [];
  const error = await reportBrowserCheckFailure(
    original,
    [
      {
        name: "screenshot",
        collect: () => {
          throw new Error("Target page, context or browser has been closed");
        },
      },
      { name: "state", collect: () => Promise.reject(new Error("context destroyed")) },
    ],
    { write: (message) => messages.push(message) },
  );
  assert.equal(error, original);
  assert.match(messages[0], /original browser assertion/);
  assert.match(messages.join(""), /Browser diagnostic screenshot failed/);
  assert.match(messages.join(""), /Browser diagnostic state failed/);
});

test("an unresponsive diagnostic is bounded while independent evidence is retained", async () => {
  const original = new Error("original timeout");
  const messages = [];
  let captured = false;
  const error = await reportBrowserCheckFailure(
    original,
    [
      { name: "unresponsive", collect: () => new Promise(() => {}) },
      {
        name: "retained",
        collect: () => {
          captured = true;
        },
      },
    ],
    { timeoutMs: 20, write: (message) => messages.push(message) },
  );
  assert.equal(error, original);
  assert.equal(captured, true);
  assert.match(messages.join(""), /unresponsive failed: exceeded 20 ms/);
});
