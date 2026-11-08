import assert from "node:assert/strict";
import { testInSandbox } from "./check-script-authoring.mjs";
import { checkNativeDebuggerWorkspace } from "./smoke-debugger-workspace.mjs";
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
    () =>
      panel
        .evaluate(
          (element, previous) =>
            !element.debuggerTransition && element.debuggerPaused && element.pause > previous,
          after,
        )
        .catch(() => false),
    "native engine pause",
    20000,
  );
  return panel.evaluate((element) => element.pause);
}
async function continueExecution(panel) {
  await panel.getByRole("button", { name: "Continue (F8)", exact: true }).click();
}
async function surface() {
  let panel;
  await waitFor(
    async () => {
      const active = browser
        .contexts()
        .flatMap((context) => context.pages())
        .find((candidate) => candidate !== page && !candidate.isClosed());
      const candidate = (active ?? page).locator("debugger-workspace");
      if (await candidate.isVisible().catch(() => false)) {
        panel = candidate;
        return true;
      }
      return false;
    },
    "debugger window",
    20000,
  );
  await panel.locator(".monaco-editor").waitFor();
  await waitFor(() => panel.evaluate((element) => !element.debuggerTransition), "ready debugger");
  return panel;
}
async function closeDebugger(panel) {
  await panel.getByRole("button", { name: "Close debugger and continue execution" }).click();
}
const invoke = (command, args = {}) =>
  page.evaluate(
    ([command, args]) => window.__TAURI_INTERNALS__.invoke(command, args),
    [command, args],
  );
function proxyGet(listener, url) {
  const [hostname, port] = listener.split(":");
  const operation = new Promise((done, fail) => {
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
    // Inspection can keep a healthy worker paused longer than an ordinary request.
    request.setTimeout(120000, () => request.destroy(new Error("Loopback client timed out")));
    request.on("error", fail);
    request.end();
  });
  // A selected exchange intentionally waits while the UI is paused. Keep its
  // rejection available to the later await without bypassing failure screenshots.
  void operation.catch(() => {});
  return operation;
}
try {
  await page.waitForFunction(() => !!document.querySelector("app-shell")?.settings);
  await page.locator("app-shell a[data-view='automation']").click();
  await page
    .locator("automation-workspace .workspace-tabs")
    .getByRole("button", { name: "Scripts", exact: true })
    .click();
  const editor = page.locator("script-editor");
  await waitFor(() => editor.evaluate((element) => element.scriptReady), "script editor");
  for (const [triggerName, firstCommand, filename] of [
    ["TypeScript source options", "Validate only", "scripts-source-options.png"],
    ["Debug", "Open debugger for live traffic", "scripts-debug-menu.png"],
  ]) {
    const trigger = editor.getByRole("button", { name: triggerName, exact: true });
    await trigger.press("Enter");
    const command = editor.getByRole("button", { name: firstCommand, exact: true });
    await waitFor(() => command.evaluate((button) => button.matches(":focus")), triggerName);
    await page.screenshot({ path: resolve(argument("--output"), filename) });
    await page.keyboard.press("Escape");
    assert.equal(await trigger.evaluate((button) => button.matches(":focus")), true);
  }
  const debuggerButton = page.getByRole("button", { name: "Debugger", exact: true });
  assert.equal(
    await debuggerButton.count(),
    0,
    "A new app session must start without a top debugger shortcut",
  );
  await editor.evaluate((element) => element.sourceEditor.focus());
  const processId = Number(argument("--process-id"));
  assert.ok(Number.isSafeInteger(processId) && processId > 0);
  // Exercise WebView2's native accelerator, including on a headless input desktop.
  evidence.nativeKey = JSON.parse(
    execFileSync(
      "pwsh.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-File",
        resolve("../../../scripts/send-desktop-probe-key.ps1"),
        "-ProbeProcessId",
        String(processId),
      ],
      { encoding: "utf8", windowsHide: true },
    ),
  );
  const initialDebugger = await surface();
  await debuggerButton.waitFor();
  await initialDebugger.evaluate((element) => element.editor.focus());
  await initialDebugger.page().keyboard.press("F12");
  await waitFor(() => initialDebugger.page().isClosed(), "F12 closes the debugger window");
  await debuggerButton.waitFor();
  await editor.getByRole("button", { name: "Debug", exact: true }).click();
  await editor.getByRole("button", { name: "Open debugger for live traffic", exact: true }).click();
  const liveDebugger = await surface();
  await closeDebugger(liveDebugger);
  await waitFor(() => liveDebugger.page().isClosed(), "live debugger closes");
  await editor.evaluate((element) =>
    element.sourceEditor.setValue(
      [
        "import {pass} from 'transmog:api';",
        "console.info('native initializer');",
        "export function onRequestHead(){",
        " const local=7;",
        " const object={nested:{value:42},get unsafe(){throw new Error('getter must not run')}};",
        " debugger;",
        " console.log('native callback',local);return pass();",
        "}",
      ].join("\n"),
    ),
  );
  await testInSandbox(editor);
  await editor.locator(".script-test-summary").filter({ hasText: "Callback completed" }).waitFor();
  assert.equal(await page.locator("debugger-workspace:visible").count(), 0);
  assert.equal(browser.contexts()[0].pages().length, 1);
  await editor.getByRole("button", { name: "Debug", exact: true }).click();
  await editor.getByRole("button", { name: "Run sandbox test in debugger", exact: true }).click();
  let panel = await surface();
  let pause = await paused(panel);
  await panel.getByRole("combobox", { name: "Pause on exceptions", exact: true }).focus();
  await panel.page().keyboard.press("F8");
  pause = await paused(panel, pause);
  await panel.getByRole("cell", { name: "7", exact: true }).waitFor();
  await panel.getByRole("button", { name: "object", exact: true }).click();
  await panel.getByRole("cell", { name: /Accessor/ }).waitFor();
  await panel.getByRole("button", { name: "nested", exact: true }).click();
  await panel.getByRole("cell", { name: "42", exact: true }).waitFor();
  await panel.getByRole("textbox", { name: "Immediate expression" }).fill("local");
  await panel.getByRole("button", { name: "Run", exact: true }).click();
  await panel.locator(".command > pre").filter({ hasText: "7" }).waitFor();
  const first = await panel.evaluate((element) => element.debuggerTarget);
  await panel.getByRole("button", { name: "Restart", exact: true }).click();
  await waitFor(
    () =>
      panel.evaluate(
        (element, old) => element.debuggerTarget !== old && element.debuggerPaused,
        first,
      ),
    "fixture restart",
  );
  await panel.getByRole("textbox", { name: "Immediate expression" }).press("Shift+F5");
  await waitFor(() => panel.evaluate((element) => element.debuggerClosed), "fixture stop");
  await closeDebugger(panel);
  evidence.sandbox = {
    ordinaryTestCompleted: true,
    firstStatement: true,
    callback: true,
    nestedValues: true,
    gettersInert: true,
    immediate: true,
    restart: true,
    stopShortcut: true,
  };

  const state = await invoke("product_state");
  state.preferences.configureSystemProxy = false;
  state.preferences.decryptHttps = false;
  state.preferences.proxyPort = null;
  await invoke("save_product_state", { productState: state });
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
  origin = createServer((request, response) => {
    response.writeHead(200, { "Content-Type": "text/plain" });
    response.end("native origin");
  });
  await new Promise((done) => origin.listen(0, "127.0.0.1", done));
  const base = `http://127.0.0.1:${origin.address().port}`;
  evidence.workspace = await checkNativeDebuggerWorkspace({
    page,
    browser,
    invoke,
    proxyGet,
    listener: status.listener,
    base,
    output: argument("--output"),
  });
  await invoke("stop_application");

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
  panel = await surface();
  await paused(panel);
  const firstVisualizer = await panel.evaluate((element) => element.debuggerTarget);
  await panel.getByRole("button", { name: "Restart", exact: true }).click();
  await waitFor(
    () =>
      panel.evaluate(
        (element, old) => element.debuggerTarget !== old && element.debuggerPaused,
        firstVisualizer,
      ),
    "visualizer restart",
    20000,
  );
  // Closing resumes initializer and every authored debugger statement, retaining the parser.
  await closeDebugger(panel);
  await waitFor(
    () => visualizer.evaluate((element) => !element.visualizerBusy && !!element.view),
    "visualizer completion",
    20000,
  );
  await visualizer.getByRole("button", { name: "Debug this instance", exact: true }).click();
  panel = await surface();
  await waitFor(
    () => panel.evaluate((element) => element.debuggerTargets.length > 0),
    "existing visualizer attachment",
  );
  const expansion = visualizer.evaluate((element) => element.toggleNode("body-text"));
  await paused(panel);
  await continueExecution(panel);
  await expansion;
  await panel.getByRole("button", { name: "Stop (Shift+F5)", exact: true }).click();
  await visualizer
    .getByText("Debugger stopped. Open the visualizer to start a fresh instance.", { exact: true })
    .waitFor();
  await closeDebugger(panel);
  evidence.visualizer = {
    initialization: true,
    restart: true,
    closeContinues: true,
    existingHeap: true,
    expansion: true,
    stopFrozen: true,
  };

  await page.locator("app-shell a[data-view='automation']").click();
  await page
    .locator("automation-workspace .workspace-tabs")
    .getByRole("button", { name: "Scripts", exact: true })
    .click();
  const samples = [];
  for (let iteration = 0; iteration < 10; iteration++) {
    await editor.getByRole("button", { name: "Debug", exact: true }).click();
    await editor.getByRole("button", { name: "Run sandbox test in debugger", exact: true }).click();
    panel = await surface();
    await paused(panel);
    if (iteration % 2) {
      await panel.getByRole("button", { name: "Stop (Shift+F5)", exact: true }).click();
      await waitFor(() => panel.evaluate((element) => element.debuggerClosed), "cycle stop");
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
  evidence.lifecycle = { cycles: 10, warm, final };
  await page.reload();
  await page.getByRole("button", { name: "Debugger", exact: true }).waitFor();
  evidence.access = {
    initiallyHidden: true,
    F12FromMonaco: true,
    F12InDetachedWindow: true,
    retainedForSession: true,
  };
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(argument("--output"), "script-debugger-native.json"),
    JSON.stringify(evidence, null, 2) + "\n",
  );
} catch (error) {
  const runtime = await invoke("script_runtime_status").catch(String);
  const exchangeIds = Array.isArray(runtime)
    ? [...new Set(runtime.flatMap((script) => script.logs.map((run) => run.context.exchangeId)))]
    : [];
  const exchanges = await Promise.all(
    exchangeIds.map(async (id) => {
      const detail = await invoke("session_detail", { id }).catch(String);
      return { id, terminal: detail.terminal, diagnostics: detail.diagnostics };
    }),
  );
  await page.screenshot({
    path: resolve(argument("--output"), "script-debugger-native-failure.png"),
  });
  await writeFile(
    resolve(argument("--output"), "script-debugger-native-failure.json"),
    JSON.stringify(
      {
        message: error.message,
        evidence,
        runtime,
        exchanges,
        editor: await page.locator("script-editor").evaluate((element) => ({
          status: element.scriptText,
          authoring: element.authoringStatus,
          issues: element.typeScriptIssues,
        })),
        surfaces: await Promise.all(
          browser
            .contexts()
            .flatMap((context) => context.pages())
            .map(async (surface) => ({
              url: surface.url(),
              shell: await surface
                .evaluate(() => {
                  const element = document.querySelector("app-shell");
                  return element
                    ? {
                        diagnostic: element.diagnosticText,
                        placement: element.debuggerPlacement,
                        docked: element.debuggerDocked,
                      }
                    : null;
                })
                .catch(String),
              panels: await surface
                .locator("debugger-workspace")
                .evaluateAll(async (elements) =>
                  Promise.all(
                    elements.map(async (element) => ({
                      error: element.debuggerError,
                      transition: element.debuggerTransition,
                      editor: !!element.editor,
                      target: element.debuggerTarget,
                      paused: element.debuggerPaused,
                      pauseNext: element.debuggerPauseNext,
                      closed: element.debuggerClosed,
                      session: element.debuggerTarget
                        ? await window.__TAURI_INTERNALS__
                            .invoke("workspace_debug_session", {
                              session: element.debuggerTarget,
                              after: 0,
                            })
                            .catch(String)
                        : null,
                      scopes: element.debuggerScopeValues.map((value) => ({
                        label: value.label,
                        expired: value.expired,
                        expanded: value.expanded,
                        loading: value.loading,
                        issue: value.issue,
                        handle: value.value.handle,
                        rows: value.rows.slice(0, 12).map((row) => ({
                          name: row.name,
                          value: row.value,
                          preview: row.preview,
                          expanded: row.expanded,
                        })),
                      })),
                      events: [...element.views.values()].flatMap((view) =>
                        view.stream.events.slice(-8).map((item) => ({
                          request: item.request,
                          kind: item.event.kind,
                          value: item.event.value,
                          message: item.event.message,
                        })),
                      ),
                    })),
                  ),
                )
                .catch(String),
            })),
        ),
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
