import assert from "node:assert/strict";
import { copyFile, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { resolve, join } from "node:path";
import { smokeResponseSetIntake } from "./smoke-response-set-intake.mjs";
import { smokeResponseSetDeparture } from "./smoke-response-set-departure.mjs";
import { smokeSavedDemos, smokeSavedDemoAssociations } from "./smoke-saved-demos.mjs";
import { smokeResponseSetGuide } from "./smoke-response-set-guide.mjs";
import { smokeResponseFollowups } from "./smoke-response-followups.mjs";

const option = (name) => process.argv[process.argv.indexOf(name) + 1];
assert.ok(
  process.argv.includes("--source") && process.argv.includes("--output"),
  "Pass a compiled snapshot and evidence directory explicitly",
);
const runId = randomUUID(),
  source = resolve(option("--source")),
  directory = resolve(option("--output"), "native-" + runId),
  profile = join(directory, "profile"),
  temporary = join(directory, "temp"),
  executable = join(directory, "transmog-owned-response-set-" + runId + ".exe");
await mkdir(profile, { recursive: true });
await mkdir(temporary);
await copyFile(source, executable);
const sourceSha256 = createHash("sha256")
    .update(await readFile(source))
    .digest("hex"),
  pids = [],
  errors = [];
let child = null,
  current = null,
  failure = null;
const startup = [];
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function targets() {
  const ports = new Set();
  const scan = async (path, depth = 0) => {
    if (depth > 12) return;
    for (const entry of await readdir(path, { withFileTypes: true }).catch(() => [])) {
      const candidate = join(path, entry.name);
      if (entry.isDirectory()) await scan(candidate, depth + 1);
      else if (entry.name === "DevToolsActivePort") {
        const port = Number((await readFile(candidate, "utf8").catch(() => "")).split(/\r?\n/)[0]);
        if (port > 0) ports.add(port);
      }
    }
  };
  await scan(profile);
  await scan(temporary);
  const all = [];
  for (const port of ports) {
    try {
      all.push(
        ...(await (
          await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(1000) })
        ).json()),
      );
    } catch {}
  }
  return all;
}
async function connect(target) {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(String(data));
    if (message.id) {
      const request = pending.get(message.id);
      pending.delete(message.id);
      if (!request) return;
      if (message.error) request.reject(new Error(message.error.message));
      else request.resolve(message.result);
    } else if (message.method === "Runtime.exceptionThrown")
      errors.push(message.params.exceptionDetails);
  });
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const result = await call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails)
      throw new Error(
        result.exceptionDetails.exception?.description ?? result.exceptionDetails.text,
      );
    return result.result.value;
  };
  await call("Runtime.enable");
  await call("Page.enable");
  return { call, evaluate, close: () => socket.close() };
}
async function stop() {
  current?.close();
  current = null;
  if (!child) return;
  if (child.exitCode !== null || child.signalCode !== null) {
    child = null;
    return;
  }
  const owned = child,
    exited = new Promise((resolve) => owned.once("exit", () => resolve(true)));
  owned.kill();
  const stopped = await Promise.race([exited, delay(5000).then(() => false)]);
  assert.ok(
    stopped || owned.exitCode !== null || owned.signalCode !== null,
    "Owned native process did not stop",
  );
  child = null;
}
async function start() {
  const attempt = {
    startedAt: new Date().toISOString(),
    stdout: "",
    stderr: "",
    exitCode: null,
    signal: null,
    error: null,
  };
  startup.push(attempt);
  child = spawn(executable, [], {
    env: {
      ...process.env,
      LOCALAPPDATA: profile,
      TEMP: temporary,
      TMP: temporary,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        "--remote-debugging-port=0 --remote-debugging-address=127.0.0.1",
    },
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  pids.push(child.pid);
  child.stdout.on("data", (chunk) => {
    attempt.stdout = (attempt.stdout + chunk.toString()).slice(-65536);
  });
  child.stderr.on("data", (chunk) => {
    attempt.stderr = (attempt.stderr + chunk.toString()).slice(-65536);
  });
  child.on("error", (error) => {
    attempt.error = String(error);
  });
  child.on("exit", (code, signal) => {
    attempt.exitCode = code;
    attempt.signal = signal;
  });
  const deadline = Date.now() + 45000;
  let target;
  while (
    !(target = (await targets()).find(
      (row) => row.type === "page" && /transmog|tauri/.test(row.url),
    ))
  ) {
    if (child.exitCode !== null || child.signalCode !== null || attempt.error)
      throw new Error("Owned native process exited before CDP: " + JSON.stringify(attempt));
    if (Date.now() > deadline)
      throw new Error("Owned native main target unavailable: " + JSON.stringify(attempt));
    await delay(100);
  }
  current = await connect(target);
  const main = current;
  const waitFor = async (expression, message = "") => {
    const end = Date.now() + 30000;
    for (;;) {
      const result =
        typeof expression === "function"
          ? await expression()
          : await main.evaluate(`(async()=>!!(await (${expression})))()`);
      if (result) return result;
      if (Date.now() > end) throw new Error(message || "Native wait timed out: " + expression);
      await delay(40);
    }
  };
  await waitFor("document.querySelector('app-shell')?.workspaceLoaded");
  return { main, waitFor };
}
try {
  let context = await start();
  if (process.argv.includes("--followups-only")) {
    const followups = await smokeResponseFollowups({ ...context, directory });
    assert.deepEqual(errors, []);
    await writeFile(
      join(directory, "native-followups-result.json"),
      JSON.stringify(
        {
          source,
          sourceSha256,
          executable,
          profile,
          temporary,
          pids,
          followups,
          runtimeErrors: errors,
        },
        null,
        2,
      ),
    );
    process.stdout.write(JSON.stringify({ directory, sourceSha256, pids, followups }) + "\n");
  } else if (process.argv.includes("--demo-associations-only")) {
    const associations = await smokeSavedDemoAssociations({ ...context, directory });
    assert.deepEqual(errors, []);
    await writeFile(
      join(directory, "native-association-result.json"),
      JSON.stringify(
        {
          source,
          sourceSha256,
          executable,
          profile,
          temporary,
          pids,
          associations,
          runtimeErrors: errors,
        },
        null,
        2,
      ),
    );
    process.stdout.write(JSON.stringify({ directory, sourceSha256, pids, associations }) + "\n");
  } else {
    const guideOwner = "native-guide-set";
    await context.main.evaluate(
      `(async()=>{const i=window.__TAURI_INTERNALS__.invoke,asset=await i('create_response_asset',{input:{id:'native-guide-response',revision:1,status:200,headers:[],body:[...new TextEncoder().encode('Native saved guide body')],mediaType:'text/plain'}}),action={headers:[],replaceBody:null,discardBody:false,abortReason:null},rule={id:'native-guide-rule',displayName:'Native guide response',revision:1,priority:0,enabled:true,matcher:{method:'GET',url:{kind:'exact',value:'https://guide.invalid/response'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null},request:{...action,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:action},status=await i('automation_status');await i('change_response_set',{generation:status.generation,change:{action:'create',id:${JSON.stringify(guideOwner)},name:'Native guide workspace',notes:'Optional guide leaves saved work unchanged.',rules:[rule]}});})()`,
    );
    const openGuideOwner = async () =>
      context.main.evaluate(
        `(async()=>{const s=document.querySelector('app-shell');await s.activateView('automation');await s.automation.refreshAutomation();await s.automation.showAutomationSection('experiments');await s.automation.openResponseSet(new CustomEvent('set-open',{detail:{id:${JSON.stringify(guideOwner)}}}),true);})()`,
      );
    await openGuideOwner();
    await smokeResponseSetGuide({ ...context, screenshot: join(directory, "native-guide.png") });
    let departure = null,
      intake = null,
      demos = null;
    if (process.argv.includes("--guide-only")) {
      await stop();
      await delay(500);
      context = await start();
    } else {
      departure = await smokeResponseSetDeparture({ ...context, directory });
      intake = process.argv.includes("--skip-intake")
        ? null
        : await smokeResponseSetIntake({ ...context, directory });
      demos = await smokeSavedDemos({
        ...context,
        directory,
        restart: async () => {
          await stop();
          await delay(500);
          context = await start();
          return context;
        },
        preview: async (url) => {
          const target = (await targets()).find((row) => row.type === "page" && row.url === url);
          assert.ok(target, "Frozen preview target missing");
          return connect(target);
        },
      });
    }
    await openGuideOwner();
    await smokeResponseSetGuide({
      ...context,
      screenshot: join(directory, "native-guide-restart.png"),
      reload: true,
    });
    const guide = {
      pointerAndKeyboard: true,
      delayedReplies: true,
      fourSteps: true,
      layouts: true,
      persistedAfterRestart: true,
    };
    assert.deepEqual(errors, []);
    await writeFile(
      join(directory, "native-combined-result.json"),
      JSON.stringify(
        {
          source,
          sourceSha256,
          executable,
          profile,
          temporary,
          pids,
          departure,
          intake,
          demos,
          guide,
          runtimeErrors: errors,
        },
        null,
        2,
      ),
    );
    process.stdout.write(
      JSON.stringify({ directory, sourceSha256, pids, departure, intake, demos, guide }) + "\n",
    );
  }
} catch (error) {
  failure = error;
  let probe = null;
  if (current) {
    const image = await current.call("Page.captureScreenshot", { format: "png" }).catch(() => null);
    if (image)
      await writeFile(join(directory, "native-failure.png"), Buffer.from(image.data, "base64"));
    probe = await current
      .evaluate(
        `({departureCallbacks:globalThis.departureCallbackResults,releaseDepartureFile:!!globalThis.releaseDepartureFile,collectionOpen:document.querySelector('app-shell')?.automation?.collectionOpen,fileChoiceBusy:document.querySelector('app-shell')?.automation?.fileChoiceBusy})`,
      )
      .catch(() => null);
  }
  await writeFile(
    join(directory, "native-failure.json"),
    JSON.stringify(
      { source, sourceSha256, pids, message: String(error), runtimeErrors: errors, probe },
      null,
      2,
    ),
  );
  throw error;
} finally {
  let cleanupError = null;
  try {
    await stop();
  } catch (error) {
    cleanupError = String(error);
  }
  await writeFile(join(directory, "startup.json"), JSON.stringify(startup, null, 2));
  await writeFile(
    join(directory, "owned-processes.json"),
    JSON.stringify(
      { source, sourceSha256, executable, profile, pids, closed: child === null, cleanupError },
      null,
      2,
    ),
  );
  if (cleanupError && !failure) throw new Error(cleanupError);
}
