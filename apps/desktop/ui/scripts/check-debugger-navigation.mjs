import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerNavigation(page, workspace, root) {
  const editor = async () => workspace.evaluate((element) => element.editor.focus());
  const source = () => workspace.evaluate((element) => element.debuggerSourceLabel);
  const view = async (name, keyboard = false) => {
    await workspace.getByRole("button", { name: "View", exact: true }).click();
    const item = workspace.getByRole("button", { name, exact: true });
    if (keyboard) await item.press("Enter");
    else await item.click();
    assert.equal(await workspace.locator("#debugger-view").isVisible(), false);
  };
  const checkView = async (expected, screenshot) => {
    const trigger = workspace.getByRole("button", { name: "View", exact: true });
    await trigger.press("Enter");
    const menu = workspace.locator("#debugger-view");
    for (const [name, shown] of Object.entries(expected)) {
      const item = menu.getByRole("button", { name, exact: true });
      assert.equal(await item.getAttribute("aria-pressed"), String(shown), name);
      assert.equal(await item.locator(".view-check").isVisible(), shown, name);
    }
    if (screenshot) {
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/debugger-view-${screenshot}.png`),
      });
    }
    await menu.getByRole("button").first().press("Escape");
    assert.equal(await menu.isVisible(), false);
    assert.equal(
      await trigger.evaluate((element) => element.getRootNode().activeElement === element),
      true,
    );
  };
  await checkView({ Sources: true, Inspector: true, "Console Esc": true }, "wide");
  const before = await page.evaluate(
    () => globalThis.__workspaceFixture.workspaceDebug.actions.length,
  );
  await editor();
  await page.keyboard.press("Control+p");
  const query = workspace.getByRole("textbox", { name: "Source query", exact: true });
  const beforeFilter = await query.boundingBox();
  await query.fill("no-matching-file");
  await workspace
    .locator(".source-dialog")
    .getByText("No matching sources.", { exact: true })
    .waitFor();
  const afterFilter = await query.boundingBox();
  assert.equal(afterFilter.y, beforeFilter.y, "Changing results must not move the input");
  assert.equal(afterFilter.x, beforeFilter.x);
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill("src/view:3:5");
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/debugger-source-location-wide.png"),
  });
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Enter");
  assert.equal(await source(), "example/src/view.ts");
  assert.deepEqual(
    await workspace.evaluate((element) => {
      const position = element.editor.getPosition();
      return { line: position.lineNumber, column: position.column };
    }),
    { line: 3, column: 5 },
  );
  await editor();
  await page.keyboard.press("Control+p");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill(":2:9");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Enter");
  assert.equal(await workspace.evaluate((element) => element.editor.getPosition().column), 9);
  // A poll can rebuild the tree before the browser delivers its queued toggle event.
  // Preserve both opening and closing through that exact event-order boundary.
  for (let attempt = 0; attempt < 2; attempt++) {
    const toggled = await workspace.evaluate((element) => {
      const summary = [...element.shadowRoot.querySelectorAll(".sources summary")].find(
        (summary) => summary.textContent === "Extensions",
      );
      const details = summary.parentElement;
      const before = details.open;
      summary.click();
      const clicked = details.open;
      element.updateNavigation();
      element.$flushUpdates();
      return { before, clicked, refreshed: details.open };
    });
    assert.notEqual(toggled.clicked, toggled.before, "The native summary click must toggle");
    assert.equal(
      toggled.refreshed,
      toggled.clicked,
      "Refreshing source metadata before the toggle event must preserve the user's choice",
    );
  }
  const extensions = workspace.locator(".sources summary").filter({ hasText: /^Extensions$/u });
  for (const key of ["Space", "Enter"]) {
    const before = await extensions.evaluate((summary) => summary.parentElement.open);
    await extensions.press(key);
    const toggled = await workspace.evaluate((element) => {
      const details = [...element.shadowRoot.querySelectorAll(".sources summary")].find(
        (summary) => summary.textContent === "Extensions",
      ).parentElement;
      const clicked = details.open;
      element.updateNavigation();
      element.$flushUpdates();
      return { clicked, refreshed: details.open };
    });
    assert.notEqual(toggled.clicked, before, `${key} must retain native disclosure activation`);
    assert.equal(toggled.refreshed, toggled.clicked, "Refresh must preserve keyboard disclosure");
  }
  await workspace
    .locator(".sources summary")
    .filter({ hasText: /^Extensions$/u })
    .click();
  await workspace
    .locator(".sources summary")
    .filter({ hasText: /^example$/u })
    .click();
  await workspace.locator(".sources summary").filter({ hasText: /^src$/u }).click();
  await workspace.getByRole("button", { name: "view.ts Unloaded", exact: true }).waitFor();
  assert.equal(
    (await workspace.locator(".source[data-selected]").textContent()).trim(),
    "view.ts Unloaded",
  );
  await workspace.getByRole("tab", { name: "main.ts", exact: true }).click();
  await workspace.getByRole("button", { name: "Back (Alt+Left)", exact: true }).click();
  assert.equal(await source(), "example/src/view.ts");
  await workspace.getByRole("button", { name: "Forward (Alt+Right)", exact: true }).click();
  assert.equal(await source(), "sample/main.ts");
  await workspace.getByRole("button", { name: "Close view.ts", exact: true }).click();
  assert.equal(await workspace.getByRole("tab", { name: "view.ts", exact: true }).count(), 0);
  assert.equal(
    await workspace
      .getByRole("tab", { name: "main.ts", exact: true })
      .evaluate((element) => element === element.getRootNode().activeElement),
    true,
  );
  await editor();
  await page.keyboard.press("Control+Shift+f");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill("const value");
  await workspace.getByRole("button", { name: /sample\/main.ts:2/u }).click();
  assert.equal(await source(), "sample/main.ts");
  assert.equal(await workspace.evaluate((element) => element.editor.getPosition().lineNumber), 2);
  assert.equal(
    await workspace.evaluate((element) =>
      element.model.getValueInRange(element.editor.getSelection()),
    ),
    "const value",
  );
  await checkSourceLoadRaces(page, workspace);
  const actions = await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.actions);
  assert.equal(
    actions.length,
    before,
    `Reading unloaded sources must not activate or control a worker: ${JSON.stringify(actions.slice(before))}`,
  );
  await editor();
  await page.keyboard.press("Control+Shift+f");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdSourceRead = true;
  });
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill("const value");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead,
  );
  await workspace
    .getByRole("textbox", { name: "Source query", exact: true })
    .fill("missing-needle");
  await workspace.locator(".source-dialog").getByText("0 results", { exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead());
  await page.waitForFunction(() => !globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead);
  assert.equal(
    await workspace.locator(".source-result").count(),
    0,
    "Late source reads must not restore stale search results",
  );
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");

  const sources = workspace.locator(".sources");
  const oldWidth = (await sources.boundingBox()).width;
  await workspace
    .getByRole("separator", { name: "Resize sources", exact: true })
    .press("ArrowRight");
  assert.ok(
    (await sources.boundingBox()).width > oldWidth,
    "The sources splitter must resize the visible pane",
  );
  await workspace
    .getByRole("separator", { name: "Resize inspector", exact: true })
    .press("ArrowLeft");
  await page.waitForFunction(() => {
    const layout = globalThis.__workspaceFixture.workspaceDebug.layout;
    return layout.sourcesSplit === 20 && layout.inspectorSplit === 24;
  });
  await workspace.getByRole("button", { name: "Close sources", exact: true }).click();
  await sources.waitFor({ state: "hidden" });
  assert.equal(await workspace.evaluate((element) => element.editor.hasTextFocus()), true);
  await checkView({ Sources: false, Inspector: true, "Console Esc": true }, "sources-hidden");
  await view("Sources", true);
  await sources.waitFor();
  await editor();
  await page.keyboard.press("Escape");
  await workspace.locator(".console").waitFor({ state: "hidden" });
  await page.keyboard.press("Escape");
  await workspace.getByRole("textbox", { name: "Immediate expression" }).waitFor();
  await workspace.getByRole("button", { name: "Hide console (Escape)", exact: true }).click();
  await checkView({ Sources: true, Inspector: true, "Console Esc": false });
  await view("Console Esc");

  await page.setViewportSize({ width: 800, height: 600 });
  await workspace.getByRole("button", { name: "Dock", exact: true }).click();
  await workspace.getByRole("button", { name: "Dock to right", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerCompact,
  );
  await view("Sources");
  await sources.waitFor();
  await checkView({ Code: false, Sources: true, Inspector: false, "Console Esc": true });
  await workspace.getByRole("button", { name: "main.ts Loaded", exact: true }).click();
  await workspace.locator(".source-pane").waitFor();
  await checkView({ Code: true, Sources: false, Inspector: false, "Console Esc": true });
  await view("Inspector");
  await workspace.locator(".variables").waitFor();
  await checkView(
    { Code: false, Sources: false, Inspector: true, "Console Esc": true },
    "compact-800",
  );
  await page.emulateMedia({ forcedColors: "active" });
  await checkView(
    { Code: false, Sources: false, Inspector: true, "Console Esc": true },
    "forced-colors-800",
  );
  await page.emulateMedia({ forcedColors: "none" });
  await workspace.getByRole("button", { name: "View", exact: true }).click();
  await workspace
    .locator("#debugger-view")
    .getByRole("button", { name: /^Open source/u })
    .click();
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
  assert.ok(
    await workspace
      .getByRole("button", { name: "View", exact: true })
      .evaluate((element) => element.getRootNode().activeElement === element),
    "Canceling source navigation restores visible focus when Code is hidden",
  );
  await workspace.getByRole("button", { name: "Close inspector", exact: true }).click();
  await workspace.locator(".source-pane").waitFor();
  await editor();
  await page.keyboard.press("Control+Shift+f");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill("value");
  await workspace.locator(".source-result").first().waitFor();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/debugger-source-search-800.png"),
  });
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
  await page.setViewportSize({ width: 1440, height: 900 });
  await workspace.getByRole("button", { name: "Dock", exact: true }).click();
  await workspace.getByRole("button", { name: "Dock to bottom", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdToggleReply = true;
  });
  await editor();
  await page.keyboard.press("F12");
  await workspace.waitFor({ state: "detached" });
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.finishToggle);
  await page.keyboard.press("F12");
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishToggle());
  await workspace.locator(".monaco-editor").waitFor();
}

async function checkSourceLoadRaces(page, workspace) {
  const result = await workspace.evaluate((element) =>
    element.debuggerNavigationResults.find(
      (result) =>
        result.file.id === element.selected.id &&
        result.file.artifact === element.selected.artifact,
    ),
  );
  assert.ok(result);
  const start = async () => {
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdSourceRead = true;
    });
    await workspace.evaluate((element, result) => {
      globalThis.__pendingSourceNavigation = element.openNavigationResult(result);
    }, result);
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead,
    );
  };
  const finish = async () =>
    page.evaluate(async () => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.holdSourceRead = false;
      debug.releaseSourceRead?.();
      await globalThis.__pendingSourceNavigation;
      delete globalThis.__pendingSourceNavigation;
    });
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  try {
    await start();
    // End the draft with punctuation so the focus probe does not request completions.
    await prompt.fill("Keep this draft;");
    // Exercise completion before releasing the source read, including on fast hosts.
    await workspace.evaluate((element) => element.completePrompt());
    await finish();
    assert.equal(
      await prompt.evaluate((element) => element.getRootNode().activeElement === element),
      true,
      "A delayed source must not steal Console focus",
    );
    assert.equal(await prompt.inputValue(), "Keep this draft;");
    await start();
    await workspace.evaluate(async (element, file) => {
      await element.openFile(file);
      element.editor.setPosition({ lineNumber: 3, column: 4 });
      element.editor.focus();
    }, result.file);
    await finish();
    assert.deepEqual(
      await workspace.evaluate((element) => {
        const position = element.editor.getPosition();
        return { line: position.lineNumber, column: position.column };
      }),
      { line: 3, column: 4 },
      "A newer visit to the same file keeps its position",
    );
    await start();
    await workspace.evaluate(
      (element, key) => element.closeTab(key),
      `${result.file.id}@${result.file.artifact}`,
    );
    await finish();
    assert.equal(
      await workspace.evaluate((element) => element.selected),
      null,
      "Closing the tab cancels its pending source load",
    );
    await workspace.evaluate((element, file) => element.openFile(file), result.file);
    await prompt.fill("");
  } finally {
    await finish();
  }
}
