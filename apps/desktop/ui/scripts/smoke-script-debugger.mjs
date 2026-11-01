import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";
import { createServer, request as httpRequest } from "node:http";
import { execFileSync } from "node:child_process";

// Owned disposable profile only; invoked by test-windows-desktop.ps1.
const argument = (name) => process.argv[process.argv.indexOf(name) + 1];
const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
const page = browser.contexts()[0].pages()[0];
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const evidence = { pageErrors: errors };
function resources() {
  const pid = Number(argument("--process-id"));
  assert.ok(Number.isSafeInteger(pid) && pid > 0);
  const query = `$p=Get-Process -Id ${pid};$workers=@(Get-CimInstance Win32_Process -Filter 'ParentProcessId=${pid}' | Where-Object {$_.Name -in @('transmog-script-host.exe','transmog-extension-host.exe')});[ordered]@{handles=$p.HandleCount;threads=$p.Threads.Count;privateBytes=$p.PrivateMemorySize64;workers=$workers.Count}|ConvertTo-Json -Compress`;
  return JSON.parse(
    execFileSync("powershell.exe", ["-NoProfile", "-Command", query], {
      encoding: "utf8",
      windowsHide: true,
    }),
  );
}
let origin;
async function waitFor(check, label, timeout = 10000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await check()) return;
    await new Promise((done) => setTimeout(done, 50));
  }
  throw new Error(`Timed out: ${label}`);
}
async function paused(panel, after = 0) {
  await waitFor(
    () => panel.evaluate((element, previous) => element.paused && element.pause > previous, after),
    "native engine pause",
  );
  return panel.evaluate((element) => element.pause);
}
async function continueExecution(panel) {
  await panel.getByRole("button", { name: /^Continue/ }).click();
}
async function localSeven(panel) {
  const local = panel.getByRole("button", { name: /^local(?: ·|$)/ }).first();
  await local.click();
  await panel.getByRole("cell", { name: "7", exact: true }).first().waitFor();
}
async function closeDebugger(panel) {
  await panel.getByRole("button", { name: "Close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });
}
const invoke = (command, args = {}) =>
  page.evaluate(
    ([command, args]) => window.__TAURI_INTERNALS__.invoke(command, args),
    [command, args],
  );
function proxyGet(listener, url) {
  const [hostname, port] = listener.split(":");
  return new Promise((done, fail) => {
    const request = httpRequest(
      { hostname, port, path: url, headers: { Host: new URL(url).host }, agent: false },
      (response) => {
        let body = "";
        response.setEncoding("utf8");
        response.on("data", (part) => {
          body += part;
        });
        response.on("end", () =>
          done({
            status: response.statusCode,
            body,
            observed: response.headers["x-debug-observed"] ?? null,
          }),
        );
      },
    );
    request.setTimeout(15000, () => request.destroy(new Error("Loopback client timed out")));
    request.on("error", fail);
    request.end();
  });
}
try {
  await page.waitForFunction(() => {
    const shell = document.querySelector("app-shell");
    return !!shell?.settings && typeof shell.activateView === "function";
  });
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    if (!(await shell.activateView("automation"))) throw new Error(shell.diagnosticText);
    await shell.automation.showAutomationSection("scripts");
    shell.automation.$flushUpdates();
  });
  const editor = page.locator("script-editor");
  await waitFor(() => editor.evaluate((element) => element.scriptReady), "script editor");
  const source = [
    "import {pass} from 'transmog:api';",
    "console.info('native debug initializer');",
    "const initialized: number = 7; debugger;",
    "function leaf(value: number) { return value + 1; }",
    "function inner(input: number) {",
    "  const local = input + 4;",
    "  const object = {get unsafe() {throw new Error('getter must not run')}};",
    "  debugger;",
    "  const result = leaf(local);",
    "  return result;",
    "}",
    "function outer(input: number) { return inner(input); }",
    "export function onRequestHead() {const result = outer(3); console.log('native result', result); return pass();}",
  ].join("\n");
  await editor.evaluate((element, text) => element.sourceEditor.setValue(text), source);
  await editor.getByRole("button", { name: "Debug test", exact: true }).click();
  let panel = editor.locator("script-debugger");
  let pause = await paused(panel);
  assert.equal(await panel.evaluate((element) => element.invocation), 0);
  await panel.locator("script-diagnostics .developer-details > summary").first().click();
  await panel
    .getByText(/native debug initializer/)
    .first()
    .waitFor();
  await continueExecution(panel);
  pause = await paused(panel, pause);
  await localSeven(panel);
  await panel.getByRole("button", { name: "[Object]", exact: true }).click();
  await panel.getByRole("cell", { name: /Accessor/ }).waitFor();
  await editor.evaluate((element) =>
    element.sourceEditor.setValue(
      "// Newer draft remains untouched\n" + element.sourceEditor.getValue(),
    ),
  );
  await panel.getByText(/Draft edits are newer/).waitFor();
  for (const name of ["Step over", "Step into", "Step out"]) {
    await panel.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    pause = await paused(panel, pause);
  }
  await localSeven(panel);
  assert.ok(
    await panel
      .locator(".property-page")
      .evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
  );
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await panel.scrollIntoViewIfNeeded();
    assert.ok(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1));
    await page.screenshot({
      path: resolve(argument("--output"), `script-debugger-native-${name}.png`),
    });
  }
  await continueExecution(panel);
  await panel.getByText("Callback completed", { exact: true }).waitFor();
  assert.equal(await panel.getByRole("button", { name: /^Continue/ }).isDisabled(), true);
  await closeDebugger(panel);
  evidence.sandbox = {
    initializer: true,
    localSeven: true,
    gettersInert: true,
    allSteps: true,
    immutableDraft: true,
  };

  // Real proxy traffic, manually routed over loopback with HTTPS decryption off.
  const state = await invoke("product_state");
  state.preferences.configureSystemProxy = false;
  state.preferences.decryptHttps = false;
  state.preferences.proxyPort = null;
  await invoke("save_product_state", { productState: state });
  const liveSource =
    "import {edit,setHeader} from 'transmog:api';let count=0;export function onRequestHead(event: any){const local=7;console.log('live',++count);if(event.request.path==='/debug')debugger;return edit({headers:[setHeader('x-debug-count',String(count))]})}";
  await editor.evaluate((element, text) => {
    element.sourceEditor.setValue(text);
    element.scriptForm.elements.namedItem("scriptHost").value = "127.0.0.1";
    element.scriptForm.elements.namedItem("scriptHeaders").value = "x-debug-count";
    element.markScriptChanged();
  }, liveSource);
  await editor.getByRole("button", { name: "Save & enable", exact: true }).click();
  await waitFor(
    () => editor.evaluate((element) => element.scriptActive && !element.scriptBusy),
    "applied live build",
  );
  const bootstrap = await invoke("desktop_bootstrap");
  const status = await invoke("start_proxy", {
    request: {
      caCertificatePath: bootstrap.caCertificatePath,
      caPrivateKeyPath: bootstrap.caPrivateKeyPath,
      listen: "127.0.0.1:0",
      route: "auto",
      allowRemoteClients: false,
    },
  });
  assert.match(status.listener, /^127\.0\.0\.1:/);
  origin = createServer((request, response) => {
    response.writeHead(200, {
      "Content-Type": "text/plain",
      "X-Debug-Observed": request.headers["x-debug-count"] ?? "missing",
    });
    response.end("native origin");
  });
  await new Promise((done) => origin.listen(0, "127.0.0.1", done));
  const base = `http://127.0.0.1:${origin.address().port}`;
  assert.equal((await proxyGet(status.listener, `${base}/prime`)).body, "native origin");
  await editor.locator(".debug-live>summary").click();
  await editor.getByRole("button", { name: "Review applied revision", exact: true }).click();
  await editor.getByRole("button", { name: "Arm for 60 seconds", exact: true }).click();
  panel = editor.locator("script-debugger");
  await panel
    .getByText("Armed · waiting for the next matching exchange", { exact: true })
    .waitFor();
  const selected = proxyGet(status.listener, `${base}/debug`);
  await paused(panel);
  await localSeven(panel);
  const competing = await proxyGet(status.listener, `${base}/competitor`).catch((error) => ({
    status: 0,
    body: error.message,
  }));
  assert.notEqual(competing.status, 200);
  assert.equal(await panel.evaluate((element) => element.paused), true);
  await continueExecution(panel);
  assert.deepEqual(await selected, { status: 200, body: "native origin", observed: "2" });
  await panel
    .getByText("Selected exchange completed; debugger detached and worker retained", {
      exact: true,
    })
    .waitFor();
  assert.equal((await proxyGet(status.listener, `${base}/after`)).observed, "3");
  const runtime = (await invoke("script_runtime_status"))[0];
  assert.ok(runtime.available);
  assert.ok(runtime.logs.flatMap((run) => run.records).some((record) => record.text === "live 3"));
  await closeDebugger(panel);
  // Applying a replacement while paused must end the old owner without restoring it.
  await editor.getByRole("button", { name: "Review applied revision", exact: true }).click();
  await editor.getByRole("button", { name: "Arm for 60 seconds", exact: true }).click();
  panel = editor.locator("script-debugger");
  await panel
    .getByText("Armed · waiting for the next matching exchange", { exact: true })
    .waitFor();
  const replaced = proxyGet(status.listener, `${base}/debug`).catch((error) => ({
    status: 0,
    body: error.message,
  }));
  await paused(panel);
  await editor.evaluate(
    (element, text) => element.sourceEditor.setValue(text),
    liveSource.replace("let count=0", "let count=100"),
  );
  await editor.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitFor(
    () => editor.evaluate((element) => !element.scriptBusy && !element.scriptUnapplied),
    "replacement applied",
  );
  await waitFor(() => panel.evaluate((element) => element.closed), "replaced debugger ended");
  assert.notEqual((await replaced).status, 200);
  assert.equal((await proxyGet(status.listener, `${base}/replacement`)).observed, "101");
  await closeDebugger(panel);
  await invoke("stop_application");
  evidence.live = {
    actualGlobalsPreserved: true,
    competingRejected: true,
    continuePreservedResponse: true,
    headerEditAfterContinue: true,
    replacementEndsOldOwner: true,
    detachedHealthy: true,
    manualLoopbackOnly: true,
  };

  await page.evaluate(async (trace) => {
    const shell = document.querySelector("app-shell");
    await shell.activateView("traffic");
    const traffic = shell.traffic;
    await traffic.importTrace(trace);
    const row = traffic.sessions.find(
      (row) => row.path === "/extension-fixture" && row.responseBytes > 1,
    );
    traffic.preferences = { ...traffic.preferences, layout: "stacked", listSplit: 30 };
    await traffic.selectTraffic(row, new MouseEvent("click"));
    traffic.responseMessage.showMode("body");
  }, argument("--trace"));
  const response = page.locator('message-inspector[side="response"]');
  await response
    .getByRole("combobox", { name: "Body viewer", exact: true })
    .selectOption({ label: "Native protocol" });
  const visualizer = response.locator('extension-visualizer[slot$="body"]');
  await visualizer.getByText("Synthetic protocol", { exact: true }).first().waitFor();
  await visualizer.locator(".visualizer-debug-options>summary").click();
  await visualizer.getByRole("button", { name: "Debug fresh fixture", exact: true }).click();
  panel = visualizer.locator("script-debugger");
  pause = await paused(panel);
  await continueExecution(panel);
  for (let index = 0; index < 2; index++) {
    pause = await paused(panel, pause);
    await localSeven(panel);
    await continueExecution(panel);
  }
  await waitFor(
    () => visualizer.evaluate((element) => !element.visualizerBusy && !!element.view),
    "debugged visualizer continuation result",
  );
  await panel.getByRole("button", { name: "Detach", exact: true }).click();
  await waitFor(
    () => panel.evaluate((element) => element.closed && element.detached),
    "visualizer idle detach",
  );
  await closeDebugger(panel);
  await visualizer.getByRole("button", { name: "Attach to this instance", exact: true }).click();
  panel = visualizer.locator("script-debugger");
  await waitFor(() => panel.evaluate((element) => element.attached), "existing parser attachment");
  const expansion = visualizer.evaluate((element) => element.toggleNode("body-text"));
  await paused(panel);
  await localSeven(panel);
  await continueExecution(panel);
  await expansion;
  assert.equal(
    await visualizer.evaluate((element) => element.view.document.roots[0].children[0].value.value),
    "9007199254740993",
  );
  await panel.getByRole("button", { name: /^Stop/ }).click();
  await visualizer
    .getByText("Debugger stopped. Open the visualizer to start a fresh instance.", { exact: true })
    .waitFor();
  evidence.visualizer = {
    initializer: true,
    inputContinuation: true,
    existingHeapExpansion: true,
    stopFrozenResult: true,
  };
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    await shell.activateView("automation");
    await shell.automation.showAutomationSection("scripts");
  });
  await editor.evaluate((element) =>
    element.sourceEditor.setValue(
      "import {pass} from 'transmog:api';export function onRequestHead(){const local=7;debugger;console.info('soak');return pass()}",
    ),
  );
  const samples = [];
  for (let iteration = 0; iteration < 10; iteration++) {
    await editor.getByRole("button", { name: "Debug test", exact: true }).click();
    panel = editor.locator("script-debugger");
    await paused(panel);
    await localSeven(panel);
    if (iteration % 2) {
      await panel.getByRole("button", { name: /^Stop/ }).click();
      await waitFor(() => panel.evaluate((element) => element.closed), "native stop");
    } else {
      await continueExecution(panel);
      await panel.getByText("Callback completed", { exact: true }).waitFor();
    }
    await closeDebugger(panel);
    if (iteration === 1 || iteration === 9) {
      await new Promise((done) => setTimeout(done, 300));
      samples.push(resources());
    }
  }
  const [warm, final] = samples;
  assert.equal(final.workers, warm.workers);
  assert.ok(final.handles <= warm.handles + 32, JSON.stringify(samples));
  assert.ok(final.threads <= warm.threads + 8, JSON.stringify(samples));
  assert.ok(final.privateBytes <= warm.privateBytes + 96 * 1024 * 1024, JSON.stringify(samples));
  await editor.getByRole("button", { name: "Debug test", exact: true }).click();
  panel = editor.locator("script-debugger");
  await paused(panel);
  const oldSession = await panel.evaluate((element) => element.sessionInfo.session);
  await page.reload();
  await waitFor(
    () => page.evaluate(() => !!document.querySelector("app-shell")?.settings),
    "reloaded desktop document",
  );
  await assert.rejects(() => invoke("debug_session", { session: oldSession, after: 0 }));
  await waitFor(() => resources().workers === final.workers, "reload releases paused worker");
  evidence.lifecycle = {
    cycles: 10,
    alternatingContinueStop: true,
    windowDocumentReloadReleasesOwner: true,
    warm,
    final,
  };
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(argument("--output"), "script-debugger-native.json"),
    JSON.stringify(evidence, null, 2) + "\n",
  );
} catch (error) {
  await page.screenshot({
    path: resolve(argument("--output"), "script-debugger-native-failure.png"),
  });
  await writeFile(
    resolve(argument("--output"), "script-debugger-native-failure.json"),
    JSON.stringify(
      {
        message: error.message,
        evidence,
        editor: await page.locator("script-editor").evaluate((element) => ({
          status: element.scriptText,
          authoring: element.authoringStatus,
          issues: element.typeScriptIssues,
        })),
        panels: await page.locator("script-debugger").allTextContents(),
      },
      null,
      2,
    ) + "\n",
  );
  throw error;
} finally {
  await invoke("stop_application").catch(() => {});
  if (origin) {
    origin.closeAllConnections();
    await new Promise((done) => origin.close(done));
  }
  await browser.close();
}
