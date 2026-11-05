import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkDebuggerNavigation } from "./check-debugger-navigation.mjs";
import { checkDebuggerInspection } from "./check-debugger-inspection.mjs";

export async function checkDebuggerWorkspace(page, root) {
  const shortcut = page.getByRole("button", { name: "Debugger", exact: true });
  assert.equal(await shortcut.count(), 0, "The top shortcut starts hidden");
  await page.locator("app-shell a[data-view='automation']").click();
  await page
    .locator("automation-workspace .workspace-tabs")
    .getByRole("button", { name: "Scripts", exact: true })
    .click();
  await page.locator("script-editor .monaco-editor").waitFor();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-access-scripts.png"),
  });
  await page
    .locator("script-editor")
    .getByRole("button", { name: "Open debugger", exact: true })
    .click();
  const workspace = page.locator("debugger-workspace");
  await workspace.locator(".monaco-editor").waitFor();
  await shortcut.waitFor();
  await checkDebuggerNavigation(page, workspace);
  await workspace.evaluate((element) => element.editor.focus());
  await page.keyboard.press("F12");
  await workspace.waitFor({ state: "detached" });
  await shortcut.waitFor();
  await page.locator("script-editor").evaluate((element) => element.sourceEditor.focus());
  await page.keyboard.press("F12");
  await workspace.locator(".monaco-editor").waitFor();
  await page.keyboard.down("F12");
  await workspace.waitFor({ state: "detached" });
  await page.keyboard.down("F12");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.open),
    false,
    "Holding F12 must not reopen the debugger",
  );
  await page.keyboard.up("F12");
  await page.locator("app-shell a[data-view='settings']").click();
  await page
    .locator("settings-workspace .settings-tabs")
    .getByRole("button", { name: "Extensions", exact: true })
    .click();
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-access-extensions-800.png"),
  });
  const access = await page
    .locator("extension-settings")
    .getByRole("button", { name: "Open debugger", exact: true })
    .boundingBox();
  assert.ok(access && access.x >= 0 && access.x + access.width <= 800);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page
    .locator("extension-settings")
    .getByRole("button", { name: "Open debugger", exact: true })
    .click();
  await workspace.locator(".monaco-editor").waitFor();
  await shortcut.click();
  assert.equal(
    await page.locator("app-shell").evaluate((element) => element.debuggerPlacement),
    "bottom",
    "Opening an existing debugger keeps its dock",
  );
  await page.locator("app-shell a[data-view='traffic']").click();
  const consoleDivider = workspace.getByRole("separator", { name: "Resize debugger console" });
  await consoleDivider.press("ArrowUp");
  assert.equal(await consoleDivider.getAttribute("aria-valuenow"), "68");
  await consoleDivider.press("Home");
  await workspace.getByRole("button", { name: "main.ts Loaded", exact: true }).click();
  assert.match(
    await workspace.evaluate((element) => element.model.getValue()),
    /const value: number/u,
  );
  const gutter = await workspace.evaluate((element) => {
    const editor = element.editor;
    const bounds = editor.getDomNode().getBoundingClientRect();
    const layout = editor.getLayoutInfo();
    const position = editor.getScrolledVisiblePosition({ lineNumber: 2, column: 1 });
    return {
      x: bounds.left + layout.glyphMarginLeft + layout.glyphMarginWidth / 2,
      y: bounds.top + position.top + position.height / 2,
    };
  });
  await page.mouse.click(gutter.x, gutter.y);
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.workspaceDebug.points.length === 1,
  );
  await workspace.locator(".debug-breakpoint").waitFor();
  await page.mouse.click(gutter.x, gutter.y, { button: "right" });
  await page.getByRole("menuitem", { name: "Add or edit conditional breakpoint" }).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.points.length),
    1,
    "Opening the breakpoint menu must not toggle the breakpoint",
  );
  // Monaco suppresses mouse-up for the first 100 ms after opening a menu.
  await page.waitForTimeout(150);
  await page.getByRole("menuitem", { name: "Add or edit conditional breakpoint" }).click();
  await workspace
    .getByRole("textbox", { name: "Breakpoint condition", exact: true })
    .fill("value === 42");
  await workspace.evaluate((element) =>
    element.openFile(element.files.find((file) => file.group === "Extensions")),
  );
  assert.equal(await workspace.locator(".condition-source").textContent(), "sample/main.ts:2");
  await workspace.getByRole("button", { name: "Save", exact: true }).click();
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.workspaceDebug.points[0].breakpoint.condition ===
      "value === 42",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.points[0].file),
    "script/sample/main.ts",
    "Source navigation during condition editing must preserve its original breakpoint",
  );
  await workspace.getByRole("button", { name: "main.ts Loaded", exact: true }).click();
  await workspace
    .getByRole("combobox", { name: "Source representation" })
    .selectOption("javascript");
  assert.doesNotMatch(
    await workspace.evaluate((element) => element.model.getValue()),
    /value: number/u,
  );
  await page.evaluate(() =>
    globalThis.__workspaceFixture.workspaceDebug.pause({
      artifact: "sample:1:exact",
      exception: {
        name: "Error",
        message: "Paused exception details",
        frames: [],
        unavailable: false,
      },
      causes: [],
      truncated: false,
    }),
  );
  await workspace
    .getByRole("button", { name: "Continue (F8)", exact: true })
    .waitFor({ state: "visible" });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerPaused,
  );
  await workspace.locator(".debug-execution-line").waitFor();
  await workspace
    .locator(".developer-exception")
    .getByText("Error: Paused exception details", { exact: true })
    .waitFor();
  await workspace.evaluate(async (element) => {
    const original = element.selected;
    await element.openFile({ ...original, artifact: "retired-artifact", state: "Retired" });
  });
  assert.equal(
    await workspace.locator(".debug-execution-line").count(),
    0,
    "A different revision must not show this worker's execution line",
  );
  await workspace.getByRole("button", { name: "main.js Loaded", exact: true }).click();
  await workspace.locator(".debug-execution-line").waitFor();
  await workspace.getByRole("textbox", { name: "Immediate expression" }).fill("value");
  await workspace
    .getByRole("textbox", { name: "Immediate expression" })
    .dispatchEvent("keydown", { key: "Enter", isComposing: true });
  assert.equal(
    await workspace.evaluate((element) => element.debuggerConsole.length),
    0,
    "Finishing IME composition must not execute the console command",
  );
  await workspace.getByRole("button", { name: "Run", exact: true }).click();
  await workspace.locator(".command > pre").filter({ hasText: "42" }).waitFor();
  await checkDebuggerInspection(page, workspace, root);
  await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.extraTargets = [{ ...debug.info, session: "other-worker", label: "Other worker" }];
    debug.holdEvaluation = true;
  });
  await workspace
    .getByRole("combobox", { name: "Debugger worker" })
    .getByRole("option", { name: /Other worker/u })
    .waitFor({ state: "attached" });
  await workspace.getByRole("textbox", { name: "Immediate expression" }).fill("value");
  await workspace.getByRole("button", { name: "Run", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.finishEvaluation);
  assert.equal(
    await workspace.getByRole("button", { name: "Run", exact: true }).isDisabled(),
    true,
  );
  await workspace.getByRole("textbox", { name: "Immediate expression" }).fill("next draft");
  await workspace.getByRole("combobox", { name: "Debugger worker" }).selectOption("other-worker");
  await workspace.evaluate((element) => element.refresh());
  await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.holdEvaluation = false;
    debug.finishEvaluation();
  });
  await workspace.locator(".command").last().getByText("42", { exact: true }).waitFor();
  assert.equal(
    await workspace.getByRole("textbox", { name: "Immediate expression" }).inputValue(),
    "next draft",
    "A delayed submission must preserve newer console text",
  );
  await workspace
    .getByRole("combobox", { name: "Debugger worker" })
    .selectOption("workspace-worker");
  await workspace.locator(".debug-execution-line").waitFor();
  await workspace.getByRole("textbox", { name: "Immediate expression" }).press("F8");
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerPaused,
  );
  await workspace
    .getByRole("textbox", { name: "Immediate expression" })
    .fill("console.log('fixture output'); 3 + 4");
  await workspace.getByRole("button", { name: "Run", exact: true }).click();
  await workspace.locator(".command > pre").filter({ hasText: "7" }).waitFor();
  await workspace
    .locator(".command > pre")
    .filter({ hasText: "<img src=x onerror=alert(1)>" })
    .waitFor();
  assert.equal(await workspace.locator(".console img").count(), 0);
  for (const [key, kind] of [
    ["F10", "over"],
    ["F11", "into"],
    ["Shift+F11", "out"],
    ["F8", "continue"],
    ["F5", "continue"],
    ["Control+\\", "continue"],
  ]) {
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
    await page.waitForFunction(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
          .debuggerPaused,
    );
    const count = await page.evaluate(
      () => globalThis.__workspaceFixture.workspaceDebug.actions.length,
    );
    await workspace.evaluate((element) => element.editor.focus());
    await page.keyboard.press(key);
    await page.waitForFunction(
      ({ count, kind }) =>
        globalThis.__workspaceFixture.workspaceDebug.actions
          .slice(count)
          .some((action) => action.kind === "resume" && action.action === kind),
      { count, kind },
    );
  }
  await workspace.getByRole("combobox", { name: "Pause on exceptions" }).selectOption("all");
  await page.waitForFunction(() => globalThis.__workspaceFixture.workspaceDebug.policy === "all");
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerPaused,
  );
  await workspace.getByRole("button", { name: "Continue (F8)", exact: true }).focus();
  await workspace.getByRole("tooltip").filter({ hasText: "Ctrl+" }).waitFor();
  assert.equal(await workspace.locator('debug-icon[name="continue"] svg').count(), 1);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check", `debugger-workspace-${width}.png`),
    });
    const bounds = await workspace.boundingBox();
    assert.ok(bounds && bounds.width > 0 && bounds.height > 150);
    const run = await workspace.getByRole("button", { name: "Run", exact: true }).boundingBox();
    assert.ok(
      run && run.y + run.height <= height,
      `Console input must remain reachable at ${width}×${height}`,
    );
    await workspace.getByRole("button", { name: "Dock", exact: true }).click();
    await workspace.getByRole("button", { name: "Dock to right", exact: true }).click();
    const rightRun = await workspace
      .getByRole("button", { name: "Run", exact: true })
      .boundingBox();
    assert.ok(
      rightRun && rightRun.y + rightRun.height <= height,
      `Console must remain reachable in a ${width}×${height} right dock`,
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check", `debugger-workspace-right-${width}.png`),
    });
    await workspace.getByRole("button", { name: "Dock", exact: true }).click();
    await workspace.getByRole("button", { name: "Dock to bottom", exact: true }).click();
  }
  await workspace.getByRole("textbox", { name: "Immediate expression" }).press("Shift+F5");
  await page.waitForFunction(() => globalThis.__workspaceFixture.workspaceDebug.closed);
  await workspace.getByRole("button", { name: "Restart", exact: true }).click();
  await page.waitForFunction(() => !globalThis.__workspaceFixture.workspaceDebug.closed);
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-workspace-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.waitForTimeout(350);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-workspace-dark.png"),
  });
  await page.emulateMedia({ colorScheme: "light" });
  await workspace.getByRole("button", { name: "Close debugger and continue execution" }).click();
  await workspace.waitFor({ state: "detached" });
}
