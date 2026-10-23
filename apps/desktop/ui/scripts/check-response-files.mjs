import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkResponseFiles(page, root) {
  await page.addInitScript(() => sessionStorage.setItem("startup-update-tested", "true"));
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  await page.evaluate(async () => {
    const s = document.querySelector("app-shell");
    await s.activateView("automation");
    await s.automation.beginScratchAutoResponse();
  });
  const editor = page.locator("automation-workspace .auto-response-editor");
  const body = editor.locator('textarea[name="body"]'),
    save = editor.getByRole("button", { name: "Save rule", exact: true });
  await editor.getByLabel("Rule name", { exact: true }).fill("External CSS iteration");
  await editor
    .getByLabel("Matching address", { exact: true })
    .fill("https://example.test/style.css");
  await body.fill("unsaved inline body");
  const file = "C:/fixtures/a long folder name/site theme & state <inert>/editable style.css";
  await page.evaluate((file) => {
    const s = globalThis.__workspaceFixture;
    s.bodyFile = file;
    s.bodyFileText = "h1 {color:red}";
    s.bodyFileBytes = 32;
  }, file);
  await editor.getByRole("button", { name: "Serve response body from file…", exact: true }).click();
  const mode = editor.getByLabel("File use", { exact: true });
  assert.equal(await mode.inputValue(), "copy");
  await mode.selectOption("linked");
  await editor.getByText(/F5 keeps the open preview/).waitFor();
  assert.equal(await editor.locator("script,img").count(), 0, "File path became active markup");
  // A metadata refresh across the old cutoff must retain the explicit link choice.
  await page.evaluate(() => (globalThis.__workspaceFixture.bodyFileBytes = 26000000));
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.fileChoiceBusy);
  assert.equal(await mode.inputValue(), "linked");
  await page.evaluate(() => (globalThis.__workspaceFixture.bodyFileBytes = 32));
  await save.click();
  await page.waitForFunction(() => {
    const a = document.querySelector("app-shell").automation;
    return (
      !a.savingAutoResponse && !a.loadingSavedResponse && !a.fileChoiceBusy && a.existingResponse
    );
  });
  const rule = await page.evaluate(
    () => document.querySelector("app-shell").automation.editingAutoResponseId,
  );
  assert.equal(await mode.inputValue(), "linked");
  const saved = await page.evaluate(() => globalThis.__workspaceFixture.lastAsset);
  assert.equal(saved.bodyFileMode, "linked");
  assert.equal(saved.bodyPath, file);
  // Error checks preserve the saved path and selection; retry does not dirty the draft.
  await page.evaluate(() => (globalThis.__workspaceFixture.bodyFileError = true));
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await editor.getByText(/Response file is missing or unreadable/).waitFor();
  assert.equal(await mode.inputValue(), "linked");
  assert.equal(await editor.getByText(file, { exact: true }).textContent(), file);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.draftDirty),
    false,
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await editor.getByRole("button", { name: "Check file", exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/response-file-repair-${width}.png`),
    });
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  const check = editor.getByRole("button", { name: "Check file", exact: true });
  await check.evaluate((button) => button.scrollIntoView({ block: "start" }));
  await check.click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.fileChoiceBusy);
  const checkBox = await check.boundingBox(),
    footerBox = await editor.locator(".auto-response-editor-actions").boundingBox();
  assert.ok(
    checkBox.y + checkBox.height <= footerBox.y,
    "Sticky save actions cover file repair at zoom",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/response-file-repair-zoom-contrast.png"),
  });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.emulateMedia({ forcedColors: "none" });
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.bodyFileError = false;
    s.lastFileFailure = "Last request failed while reading the linked file.";
  });
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await editor
    .getByText("Last request failed while reading the linked file.", { exact: true })
    .waitFor();
  await page.evaluate(() => (globalThis.__workspaceFixture.lastFileFailure = null));
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.fileChoiceBusy);
  // Explicit copy also stays copy when the source grows; no silent link promotion.
  await mode.selectOption("copy");
  await page.evaluate(() => (globalThis.__workspaceFixture.bodyFileBytes = 26000000));
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.fileChoiceBusy);
  assert.equal(await mode.inputValue(), "copy");
  await page.evaluate(() => (globalThis.__workspaceFixture.assetError = true));
  await save.click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.savingAutoResponse,
  );
  assert.equal(await mode.inputValue(), "copy");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.draftDirty),
    true,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.assetError = false;
    globalThis.__workspaceFixture.bodyFileBytes = 32;
  });
  await save.click();
  await page.waitForFunction(() => {
    const a = document.querySelector("app-shell").automation;
    return !a.savingAutoResponse && !a.loadingSavedResponse && !a.draftDirty;
  });
  assert.equal(await mode.count(), 0);
  await editor.getByText(/Saved copy from/).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastAssetEdit.bodyFileMode),
    "copy",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.editingAutoResponseId),
    rule,
  );
  // Clearing a pending choice restores the inline draft. A stale check cannot restore it.
  await body.fill("preserved inline body");
  await editor.getByRole("button", { name: "Serve response body from file…", exact: true }).click();
  await page.evaluate(() => (globalThis.__workspaceFixture.deferFileInfo = true));
  await editor.getByRole("button", { name: "Check file", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishFileInfo);
  await editor.getByRole("button", { name: "Clear file choice", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.finishFileInfo());
  assert.equal(await body.inputValue(), "preserved inline body");
  assert.equal(await mode.count(), 0);
  await save.click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.draftDirty);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseCopiedFrom),
    "",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
}
