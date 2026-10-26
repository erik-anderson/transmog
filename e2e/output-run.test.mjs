import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { beginOutputRun } from "./output-run.mjs";

test("output leases record explicit outcomes and protect interrupted runs", async () => {
  const directory = await mkdtemp(join(tmpdir(), "transmog-output-lease-"));
  const python = process.env.TRANSMOG_PYTHON ?? "python";
  try {
    for (const passed of [true, false, null]) {
      const run = join(directory, String(passed));
      await mkdir(run);
      const finish = await beginOutputRun(run, "load", python, "fixture");
      const running = JSON.parse(await readFile(join(run, ".transmog-test-run.json"), "utf8"));
      assert.equal(running.state, "running");
      await finish(passed);
      const result = JSON.parse(await readFile(join(run, ".transmog-test-run.json"), "utf8"));
      assert.equal(result.state, passed === null ? "interrupted" : "completed");
      assert.equal(result.passed, passed === null ? undefined : passed);
    }
    const existing = join(directory, "existing");
    await mkdir(existing);
    await writeFile(join(existing, ".transmog-test-run.json"), "{}");
    await assert.rejects(beginOutputRun(existing, "load", python), /already registered/);
  } finally {
    assert.ok(
      resolve(directory).startsWith(
        resolve(tmpdir()) + (process.platform === "win32" ? "\\" : "/"),
      ),
    );
    await rm(directory, { recursive: true, force: true });
  }
});
