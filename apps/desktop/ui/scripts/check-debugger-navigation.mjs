import assert from "node:assert/strict";

export async function checkDebuggerNavigation(page, workspace) {
  const editor = async () => workspace.evaluate((element) => element.editor.focus());
  const source = () => workspace.evaluate((element) => element.debuggerSourceLabel);
  const view = async (name) => {
    await workspace.getByRole("button", { name: "View", exact: true }).click();
    await workspace.getByRole("button", { name, exact: true }).click();
  };
  const before = await page.evaluate(
    () => globalThis.__workspaceFixture.workspaceDebug.actions.length,
  );
  await editor();
  await page.keyboard.press("Control+p");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).fill("src/view");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Enter");
  assert.equal(await source(), "example/src/view.ts");
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
  assert.equal(await workspace.locator(".source[data-selected]").textContent(), "view.ts Unloaded");
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
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.actions.length),
    before,
    "Reading unloaded sources must not activate or control a worker",
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
  await view("Sources");
  await sources.waitFor();
  await editor();
  await page.keyboard.press("Escape");
  await workspace.locator(".console").waitFor({ state: "hidden" });
  await page.keyboard.press("Escape");
  await workspace.getByRole("textbox", { name: "Immediate expression" }).waitFor();
  await workspace.getByRole("button", { name: "Hide console (Escape)", exact: true }).click();
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
  await workspace.getByRole("button", { name: "main.ts Loaded", exact: true }).click();
  await workspace.locator(".source-pane").waitFor();
  await view("Inspector");
  await workspace.locator(".variables").waitFor();
  await workspace.getByRole("button", { name: "Close inspector", exact: true }).click();
  await workspace.locator(".source-pane").waitFor();
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
