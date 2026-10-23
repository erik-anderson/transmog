import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  isMemoryWorkbenchTarget,
  selectMemoryWorkbench,
  readProcessMemory,
  verifyWorkingSetGrowth,
  workingSetGrowthLimitBytes as limit,
} from "./desktop-memory.mjs";

const mib = 1024 * 1024;
const sample = (workingSetBytes) => ({
  processId: 42,
  startedAtUtc: "2026-10-19T00:00:00Z",
  sampledAtUtc: "2026-10-19T00:01:00Z",
  workingSetBytes,
  privateBytes: 100 * mib,
});
async function fixture(run) {
  const directory = await mkdtemp(join(tmpdir(), "transmog-memory-"));
  try {
    await run(join(directory, "memory.json"));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
const verify = (reportPath, samples, work = async () => ({ iterations: 100 })) =>
  verifyWorkingSetGrowth({
    processId: 42,
    reportPath,
    work,
    readSample: async () => {
      const next = samples.shift();
      if (next instanceof Error) throw next;
      return next;
    },
  });

test("loaded fixture footprint is excluded; growth at the limit passes", async () =>
  fixture(async (reportPath) => {
    const report = await verify(reportPath, [sample(350 * mib), sample(350 * mib + limit)]);
    assert.equal(report.growthBytes, limit);
    assert.equal(JSON.parse(await readFile(reportPath)).status, "passed");
  }));
test("growth one byte over the unchanged limit fails and retains evidence", async () =>
  fixture(async (reportPath) => {
    await assert.rejects(
      verify(reportPath, [sample(350 * mib), sample(350 * mib + limit + 1)]),
      /limit 256 MiB/,
    );
    const report = JSON.parse(await readFile(reportPath));
    assert.equal(report.status, "failed");
    assert.equal(report.growthBytes, limit + 1);
  }));
test("process exit, PID reuse and invalid counters cannot pass as zero growth", async () => {
  for (const after of [
    new Error("Process exited"),
    { ...sample(mib), startedAtUtc: "2026-10-19T00:01:00Z" },
    { ...sample(mib), processId: 43 },
    { ...sample(mib), workingSetBytes: undefined },
    { ...sample(mib), privateBytes: -1 },
  ]) {
    await fixture(async (reportPath) => {
      await assert.rejects(verify(reportPath, [sample(mib), after]));
      assert.equal(JSON.parse(await readFile(reportPath)).status, "failed");
    });
  }
});
test("workload failures remain failures with retained evidence", async () =>
  fixture(async (reportPath) => {
    const failure = new Error("Refresh command rejected");
    await assert.rejects(
      verify(reportPath, [sample(mib)], async () => {
        throw failure;
      }),
      (error) => error === failure,
    );
    assert.match(JSON.parse(await readFile(reportPath)).error, /Refresh command rejected/);
  }));
test(
  "Windows sampler measures the requested live process",
  { skip: process.platform !== "win32" },
  async () => {
    const measured = await readProcessMemory(process.pid);
    assert.equal(measured.processId, process.pid);
    assert.ok(measured.workingSetBytes > 0 && measured.privateBytes > 0);
    assert.ok(Number.isFinite(Date.parse(measured.startedAtUtc)));
  },
);

test("viewer memory checks refresh the large capture instead of the reopened empty main window", () => {
  const empty = { windowRole: "main", sessionCount: 0 };
  const small = { windowRole: "viewer", sessionCount: 1 };
  const large = { windowRole: "viewer", sessionCount: 20000 };
  assert.equal(selectMemoryWorkbench([empty, small, large], "viewer"), large);
  assert.equal(selectMemoryWorkbench([large, small, empty], "main"), empty);
  assert.throws(() => selectMemoryWorkbench([empty], "viewer"), /No initialized viewer/);
  assert.throws(() => selectMemoryWorkbench([large], "main"), /No initialized main/);
});
test("captured previews and closed or unrelated targets are excluded from memory probing", () => {
  assert.ok(isMemoryWorkbenchTarget({ type: "page", url: "http://transmog-ui.localhost/" }));
  for (const target of [
    { type: "page", url: "http://transmog-preview.localhost/captured-page" },
    { type: "page", url: "https://example.invalid/captured-app-shell" },
    { type: "page", url: "about:blank" },
    { type: "page", url: "" },
    { type: "worker", url: "http://transmog-ui.localhost/" },
  ])
    assert.equal(isMemoryWorkbenchTarget(target), false);
});
