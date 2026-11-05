import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkNativeDebuggerWorkspace({
  page,
  browser,
  invoke,
  proxyGet,
  listener,
  base,
  output,
}) {
  const wait = async (check, label) => {
    const start = Date.now();
    while (Date.now() - start < 15000) {
      if (await check()) return;
      await new Promise((done) => setTimeout(done, 50));
    }
    throw new Error(`Debugger workspace timed out: ${label}`);
  };
  await page.locator("app-shell a[data-view='automation']").click();
  await page
    .locator("automation-workspace .workspace-tabs")
    .getByRole("button", { name: "Scripts", exact: true })
    .click();
  const editor = page.locator("script-editor");
  await wait(() => editor.evaluate((element) => element.scriptReady), "editor");
  await editor.evaluate((element) => {
    element.sourceEditor.setValue(
      "import {pass} from 'transmog:api';\nlet count=0;\nexport function onRequestHead(event:any){\n const local=7;\n if(event.request.path==='/workspace')debugger;\n count++;Reflect.set(globalThis, 'workspaceCount',count);return pass();\n}",
    );
    element.scriptForm.elements.namedItem("scriptHost").value = "127.0.0.1";
    element.scriptForm.elements.namedItem("scriptPath").value = "";
    element.scriptForm.elements.namedItem("scriptMethod").value = "";
    element.markScriptChanged();
  });
  await editor.getByRole("button", { name: /^Save & (?:apply|enable)$/ }).click();
  await wait(
    () => editor.evaluate((element) => !element.scriptBusy && !element.scriptUnapplied),
    "applied workspace fixture",
  );
  await page.evaluate(async () => document.querySelector("app-shell").activateView("traffic"));
  await page.keyboard.press("F12");
  let detached;
  await wait(() => {
    detached = browser
      .contexts()[0]
      .pages()
      .find((candidate) => candidate !== page && !candidate.isClosed());
    return !!detached;
  }, "detached window");
  const surface = detached.locator("debugger-workspace");
  await wait(
    () =>
      surface
        .evaluate((element) => !!element.editor && element.debuggerTargets.length > 0)
        .catch(() => false),
    "attached editor",
  );
  await assert.rejects(() => invoke("debugger_workspace"));
  const paused = proxyGet(listener, `${base}/workspace`);
  await wait(() => surface.evaluate((element) => element.debuggerPaused), "normal traffic pause");
  await page.getByRole("button", { name: "Debugger · 1 paused", exact: true }).waitFor();
  const gutter = await surface.evaluate((element) => {
    const editor = element.editor;
    editor.revealLineInCenter(1);
    const bounds = editor.getDomNode().getBoundingClientRect();
    const layout = editor.getLayoutInfo();
    const position = editor.getScrolledVisiblePosition({ lineNumber: 1, column: 1 });
    return {
      x: bounds.left + layout.glyphMarginLeft + layout.glyphMarginWidth / 2,
      y: bounds.top + position.top + position.height / 2,
    };
  });
  await detached.mouse.click(gutter.x, gutter.y, { button: "right" });
  await detached.getByRole("menuitem", { name: "Add or edit conditional breakpoint" }).waitFor();
  // Monaco ignores accidental mouse-up during the first 100 ms of a menu.
  await detached.waitForTimeout(150);
  await detached.getByRole("menuitem", { name: "Add or edit conditional breakpoint" }).click();
  await surface.getByRole("textbox", { name: "Breakpoint condition", exact: true }).waitFor();
  assert.match(await surface.locator(".condition-source").textContent(), /:1$/u);
  await detached.screenshot({ path: resolve(output, "debugger-native-condition.png") });
  await surface.getByRole("button", { name: "Cancel", exact: true }).click();
  await surface.getByRole("textbox", { name: "Immediate expression" }).fill("local");
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  await surface.locator(".command > pre").filter({ hasText: "7" }).waitFor();
  const inspector = surface.getByRole("complementary", { name: "Call stack and variables" });
  await inspector.locator(".call-stack button").filter({ hasText: "onRequestHead" }).waitFor();
  await inspector.getByRole("textbox", { name: "Watch expression" }).fill("local");
  await inspector.getByRole("button", { name: "Add watch", exact: true }).click();
  await inspector.locator(".watch").getByText("7", { exact: true }).waitFor();
  const prompt = surface.getByRole("textbox", { name: "Immediate expression" });
  await prompt.fill("loc");
  await prompt.press("Control+Space");
  await surface.getByRole("option", { name: "local", exact: true }).waitFor();
  await prompt.press("Tab");
  assert.equal(await prompt.inputValue(), "local");
  await prompt.fill("({nested:{answer:42},get secret(){throw Error('getter ran')}})");
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  const object = surface.locator(".command").filter({ hasText: "› ({nested:" });
  await object.locator("debug-value").getByRole("button", { name: "Result", exact: false }).click();
  await object
    .getByText("Accessor or unavailable property; not evaluated", { exact: true })
    .waitFor();
  await object.getByRole("button", { name: "nested", exact: false }).click();
  await object.getByText("answer", { exact: true }).waitFor();
  await surface.getByRole("textbox", { name: "Immediate expression" }).fill("preserved draft");
  await surface.evaluate((element) => element.place("bottom"));
  const dock = page.locator("debugger-workspace");
  await wait(
    () => dock.evaluate((element) => element.debuggerPaused).catch(() => false),
    "paused dock",
  );
  assert.equal(
    await dock.getByRole("textbox", { name: "Immediate expression" }).inputValue(),
    "preserved draft",
  );
  await dock.getByRole("combobox", { name: "Source representation" }).selectOption("javascript");
  await dock.locator(".debug-execution-line").waitFor();
  let queuedFinished = false;
  const queued = proxyGet(listener, `${base}/queued`).then((result) => {
    queuedFinished = true;
    return result;
  });
  await new Promise((done) => setTimeout(done, 200));
  assert.equal(queuedFinished, false);
  await page.screenshot({ path: resolve(output, "debugger-workspace-native-docked.png") });
  await dock.getByRole("textbox", { name: "Immediate expression" }).press("F12");
  const pausedResult = await paused;
  assert.equal(
    pausedResult.status,
    200,
    JSON.stringify({ pausedResult, runtime: await invoke("script_runtime_status") }),
  );
  const queuedResult = await queued;
  assert.equal(
    queuedResult.status,
    200,
    JSON.stringify({ queuedResult, runtime: await invoke("script_runtime_status") }),
  );
  assert.equal((await proxyGet(listener, `${base}/workspace`)).status, 200);
  await invoke("place_debugger", { placement: "bottom" });
  await wait(
    () =>
      dock
        .evaluate((element) => !!element.editor && element.debuggerTargets.length > 0)
        .catch(() => false),
    "reopened idle worker",
  );
  await dock
    .getByRole("textbox", { name: "Immediate expression" })
    .fill("Reflect.get(globalThis, 'workspaceCount')");
  await dock.getByRole("button", { name: "Run", exact: true }).click();
  await dock.locator(".command > pre").filter({ hasText: "3" }).waitFor();
  await dock.getByRole("combobox", { name: "Pause on exceptions" }).selectOption("all");
  const previousWorker = await dock.evaluate((element) => element.debuggerTarget);
  await dock.getByRole("button", { name: "Restart", exact: true }).click();
  await wait(
    () =>
      dock.evaluate(
        (element, previous) =>
          element.debuggerTarget !== previous &&
          !element.debuggerClosed &&
          element.debuggerExceptions === "all",
        previousWorker,
      ),
    "restart focus and exception policy",
  );
  await dock.evaluate((element) => element.place("window"));
  await wait(() => {
    detached = browser
      .contexts()[0]
      .pages()
      .find((candidate) => candidate !== page && !candidate.isClosed());
    return !!detached;
  }, "undocked window");
  await detached.locator("debugger-workspace .monaco-editor").waitFor();
  await detached.getByRole("button", { name: "Close debugger and continue execution" }).click();
  // Close destinations before editor readiness, then reopen under a fresh owner.
  for (let cycle = 0; cycle < 3; cycle++) {
    await invoke("place_debugger", { placement: "window" });
    await invoke("place_debugger", { placement: "closed" });
    await invoke("place_debugger", { placement: "bottom" });
    await wait(
      () =>
        dock
          .evaluate((element) => !!element.editor && element.debuggerTargets.length > 0)
          .catch(() => false),
      "rapid reopen",
    );
    assert.equal((await invoke("debugger_workspace")).open, true);
    await dock.getByRole("button", { name: "Close debugger and continue execution" }).click();
  }
  await invoke("place_debugger", { placement: "bottom" });
  await dock.locator(".monaco-editor").waitFor();
  await dock.getByRole("separator", { name: "Resize sources", exact: true }).press("ArrowRight");
  await dock.getByRole("button", { name: "Dock", exact: true }).click();
  await dock.getByRole("button", { name: "Dock to right", exact: true }).click();
  await dock.getByRole("button", { name: "Hide console (Escape)", exact: true }).click();
  await wait(
    async () => (await invoke("debugger_layout")).consoleVisible === false,
    "saved hidden console",
  );
  await page.keyboard.press("F12");
  await dock.waitFor({ state: "detached" });
  await page.keyboard.press("F12");
  await dock.locator(".monaco-editor").waitFor();
  await wait(
    () => page.locator("app-shell").evaluate((element) => element.debuggerPlacement === "right"),
    "remembered right dock",
  );
  assert.equal(await dock.evaluate((element) => element.debuggerSourcesSplit), "20");
  assert.equal(await dock.evaluate((element) => element.debuggerConsoleVisible), false);
  await dock.evaluate((element) => element.toggleConsole());
  await dock.getByRole("button", { name: "Close debugger and continue execution" }).click();
  return {
    layoutRestored: true,
    normalTrafficPauses: true,
    queuedTrafficWaits: true,
    pausedDockTransfer: true,
    consoleDraftPreserved: true,
    idleCommands: true,
    closeContinues: true,
    closedDebuggerRunsThrough: true,
    rapidCloseReopen: true,
  };
}
