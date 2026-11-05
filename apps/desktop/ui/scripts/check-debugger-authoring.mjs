import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerAuthoring(page, workspace, root) {
  const watches = workspace.locator(".watches");
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  const edit = watches.getByRole("textbox", { name: "Edit watch expression", exact: true });
  await watches.getByRole("button", { name: "Edit watch value", exact: true }).click();
  await edit.fill("sample");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectWatchSave = true;
  });
  await edit.press("Enter");
  await workspace.getByRole("alert").filter({ hasText: "Watch save failed" }).waitFor();
  assert.equal(await edit.inputValue(), "sample", "Failed saves preserve the draft");
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.watches),
    ["value"],
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectWatchSave = false;
  });
  await edit.press("Escape");
  assert.equal(await edit.count(), 0);
  assert.equal(await prompt.isVisible(), true, "Canceling a watch must not close Console");
  await watches.getByRole("button", { name: "Edit watch value", exact: true }).click();
  await edit.fill("sample");
  await edit.press("Enter");
  await watches.getByRole("button", { name: "Edit watch sample", exact: true }).waitFor();
  assert.equal(
    await workspace.evaluate((element) => element.debuggerError),
    "",
    "A successful retry clears the previous save error",
  );
  await watches.locator("debug-value").getByText("[Object]", { exact: true }).waitFor();
  await watches.getByRole("button", { name: "Remove watch sample", exact: true }).click();

  await watches.locator("summary").click();
  await workspace.getByRole("button", { name: "Close inspector", exact: true }).click();
  const selection = await workspace.evaluate((element) => {
    element.editor.setSelection({
      startLineNumber: 2,
      startColumn: 9,
      endLineNumber: 2,
      endColumn: 14,
    });
    element.editor.focus();
    const p = element.editor.getScrolledVisiblePosition({ lineNumber: 2, column: 11 });
    const b = element.editor.getDomNode().getBoundingClientRect();
    return { x: b.left + p.left, y: b.top + p.top + p.height / 2 };
  });
  assert.equal(
    await workspace.evaluate((element) => element.sourceSelection(1024)),
    "value",
    "The editor selection is available before opening its menu",
  );
  await page.mouse.click(selection.x, selection.y, { button: "right" });
  assert.equal(
    await workspace.evaluate((element) => element.sourceSelection(1024)),
    "value",
    "The context menu preserves selected source",
  );
  // Monaco suppresses activation for the first 100 ms after opening a menu.
  await page.waitForTimeout(150);
  await page.getByRole("menuitem", { name: "Add selection to Watch", exact: false }).click();
  await watches.getByRole("button", { name: "Edit watch value", exact: true }).waitFor();
  await watches.getByText("42", { exact: true }).waitFor();
  await prompt.fill("unsent Console draft");
  await prompt.press("ArrowUp");
  const historyEntry = await prompt.inputValue();
  await workspace.locator(".call-stack button").filter({ hasText: "outer" }).click();
  await workspace.evaluate((element) => {
    element.editor.setSelection({
      startLineNumber: 2,
      startColumn: 9,
      endLineNumber: 2,
      endColumn: 14,
    });
    element.editor.focus();
  });
  await page.keyboard.press("Control+Shift+E");
  await workspace
    .locator(".command")
    .filter({ hasText: "› value" })
    .last()
    .getByText("21", { exact: true })
    .waitFor();
  assert.equal(await prompt.inputValue(), historyEntry);
  await prompt.press("ArrowDown");
  await prompt.press("ArrowDown");
  assert.equal(
    await prompt.inputValue(),
    "unsent Console draft",
    "Source evaluation preserves a draft held behind history navigation",
  );
  await workspace.locator(".call-stack button").filter({ hasText: "run" }).click();

  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.__copiedDebuggerStack = text;
        },
      },
    });
  });
  await workspace.getByRole("button", { name: "Copy stack trace", exact: true }).click();
  assert.match(
    await page.evaluate(() => globalThis.__copiedDebuggerStack),
    /at run \(file:\/\/\/transmog-scripts\/sample\/main.ts:2:3\)\n {4}at outer/u,
  );
  await workspace.locator(".call-stack button").first().press("Shift+F10");
  await page.getByRole("menuitem", { name: "Copy stack trace", exact: true }).click();
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("debugger-workspace")
      .shadowRoot.activeElement?.matches(".call-stack button"),
  );

  await watches.getByRole("button", { name: "Edit watch value", exact: true }).click();
  await edit.fill("(value");
  await edit.press("Shift+Enter");
  await edit.press("End");
  await edit.press("Shift+Enter");
  await edit.type("+ 1)");
  const multiline = await edit.inputValue();
  assert.match(multiline, /\n/u);
  await watches
    .getByRole("textbox", { name: "Watch expression", exact: true })
    .fill("new watch draft");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.recreateSurface = true;
  });
  await workspace.evaluate((element) => element.place("window"));
  await workspace.locator(".monaco-editor").waitFor();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerTransition,
  );
  assert.equal(await edit.inputValue(), multiline, "Moving preserves multiline watch drafts");
  assert.equal(
    await watches.getByRole("textbox", { name: "Watch expression", exact: true }).inputValue(),
    "new watch draft",
  );
  await edit.press("Escape");
  await watches.getByRole("textbox", { name: "Watch expression", exact: true }).fill("");

  // Native memory is committed before the delayed acknowledgment; closing the
  // source surface must not let its eventual reply overwrite the reopened one.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdWatchSave = true;
  });
  await watches.getByRole("textbox", { name: "Watch expression", exact: true }).fill("pending");
  await watches.getByRole("button", { name: "Add watch", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.finishWatchSave);
  await page.evaluate(() => globalThis.__debuggerWorkspaceCommand("toggle_debugger"));
  await workspace.waitFor({ state: "detached" });
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishWatchSave());
  await page.evaluate(() => globalThis.__debuggerWorkspaceCommand("open_debugger"));
  await workspace.locator(".monaco-editor").waitFor();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerTransition,
  );
  await watches.getByRole("button", { name: "Edit watch pending", exact: true }).waitFor();
  await watches.getByRole("button", { name: "Remove watch pending", exact: true }).click();
  await watches.getByText("42", { exact: true }).waitFor();
  await workspace
    .getByRole("combobox", { name: "Source representation" })
    .selectOption("javascript");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.recreateSurface = false;
  });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-authoring-1440.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  if (
    await workspace.evaluate(
      (element) => element.debuggerCompact || !element.debuggerInspectorVisible,
    )
  ) {
    await workspace.getByRole("button", { name: "View", exact: true }).click();
    await workspace.getByRole("button", { name: "Inspector", exact: true }).click();
  }
  await watches.getByRole("button", { name: "Edit watch value", exact: true }).click();
  await edit.fill("value + aLongerExpressionToReview");
  assert.equal(
    await watches.locator(".watch-edit").evaluate((form) => form.scrollWidth <= form.clientWidth),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-watch-edit-800.png"),
  });
  await watches.getByRole("button", { name: "Cancel", exact: true }).press("Enter");
  await page.setViewportSize({ width: 1440, height: 900 });
}
