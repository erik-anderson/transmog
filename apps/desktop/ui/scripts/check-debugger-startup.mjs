import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerStartup(page, workspace, root) {
  const context = workspace.getByRole("combobox", { name: "Console context", exact: true });
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  const output = workspace.locator(".console-output");
  const run = async (expression, expected) => {
    await prompt.fill(expression);
    await prompt.press("Enter");
    await output.locator(".command").last().getByText(expected, { exact: true }).waitFor();
  };
  await workspace.getByRole("region", { name: "Start debugging", exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.canOpenScripts = false;
  });
  await workspace
    .getByRole("button", { name: "Open Scripts", exact: true })
    .waitFor({ state: "detached" });
  await workspace
    .getByText("For live debugging, arm Pause next callback, then use a visualizer.", {
      exact: true,
    })
    .waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.canOpenScripts = true;
  });
  await workspace.getByRole("button", { name: "Open Scripts", exact: true }).waitFor();
  await workspace
    .getByText(
      "For live debugging, arm Pause next callback, then trigger traffic matching an enabled script or use a visualizer. In Scripts, choose Debug → Run sandbox test in debugger to test a draft.",
      { exact: true },
    )
    .waitFor();
  assert.equal(await workspace.getByRole("tab").count(), 0);
  assert.equal(
    await workspace.locator(".monaco-editor").count(),
    0,
    "Scratch startup must not allocate a hidden source editor",
  );
  assert.equal(await context.inputValue(), "scratch");
  assert.equal(
    await workspace.getByRole("combobox", { name: "Debugger worker", exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await workspace
      .getByRole("button", { name: "Pause next callback (F8)", exact: true })
      .isEnabled(),
    true,
  );
  await run("globalThis.scratchValue = 41", "41");
  await run("scratchValue + 1", "42");
  await workspace.getByRole("button", { name: "Clear output", exact: true }).click();
  await run("typeof scratchValue", "number");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectScratchReset = true;
  });
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await workspace
    .locator(".workspace > .error")
    .filter({ hasText: "Scratch reset failed" })
    .waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectScratchReset = false;
  });
  await prompt.fill("typeof scratchValue");
  await prompt.press("Enter");
  await output
    .locator(".command")
    .last()
    .getByText("number", { exact: true })
    .waitFor({ timeout: 3000 });
  const first = await page.evaluate(
    () => globalThis.__workspaceFixture.workspaceDebug.scratch.info.session,
  );
  await prompt.fill("Keep this Scratch draft;");
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await output.getByText("Scratch reset", { exact: true }).waitFor();
  assert.equal(await prompt.inputValue(), "Keep this Scratch draft;");
  await run("typeof scratchValue", "undefined");
  assert.notEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.scratch.info.session),
    first,
  );
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchStart = true;
  });
  await prompt.fill("globalThis.scratchValue = 41");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchStart,
  );
  await context.selectOption("worker");
  await prompt.fill("Keep the newer worker input;");
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishScratchStart());
  await output.locator(".command").last().getByText("41", { exact: true }).waitFor();
  assert.equal(await prompt.inputValue(), "Keep the newer worker input;");
  assert.equal(
    await workspace.getByRole("button", { name: "Reset scratch", exact: true }).count(),
    0,
  );
  await context.selectOption("scratch");
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchStart = true;
  });
  await prompt.fill("globalThis.scratchValue = 41");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchStart,
  );
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishScratchStart());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerSubmitting,
  );
  await run("typeof scratchValue", "undefined");

  await prompt.press("F8");
  await workspace
    .getByText("Waiting for next callback · any script or extension", { exact: true })
    .waitFor();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/debugger-startup-scratch.png"),
  });
  await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.noWorkers = false;
    debug.pause();
  });
  await workspace.getByRole("button", { name: "Continue (F8)", exact: true }).waitFor();
  assert.equal(
    await context.inputValue(),
    "scratch",
    "A live pause must preserve Scratch as the command destination",
  );
  assert.equal(
    await workspace.getByRole("combobox", { name: "Pause target", exact: true }).inputValue(),
    "any",
  );
  await prompt.press("F8");
  await workspace.getByRole("button", { name: "Pause next callback (F8)", exact: true }).waitFor();
  await prompt.press("F8");
  await workspace
    .getByRole("button", { name: "Cancel callback pause (F8)", exact: true })
    .waitFor();
  await prompt.press("F8");
  await workspace.getByRole("button", { name: "Pause next callback (F8)", exact: true }).waitFor();

  await context.selectOption("worker");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdWorkerEvaluation = true;
  });
  await prompt.fill("value");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishWorkerEvaluation,
  );
  await context.selectOption("scratch");
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishWorkerEvaluation());
  await output.locator(".command").getByText("42", { exact: true }).last().waitFor();

  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchEvaluation = true;
  });
  await prompt.fill("globalThis.scratchValue = 41");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchEvaluation,
  );
  await workspace.evaluate((element) => {
    globalThis.__failedResetSubmission = element.evaluationSubmission;
  });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectScratchReset = true;
  });
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await workspace
    .locator(".workspace > .error")
    .filter({ hasText: "Scratch reset failed" })
    .waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.rejectScratchReset = false;
  });
  await run("typeof scratchValue", "number");
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.workspaceDebug.finishScratchEvaluation();
    await globalThis.__failedResetSubmission;
    delete globalThis.__failedResetSubmission;
  });
  await output
    .locator(".command")
    .filter({ hasText: "› globalThis.scratchValue = 41" })
    .last()
    .getByText("Command reply unavailable after reset failed", { exact: true })
    .waitFor({ timeout: 3000 });
  await run("scratchValue + 1", "42");

  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchReset = true;
  });
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchReset,
  );
  const viewButton = workspace.getByRole("button", { name: "View", exact: true });
  await viewButton.focus();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishScratchReset());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerScratchBusy,
  );
  assert.equal(
    await viewButton.evaluate((element) => element.getRootNode().activeElement === element),
    true,
    "A late reset must not take focus from another control",
  );

  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchEvaluation = true;
  });
  await prompt.fill("globalThis.scratchValue = 41");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.rejectScratchEvaluation,
  );
  await workspace.evaluate((element) => {
    globalThis.__lateScratchSubmission = element.evaluationSubmission;
  });
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerScratchBusy,
  );
  await prompt.fill("failNewCommand");
  await prompt.press("Enter");
  await workspace
    .locator(".workspace > .error")
    .filter({ hasText: "New command failed" })
    .waitFor();
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.workspaceDebug.rejectScratchEvaluation();
    await globalThis.__lateScratchSubmission.catch(() => {});
    delete globalThis.__lateScratchSubmission;
  });
  assert.equal(
    await workspace.locator(".error").first().textContent(),
    "New command failed",
    "A retired reply must preserve newer feedback",
  );
  await run("typeof scratchValue", "undefined");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchReset = true;
  });
  await workspace.getByRole("button", { name: "Reset scratch", exact: true }).click();
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchReset,
  );
  await context.selectOption("worker");
  await run("value", "42");
  await prompt.fill("failNewCommand");
  await prompt.press("Enter");
  await workspace
    .locator(".workspace > .error")
    .filter({ hasText: "New command failed" })
    .waitFor();
  await prompt.fill("Keep the worker focus;");
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishScratchReset());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerScratchBusy,
  );
  assert.equal(await prompt.inputValue(), "Keep the worker focus;");
  assert.equal(await workspace.locator(".error").first().textContent(), "New command failed");
  assert.equal(
    await prompt.evaluate((element) => element.getRootNode().activeElement === element),
    true,
  );

  await context.selectOption("scratch");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdScratchEvaluation = true;
  });
  await prompt.fill("globalThis.scratchValue = 41");
  await prompt.press("Enter");
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.finishScratchEvaluation,
  );
  await workspace
    .getByRole("button", { name: "Close debugger and continue execution", exact: true })
    .click();
  await workspace.waitFor({ state: "detached" });
  await page.getByRole("button", { name: "Debugger", exact: true }).click();
  await workspace.getByRole("textbox", { name: "Immediate expression", exact: true }).waitFor();
  await output
    .locator(".command")
    .last()
    .getByText("Command reply unavailable after closing debugger", { exact: true })
    .waitFor({ timeout: 3000 });
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishScratchEvaluation());

  await context.selectOption("worker");
  assert.equal(
    await workspace.getByRole("button", { name: "Reset scratch", exact: true }).count(),
    0,
  );
  await prompt.fill("Keep the worker draft;");
  await context.selectOption("scratch");
  await prompt.fill("Keep the Scratch draft;");
  await context.selectOption("worker");
  assert.equal(await prompt.inputValue(), "Keep the worker draft;");
  await prompt.fill("");
  // A pause reveals the real source. Source browsing itself remains passive.
  await workspace.locator(".monaco-editor").waitFor();
}
