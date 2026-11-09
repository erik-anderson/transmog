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
      "import {pass} from 'transmog:api';\nlet count=0;\nexport function onRequestHead(event:any){\n const local=7;console.log('workspace callback');\n if(event.request.path==='/workspace')debugger;\n count++;Reflect.set(globalThis, 'workspaceCount',count);return pass();\n}",
    );
    element.scriptForm.elements.namedItem("scriptHost").value = "127.0.0.1";
    element.scriptForm.elements.namedItem("scriptPath").value = "";
    element.scriptForm.elements.namedItem("scriptMethod").value = "";
    element.markScriptChanged();
  });
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
    () => surface.evaluate((element) => !element.debuggerTransition).catch(() => false),
    "empty debugger ready",
  );
  assert.equal(await surface.evaluate((element) => element.debuggerTargets.length), 0);
  await surface.getByRole("button", { name: "Pause next callback (F8)", exact: true }).click();
  await surface.getByRole("button", { name: "Cancel callback pause (F8)", exact: true }).waitFor();
  await editor.getByRole("button", { name: /^Save & (?:apply|enable)$/ }).click();
  await wait(
    () => editor.evaluate((element) => !element.scriptBusy && !element.scriptUnapplied),
    "applied workspace fixture",
  );
  await page.evaluate(async () => document.querySelector("app-shell").activateView("traffic"));
  const checkView = async (panel, window, label) => {
    await panel.getByRole("button", { name: "View", exact: true }).click();
    const menu = panel.locator("#debugger-view");
    for (const [name, selector] of [
      ["Code", ".source-pane"],
      ["Sources", ".sources"],
      ["Inspector", ".variables"],
      ["Console Esc", ".console"],
    ]) {
      const item = menu.getByRole("button", { name, exact: true });
      if (!(await item.count())) continue;
      const visible = await panel.locator(selector).isVisible();
      assert.equal(await item.getAttribute("aria-pressed"), String(visible), name);
      assert.equal(await item.locator(".view-check").isVisible(), visible, name);
    }
    await window.screenshot({ path: resolve(output, `debugger-view-${label}-native.png`) });
    await menu.getByRole("button").first().press("Escape");
    assert.equal(await menu.isVisible(), false);
  };
  await wait(
    () =>
      surface
        .evaluate((element) => !element.debuggerTransition && element.debuggerTargets.length > 0)
        .catch(() => false),
    "attached editor",
  );
  await checkView(surface, detached, "window");
  await surface
    .getByRole("combobox", { name: "Console context", exact: true })
    .selectOption("worker");
  assert.equal(
    await surface.getByRole("button", { name: "Reset scratch", exact: true }).count(),
    0,
  );
  await assert.rejects(() =>
    surface.evaluate((element) =>
      window.__TAURI_INTERNALS__.invoke("reset_scratch_console", {
        session: element.debuggerTarget,
      }),
    ),
  );
  await assert.rejects(() => invoke("debugger_workspace"));
  for (let callback = 0; callback < 2; callback++) {
    if (callback > 0)
      await surface.getByRole("button", { name: "Pause next callback (F8)", exact: true }).click();
    await wait(
      () =>
        surface.evaluate(
          (element) => element.debuggerPauseNext && !element.debuggerControlSubmitting,
        ),
      "scheduled callback pause",
    );
    const response = proxyGet(listener, `${base}/scheduled-${callback}`);
    let outcome = "pending";
    void response.then(
      (result) => {
        outcome = result;
      },
      (error) => {
        outcome = String(error);
      },
    );
    try {
      await wait(
        () => surface.evaluate((element) => element.debuggerPaused),
        `scheduled callback ${callback + 1} hit`,
      );
    } catch (error) {
      throw new Error(`${error.message}: ${JSON.stringify(outcome)}`);
    }
    await surface.locator(".status").getByText("next callback", { exact: true }).waitFor();
    assert.equal(
      await surface.getByRole("combobox", { name: "Pause target", exact: true }).inputValue(),
      "any",
    );
    await surface.getByRole("button", { name: "Continue (F8)", exact: true }).click();
    assert.equal((await response).status, 200);
    await wait(
      () => surface.evaluate((element) => !element.debuggerPaused && !element.debuggerPauseNext),
      "one-shot pause consumed",
    );
  }
  assert.equal(
    await surface.getByRole("combobox", { name: "Debugger worker", exact: true }).inputValue(),
    await surface.evaluate((element) => element.debuggerTarget),
  );
  for (const [action, title, escape] of [
    [/^Open source/u, "Open source", true],
    [/^Search sources/u, "Search sources", false],
  ]) {
    await surface.getByRole("button", { name: "View", exact: true }).click();
    await surface.locator("#debugger-view").getByRole("button", { name: action }).click();
    const dialog = surface.getByRole("dialog", { name: title, exact: true });
    await dialog.waitFor();
    assert.equal(await surface.locator("#debugger-view").isVisible(), false);
    await detached.screenshot({
      path: resolve(
        output,
        title === "Open source"
          ? "debugger-open-source-menu-native.png"
          : "debugger-search-sources-menu-native.png",
      ),
    });
    if (escape)
      await surface.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
    else await dialog.getByRole("button", { name: "Close source navigation", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.ok(
      await surface
        .getByRole("button", { name: "View", exact: true })
        .evaluate((element) => element.getRootNode().activeElement === element),
    );
  }
  const navigationPrompt = surface.getByRole("textbox", {
    name: "Immediate expression",
    exact: true,
  });
  await navigationPrompt.press("Control+p");
  await surface.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
  assert.ok(
    await navigationPrompt.evaluate((element) => element.getRootNode().activeElement === element),
  );
  const paused = proxyGet(listener, `${base}/workspace`);
  await wait(() => surface.evaluate((element) => element.debuggerPaused), "normal traffic pause");
  await surface.locator(".status").getByText("Paused in onRequestHead", { exact: true }).waitFor();
  await surface.evaluate((element) => element.editor.focus());
  await detached.keyboard.press("Control+p");
  const sourceQuery = surface.getByRole("textbox", { name: "Source query", exact: true });
  const queryPosition = await sourceQuery.boundingBox();
  await sourceQuery.fill(":1:1");
  assert.equal((await sourceQuery.boundingBox()).y, queryPosition.y);
  await sourceQuery.press("Enter");
  assert.equal(await surface.evaluate((element) => element.editor.getPosition().lineNumber), 1);
  assert.equal(await surface.evaluate((element) => element.editor.getPosition().column), 1);
  const pausedRequest = await surface.evaluate(
    (element) => element.views.get(element.debuggerTarget)?.request,
  );
  assert.ok(pausedRequest?.url.endsWith("/workspace"), JSON.stringify(pausedRequest));
  const pauseIdentity = await surface.evaluate((element) => ({
    session: element.debuggerTarget,
    pause: element.pause,
  }));
  await assert.rejects(() => invoke("workspace_debug_request", pauseIdentity));
  assert.equal(
    await surface.evaluate(async (element) => {
      try {
        await window.__TAURI_INTERNALS__.invoke("workspace_debug_request", {
          session: element.debuggerTarget,
          pause: element.pause + 1,
        });
        return false;
      } catch {
        return true;
      }
    }),
    true,
    "Request navigation rejects a stale pause",
  );
  await surface.getByRole("button", { name: "Show request", exact: true }).click();
  await wait(
    () =>
      page
        .locator("traffic-workspace")
        .evaluate((element, id) => element.selectedSessionId === id, pausedRequest.id),
    "paused request selected in owning Traffic window",
  );
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
  await surface.getByRole("textbox", { name: "Immediate expression" }).fill('""');
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  await surface.locator(".command > pre").getByText('""', { exact: true }).waitFor();
  await surface
    .getByRole("textbox", { name: "Immediate expression" })
    .fill(
      'console.warn("native snapshot", {headers:{accept:"application/json"}}); console.debug("native verbose")',
    );
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  const snapshot = surface
    .locator(".command")
    .filter({ hasText: "native snapshot" })
    .filter({ has: detached.locator("console-values") });
  await snapshot.locator("console-values .snapshot-toggle").first().click();
  await snapshot.locator("console-values .snapshot-toggle").filter({ hasText: "headers" }).click();
  await snapshot.getByText("application/json", { exact: true }).click({ button: "right" });
  await detached.getByRole("menuitem", { name: "Copy value", exact: true }).press("Escape");
  assert.equal(await surface.getByText("native verbose", { exact: true }).count(), 0);
  const inspector = surface.getByRole("complementary", { name: "Call stack and variables" });
  await inspector.locator(".call-stack button").filter({ hasText: "onRequestHead" }).waitFor();
  await inspector.getByRole("textbox", { name: "Watch expression" }).fill("local");
  await inspector.getByRole("button", { name: "Add watch", exact: true }).click();
  await inspector.locator(".watch").getByText("7", { exact: true }).waitFor();
  await inspector.locator(".call-stack button").first().press("Shift+F10");
  await detached.getByRole("menuitem", { name: "Copy stack trace", exact: true }).press("Escape");
  await wait(
    () =>
      surface.evaluate((element) =>
        element.shadowRoot.activeElement?.matches(".call-stack button"),
      ),
    "stack menu restores frame focus",
  );
  await inspector.getByRole("button", { name: "Edit watch local", exact: true }).click();
  const watchEdit = inspector.getByRole("textbox", { name: "Edit watch expression", exact: true });
  await watchEdit.fill("(\nlocal + 1\n)");
  await watchEdit.press("Enter");
  await inspector.locator(".watch").getByText("8", { exact: true }).waitFor();
  await inspector.getByRole("button", { name: "Edit watch ( local + 1 )", exact: true }).click();
  await watchEdit.fill("local");
  await watchEdit.press("Enter");
  await inspector.locator(".watch").getByText("7", { exact: true }).waitFor();
  const prompt = surface.getByRole("textbox", { name: "Immediate expression" });
  await prompt.fill("loc");
  await prompt.press("Control+Space");
  await surface.getByRole("option", { name: "local", exact: true }).waitFor();
  await prompt.press("Tab");
  assert.equal(await prompt.inputValue(), "local");
  await prompt.fill("({nested:{answer:42},get secret(){throw Error('getter ran')}})");
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  const object = surface
    .locator(".command")
    .filter({ has: detached.locator("debug-value") })
    .last();
  await object.locator("debug-value").getByRole("button", { name: "Result", exact: false }).click();
  await object
    .getByText("Accessor or unavailable property; not evaluated", { exact: true })
    .waitFor();
  await object.getByRole("button", { name: "nested", exact: false }).click();
  await object.getByText("answer", { exact: true }).waitFor();
  const skip = surface.getByRole("button", { name: "Skip all pauses", exact: true });
  await skip.click();
  await wait(
    () =>
      surface.evaluate(
        (element) => element.views.get(element.debuggerTarget)?.stream.skipPauses === true,
      ),
    "skip pauses acknowledged",
  );
  assert.equal(await surface.evaluate((element) => element.debuggerPaused), true);
  await surface.getByRole("button", { name: "Skip all pauses", pressed: true }).click();
  await wait(
    () =>
      surface.evaluate(
        (element) => element.views.get(element.debuggerTarget)?.stream.skipPauses === false,
      ),
    "pauses enabled",
  );
  await surface.evaluate((element) => element.condition(6, "log"));
  await surface
    .getByRole("textbox", { name: "Log expression", exact: true })
    .fill('"workspace local: " + local');
  await surface.getByRole("button", { name: "Save", exact: true }).click();
  try {
    await surface.locator(".debug-logpoint").waitFor({ timeout: 10000 });
  } catch {
    throw new Error(
      "Native logpoint did not bind: " +
        JSON.stringify(
          await surface.evaluate((element) => ({
            error: element.debuggerError,
            selected: element.selected,
            points: element.points,
            positions: [...element.positions],
            inventory: element.views.get(element.debuggerTarget)?.stream.inventory,
          })),
        ),
    );
  }
  const beforeContinue = await surface.evaluate((element) => element.pause);
  await surface.evaluate((element) => element.continueTo(6));
  await wait(
    () =>
      surface.evaluate(
        (element, before) => element.debuggerPaused && element.pause > before,
        beforeContinue,
      ),
    "temporary continue to location",
  );
  await surface.locator(".command").getByText("workspace local: 7", { exact: true }).waitFor();
  await surface
    .locator(".command")
    .getByText("workspace callback", { exact: true })
    .first()
    .waitFor();
  await surface.getByRole("button", { name: "Clear output", exact: true }).click();
  await prompt.fill("Array.from({length:150},(_,i)=>({i}))");
  await surface.getByRole("button", { name: "Run", exact: true }).click();
  const arrayResult = surface
    .locator(".command")
    .filter({ has: detached.locator("debug-value") })
    .last();
  await arrayResult
    .locator("debug-value")
    .getByRole("button", { name: "Result", exact: false })
    .click();
  await arrayResult.getByRole("button", { name: /\b0$/u }).waitFor();
  await surface.evaluate((element) => {
    element.consoleOutput.scrollTop = element.consoleOutput.scrollHeight;
  });
  await arrayResult.getByText("length", { exact: true }).waitFor();
  assert.equal(await arrayResult.getByText(/value handle limit reached/u).count(), 0);
  assert.equal(await arrayResult.getByRole("button", { name: /\b0$/u }).count(), 1);
  await surface.getByRole("textbox", { name: "Immediate expression" }).fill("preserved draft");
  await surface.getByRole("textbox", { name: "Immediate expression" }).press("ArrowUp");
  await surface
    .getByRole("combobox", { name: "Console output", exact: true })
    .selectOption("selected");
  await surface.getByRole("button", { name: "Console levels", exact: true }).click();
  await surface.getByRole("checkbox", { name: "Info", exact: true }).uncheck();
  await surface.getByRole("checkbox", { name: "Errors", exact: true }).uncheck();
  await surface.getByRole("checkbox", { name: "Warnings", exact: true }).press("Escape");
  await page.evaluate(() => {
    const prototype = customElements.get("debugger-workspace").prototype;
    const original = prototype.start;
    prototype.start = function () {
      const destination = this;
      return new Promise((resolve, reject) => {
        globalThis.__finishDebuggerHandoff = () => {
          prototype.start = original;
          delete globalThis.__finishDebuggerHandoff;
          original.call(destination).then(resolve, reject);
        };
      });
    };
  });
  await surface.evaluate((element) => element.place("bottom"));
  await page.waitForFunction(() => !!globalThis.__finishDebuggerHandoff);
  assert.equal(await surface.locator(".workspace").evaluate((element) => element.inert), true);
  await detached.keyboard.type("must not replace the draft");
  assert.equal(
    await surface.evaluate((element) => element.debuggerInput),
    "Array.from({length:150},(_,i)=>({i}))",
  );
  // Cancel from the destination before it acknowledges ownership, then retry.
  await invoke("place_debugger", { placement: "closed" });
  await wait(
    () => surface.evaluate((element) => !element.debuggerTransition && element.debuggerPaused),
    "restored source after canceled handoff",
  );
  await page.evaluate(() => globalThis.__finishDebuggerHandoff());
  const dock = page.locator("debugger-workspace");
  await dock.waitFor({ state: "detached" });
  assert.equal(
    await surface.getByRole("textbox", { name: "Immediate expression" }).inputValue(),
    "Array.from({length:150},(_,i)=>({i}))",
  );
  await surface.evaluate((element) => element.place("bottom"));
  await wait(
    () =>
      dock
        .evaluate((element) => !element.debuggerTransition && element.debuggerPaused)
        .catch(() => false),
    "paused dock",
  );
  assert.equal(
    await dock.getByRole("textbox", { name: "Immediate expression" }).inputValue(),
    "Array.from({length:150},(_,i)=>({i}))",
  );
  const dockButton = dock.getByRole("button", { name: "Dock", exact: true });
  assert.equal(
    await dock.getByRole("combobox", { name: "Debugger worker", exact: true }).inputValue(),
    await dock.evaluate((element) => element.debuggerTarget),
  );
  const consoleVisible = await dock.locator(".console").isVisible();
  await dockButton.click();
  const dockMenu = dock.locator("#debugger-placement");
  const triggerBox = await dockButton.boundingBox();
  const menuBox = await dockMenu.boundingBox();
  assert.ok(
    Math.min(
      Math.abs(menuBox.y - triggerBox.y - triggerBox.height),
      Math.abs(triggerBox.y - menuBox.y - menuBox.height),
    ) <= 12,
  );
  assert.ok(
    menuBox.x < triggerBox.x + triggerBox.width && menuBox.x + menuBox.width > triggerBox.x,
  );
  await page.screenshot({ path: resolve(output, "debugger-dock-menu-native.png") });
  await dockMenu.getByRole("button").first().focus();
  await page.keyboard.down("Escape");
  await dockMenu.waitFor({ state: "hidden" });
  await page.keyboard.down("Escape");
  await page.keyboard.up("Escape");
  assert.ok(
    await dockButton.evaluate((element) => element.getRootNode().activeElement === element),
  );
  assert.equal(await dock.locator(".console").isVisible(), consoleVisible);
  await dock.getByRole("textbox", { name: "Immediate expression" }).press("ArrowDown");
  assert.equal(
    await dock.getByRole("textbox", { name: "Immediate expression" }).inputValue(),
    "preserved draft",
  );
  assert.equal(
    await dock.getByRole("combobox", { name: "Console output", exact: true }).inputValue(),
    "selected",
  );
  assert.equal(
    await dock.getByRole("button", { name: "Console levels", exact: true }).textContent(),
    "Warnings",
  );
  await dock.getByRole("button", { name: "Console levels", exact: true }).click();
  await dock.getByRole("checkbox", { name: "Info", exact: true }).check();
  await dock.getByRole("checkbox", { name: "Errors", exact: true }).check();
  await dock.getByRole("checkbox", { name: "Info", exact: true }).press("Escape");
  assert.equal(
    await dock.locator(".command").getByText("workspace callback", { exact: true }).count(),
    0,
  );

  assert.equal(
    await dock.locator(".command").getByText("workspace local: 7", { exact: true }).count(),
    0,
    "Moving the debugger must not replay cleared logpoint output",
  );
  await dock.locator(".watch").getByText("7", { exact: true }).waitFor();
  await dock
    .getByRole("complementary", { name: "Call stack and variables" })
    .locator("details")
    .filter({ has: page.getByText("Scopes", { exact: true }) })
    .locator("debug-value")
    .filter({ hasText: "local" })
    .getByText("7", { exact: true })
    .waitFor();
  const keptResult = dock
    .locator(".command")
    .filter({ has: page.locator("debug-value") })
    .last();
  await keptResult
    .locator("debug-value")
    .getByRole("button", { name: "Result", exact: false })
    .click();
  await keptResult.getByRole("button", { name: "0", exact: false }).first().waitFor();
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
        .evaluate(
          (element) =>
            !element.debuggerTransition && !!element.editor && element.debuggerTargets.length > 0,
        )
        .catch(() => false),
    "reopened idle worker",
  );
  await dock.getByRole("button", { name: "Edit watch local", exact: true }).waitFor();
  await dock.locator(".watch").getByText("local: Not paused", { exact: true }).waitFor();
  await dock
    .getByRole("textbox", { name: "Immediate expression" })
    .fill("Reflect.get(globalThis, 'workspaceCount')");
  await dock.getByRole("button", { name: "Run", exact: true }).click();
  await dock.locator(".command > pre").filter({ hasText: "5" }).waitFor();
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
  await wait(
    () => detached.locator("debugger-workspace").evaluate((element) => !element.debuggerTransition),
    "ready undocked worker",
  );
  await page.evaluate(() => {
    const prototype = customElements.get("debugger-workspace").prototype;
    const original = prototype.start;
    prototype.start = function () {
      const destination = this;
      return new Promise((resolve, reject) => {
        globalThis.__finishDebuggerHandoff = () => {
          prototype.start = original;
          delete globalThis.__finishDebuggerHandoff;
          original.call(destination).then(resolve, reject);
        };
      });
    };
  });
  try {
    await detached.locator("debugger-workspace").evaluate((element) => element.place("bottom"));
    await page.waitForFunction(() => !!globalThis.__finishDebuggerHandoff);
    await page.bringToFront();
    await page.keyboard.press("F12");
    await dock.waitFor({ state: "detached" });
    await wait(() => detached.isClosed(), "F12 closes both surfaces during handoff");
    await assert.rejects(() => invoke("acknowledge_debugger"));
  } finally {
    await page.evaluate(() => globalThis.__finishDebuggerHandoff?.());
  }
  assert.equal((await proxyGet(listener, `${base}/moving-close`)).status, 200);
  // Close destinations before editor readiness, then reopen under a fresh owner.
  for (let cycle = 0; cycle < 3; cycle++) {
    await invoke("place_debugger", { placement: "window" });
    await invoke("place_debugger", { placement: "closed" });
    await invoke("place_debugger", { placement: "bottom" });
    await wait(
      () =>
        dock
          .evaluate(
            (element) =>
              !element.debuggerTransition && !!element.editor && element.debuggerTargets.length > 0,
          )
          .catch(() => false),
      "rapid reopen",
    );
    assert.equal((await invoke("debugger_workspace")).open, true);
    await dock.getByRole("button", { name: "Close debugger and continue execution" }).click();
  }
  await invoke("place_debugger", { placement: "bottom" });
  await dock.locator(".monaco-editor").waitFor();
  await wait(
    () => dock.evaluate((element) => !element.debuggerTransition),
    "ready layout controls",
  );
  await dock.getByRole("separator", { name: "Resize sources", exact: true }).press("ArrowRight");
  assert.equal(await dock.evaluate((element) => element.debuggerSourcesSplit), "20");
  await dock.getByRole("button", { name: "Dock", exact: true }).click();
  await dock.getByRole("button", { name: "Dock to right", exact: true }).click();
  const outerDivider = page.getByRole("separator", { name: "Resize debugger", exact: true });
  assert.equal(await outerDivider.getAttribute("aria-orientation"), "vertical");
  assert.equal(
    await outerDivider.evaluate((element) => getComputedStyle(element).cursor),
    "col-resize",
  );
  const beforeDrag = await dock.boundingBox();
  const handle = await outerDivider.boundingBox();
  const x = handle.x + handle.width / 2,
    y = handle.y + handle.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x - 40, y, { steps: 5 });
  await page.mouse.up();
  const growth = (await dock.boundingBox()).width - beforeDrag.width;
  assert.ok(growth >= 20 && growth <= 60, `Right dock follows horizontal dragging, grew ${growth}`);
  await outerDivider.press("Home");
  for (let step = 0; step < 5; step++) await outerDivider.press("Shift+ArrowRight");
  const minimum = await dock.boundingBox();
  const limit = await outerDivider.boundingBox();
  await page.screenshot({ path: resolve(output, "debugger-right-minimum-native.png") });
  await checkView(dock, page, "compact");
  const limitX = limit.x + limit.width / 2,
    limitY = limit.y + limit.height / 2;
  await page.mouse.move(limitX, limitY);
  await page.mouse.down();
  await page.mouse.move(limitX - 30, limitY, { steps: 5 });
  await page.mouse.up();
  assert.ok(
    (await dock.boundingBox()).width - minimum.width > 15,
    "Dragging away from the minimum dock width responds immediately",
  );
  await outerDivider.press("Home");
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
  await wait(
    () => dock.evaluate((element) => !element.debuggerTransition),
    "ready restored layout",
  );
  assert.equal(await dock.evaluate((element) => element.debuggerSourcesSplit), "20");
  assert.equal(await dock.evaluate((element) => element.debuggerConsoleVisible), false);
  await dock.evaluate((element) => element.toggleConsole());
  await dock.getByRole("button", { name: "Close debugger and continue execution" }).click();
  return {
    resizeLimits: true,
    workerSelection: true,
    menuFocus: true,
    navigationMenuActions: true,
    dockingPointerAxes: true,
    anchoredMenus: true,
    pauseNextRearmed: true,
    sourceLocations: true,
    closeDuringHandoff: true,
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
