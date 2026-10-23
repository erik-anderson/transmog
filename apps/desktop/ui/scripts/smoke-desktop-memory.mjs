import assert from "node:assert/strict";
import { glob, readFile } from "node:fs/promises";
import { join } from "node:path";
import {
  isMemoryWorkbenchTarget,
  selectMemoryWorkbench,
  verifyWorkingSetGrowth,
} from "./desktop-memory.mjs";

const argument = (name) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
};
const port = argument("--port"),
  profileRoot = argument("--profile-root");
const processId = Number(argument("--process-id")),
  reportPath = argument("--memory-report");
const minutes = Number(argument("--soak-minutes") ?? 0);
const windowRole = argument("--window-role") ?? "main";
assert.ok(
  ["main", "viewer"].includes(windowRole),
  "Memory probe window role must be main or viewer",
);
assert.ok(Number.isFinite(minutes) && minutes >= 0 && minutes <= 240, "Invalid soak duration");
assert.match(port ?? "", /^\d{1,5}$/, "A loopback DevTools port is required");
assert.ok(
  profileRoot && reportPath && Number.isSafeInteger(processId) && processId > 0,
  "An isolated native profile, process ID and memory report are required",
);

const ports = new Set([port]);
for await (const path of glob("**/DevToolsActivePort", { cwd: profileRoot })) {
  try {
    ports.add((await readFile(join(profileRoot, path), "utf8")).split(/\r?\n/)[0]);
  } catch {
    /* Closed profiles can disappear. */
  }
}
const pages = (
  await Promise.all(
    [...ports].map(async (value) => {
      try {
        return await fetch(`http://127.0.0.1:${value}/json/list`, {
          signal: AbortSignal.timeout(5000),
        }).then((response) => response.json());
      } catch {
        return [];
      }
    }),
  )
)
  .flat()
  .filter(isMemoryWorkbenchTarget);
const sockets = [];

async function connect(target) {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  sockets.push(socket);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("Memory probe DevTools connection timed out")),
      5000,
    );
    socket.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
    socket.addEventListener(
      "error",
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
      { once: true },
    );
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const reply = JSON.parse(String(data)),
      task = pending.get(reply.id);
    if (!task) return;
    pending.delete(reply.id);
    clearTimeout(task.timer);
    if (reply.error) task.reject(new Error(reply.error.message));
    else task.resolve(reply.result);
  });
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 15_000);
      pending.set(id, { timer, resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const result = await call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result?.value;
  };
  return { call, evaluate };
}

try {
  const clients = [];
  for (const page of pages) {
    const client = await connect(page);
    const { evaluate } = client;
    const state = await evaluate(
      `(()=>{const shell=document.querySelector('app-shell'),traffic=shell?.shadowRoot?.querySelector('traffic-workspace');return traffic?{windowRole:shell.viewerMode?'viewer':'main',sessionCount:traffic.totalMatched??traffic.sessions.length}:null;})()`,
    );
    if (state) clients.push({ ...client, ...state });
  }
  const { call, evaluate, sessionCount } = selectMemoryWorkbench(clients, windowRole);
  const viewer = windowRole === "viewer";
  const refresh = async (iteration) =>
    evaluate(`(async()=>{
    const shell=document.querySelector('app-shell');
    ${viewer ? "await window.__TAURI_INTERNALS__.invoke('app_status');" : "await shell.refreshStatus();"}
    ${iteration % 20 === 0 ? "await shell.shadowRoot.querySelector('traffic-workspace').refreshSessions();" : ""}
  })()`);
  if (!viewer) await call("Performance.enable");
  await refresh(0);
  console.log(
    `Native memory gate: warmed ${windowRole} status workload (${sessionCount} rows); measuring ${minutes ? minutes + " minutes" : "100 refreshes"}.`,
  );
  const report = await verifyWorkingSetGrowth({
    processId,
    reportPath,
    work: async () => {
      const deadline = performance.now() + minutes * 60_000;
      let iterations = 0,
        nextProgress = performance.now() + 30_000;
      do {
        await refresh(iterations++);
        if (minutes && performance.now() < deadline)
          await new Promise((resolve) => setTimeout(resolve, 100));
        if (performance.now() >= nextProgress) {
          console.log(`Native memory gate: ${iterations} status refreshes completed.`);
          nextProgress += 30_000;
        }
      } while (iterations < (minutes ? 1 : 100) || performance.now() < deadline);
      let finalHeapBytes;
      if (!viewer) {
        const { metrics } = await call("Performance.getMetrics");
        finalHeapBytes = metrics.find((metric) => metric.name === "JSHeapUsedSize")?.value;
        assert.ok(
          Number.isFinite(finalHeapBytes) && finalHeapBytes < 64 * 1024 * 1024,
          `Bounded status soak exceeded 64 MiB JS heap: ${finalHeapBytes}`,
        );
      }
      return { iterations, minutes, windowRole, sessionCount, finalHeapBytes };
    },
  });
  console.log(
    `Native status-refresh memory gate passed: ${(report.growthBytes / 1024 / 1024).toFixed(2)} MiB growth (limit 256 MiB). Report: ${reportPath}`,
  );
} finally {
  for (const socket of sockets) socket.close();
}
