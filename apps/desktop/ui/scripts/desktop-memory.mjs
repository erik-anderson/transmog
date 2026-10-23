import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

export const workingSetGrowthLimitBytes = 256 * 1024 * 1024;
const execute = promisify(execFile);
const sampler = fileURLToPath(
  new URL("../../../../scripts/measure-desktop-memory.ps1", import.meta.url),
);

export async function readProcessMemory(processId) {
  assert.ok(Number.isSafeInteger(processId) && processId > 0, "A native process ID is required");
  const { stdout } = await execute(
    "pwsh.exe",
    ["-NoProfile", "-NonInteractive", "-File", sampler, "-ProbeProcessId", String(processId)],
    {
      windowsHide: true,
      timeout: 10_000,
      maxBuffer: 64 * 1024,
    },
  );
  return JSON.parse(stdout);
}

export function isMemoryWorkbenchTarget(target) {
  try {
    return target.type === "page" && new URL(target.url).origin === "http://transmog-ui.localhost";
  } catch {
    return false;
  }
}

export function selectMemoryWorkbench(clients, windowRole) {
  assert.ok(
    ["main", "viewer"].includes(windowRole),
    "Memory probe window role must be main or viewer",
  );
  const candidates = clients.filter((client) => client.windowRole === windowRole);
  assert.ok(
    candidates.length,
    `No initialized ${windowRole} workbench is available for the memory gate`,
  );
  for (const client of candidates)
    assert.ok(
      Number.isSafeInteger(client.sessionCount) && client.sessionCount >= 0,
      "Invalid workbench session count",
    );
  // Viewer suites leave several captures open, plus an empty reopened main window.
  return candidates.toSorted((left, right) => right.sessionCount - left.sessionCount)[0];
}

function validateSample(sample, processId) {
  assert.equal(sample.processId, processId, "Memory sample belongs to another process");
  for (const key of ["startedAtUtc", "sampledAtUtc"]) {
    assert.ok(
      typeof sample[key] === "string" && Number.isFinite(Date.parse(sample[key])),
      `Invalid memory sample ${key}`,
    );
  }
  for (const key of ["workingSetBytes", "privateBytes"]) {
    assert.ok(
      Number.isSafeInteger(sample[key]) && sample[key] >= 0,
      `Invalid memory sample ${key}`,
    );
  }
}

// Compare one fixed workload in an already loaded application. Cold-to-loaded
// functional-suite growth is retained separately; fixture size is not a leak.
export async function verifyWorkingSetGrowth({
  processId,
  reportPath,
  work,
  readSample = readProcessMemory,
}) {
  assert.ok(Number.isSafeInteger(processId) && processId > 0, "A native process ID is required");
  assert.ok(reportPath, "A retained memory report is required");
  const report = {
    schemaVersion: 1,
    phase: "status-refresh",
    processId,
    limitBytes: workingSetGrowthLimitBytes,
    status: "running",
  };
  const save = () => writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
  try {
    report.before = await readSample(processId);
    validateSample(report.before, processId);
    await save();
    report.workload = await work();
    report.after = await readSample(processId);
    validateSample(report.after, processId);
    assert.equal(
      report.after.startedAtUtc,
      report.before.startedAtUtc,
      "Native process identity changed during measurement",
    );
    report.growthBytes = report.after.workingSetBytes - report.before.workingSetBytes;
    assert.ok(
      report.growthBytes <= workingSetGrowthLimitBytes,
      `Desktop status-refresh working set grew by ${(report.growthBytes / 1024 / 1024).toFixed(2)} MiB (limit 256 MiB)`,
    );
    report.status = "passed";
    await save();
    return report;
  } catch (error) {
    report.status = "failed";
    report.error = String(error);
    await save();
    throw error;
  }
}
