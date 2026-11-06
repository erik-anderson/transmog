import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerControls(page, workspace, root) {
  const delayNextPointEdit = () =>
    page.evaluate(() => {
      const original = window.__TAURI_INTERNALS__.invoke;
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.restorePointInvoke = () => {
        window.__TAURI_INTERNALS__.invoke = original;
      };
      let delayed = false;
      window.__TAURI_INTERNALS__.invoke = (command, args) => {
        if (command === "workspace_breakpoints" && !delayed) {
          delayed = true;
          return new Promise((resolve) => {
            debug.releasePointEdit = () => resolve(original(command, args));
          });
        }
        return original(command, args);
      };
    });
  const menu = async (line, name) => {
    const point = await workspace.evaluate((element, line) => {
      element.editor.revealLineInCenter(line);
      const position = element.editor.getScrolledVisiblePosition({ lineNumber: line, column: 1 });
      const bounds = element.editor.getDomNode().getBoundingClientRect();
      return {
        x: bounds.left + position.left + 15,
        y: bounds.top + position.top + position.height / 2,
      };
    }, line);
    await page.mouse.click(point.x, point.y, { button: "right" });
    await page.getByRole("menuitem", { name, exact: true }).waitFor();
    await page.waitForTimeout(150);
    await page.getByRole("menuitem", { name, exact: true }).click();
  };
  await menu(3, "Add or edit logpoint");
  await workspace
    .getByRole("textbox", { name: "Log expression", exact: true })
    .fill('"value: " + value');
  await delayNextPointEdit();
  await workspace.getByRole("button", { name: "Save", exact: true }).dblclick();
  assert.equal(
    await workspace.evaluate((element) => element.pointEditsPending),
    1,
    "Double submission must create one point",
  );
  await workspace.getByRole("button", { name: "Saving…", exact: true }).waitFor();
  assert.equal(
    await workspace.getByRole("button", { name: "Saving…", exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await workspace.getByRole("textbox", { name: "Log expression", exact: true }).isDisabled(),
    true,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releasePointEdit());

  await page.waitForFunction(
    () => globalThis.__workspaceFixture.workspaceDebug.points.length === 2,
  );
  await workspace.locator(".debug-logpoint").waitFor();
  await workspace
    .getByRole("dialog", { name: "Logpoint", exact: true })
    .waitFor({ state: "hidden" });
  assert.equal(await workspace.evaluate((element) => element.editor.hasTextFocus()), true);
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.restorePointInvoke());
  await page.waitForFunction(() => {
    const groups = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("debugger-workspace").debuggerPointGroups;
    const points = groups.flatMap((group) => group.points);
    return points.length === 2 && points.every((point) => point.status === "Bound");
  });
  const points = workspace.locator(".breakpoints");
  assert.equal(await points.getByRole("checkbox").count(), 2);
  await delayNextPointEdit();
  await points.getByRole("checkbox").first().uncheck();
  await points.getByRole("checkbox").first().check();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releasePointEdit());
  await page.waitForFunction(() => {
    const workspace = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("debugger-workspace");
    return (
      !workspace.pointEditsPending &&
      globalThis.__workspaceFixture.workspaceDebug.points.every((point) => point.enabled)
    );
  });
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.restorePointInvoke());
  await points.getByRole("checkbox").first().uncheck();
  await page.waitForFunction(() =>
    globalThis.__workspaceFixture.workspaceDebug.points.some((point) => !point.enabled),
  );
  await points.getByRole("button", { name: "Disable all", exact: true }).click();
  await page.waitForFunction(() =>
    globalThis.__workspaceFixture.workspaceDebug.points.every((point) => !point.enabled),
  );
  await points.getByRole("button", { name: "Enable all", exact: true }).click();
  await page.waitForFunction(() =>
    globalThis.__workspaceFixture.workspaceDebug.points.every((point) => point.enabled),
  );
  await points
    .getByRole("button", { name: "Edit breakpoint sample/main.js:3", exact: true })
    .click();
  assert.equal(
    await workspace.getByRole("textbox", { name: "Log expression", exact: true }).inputValue(),
    '"value: " + value',
  );
  await workspace
    .getByRole("textbox", { name: "Breakpoint condition", exact: true })
    .fill("value > 0");
  await workspace.getByRole("button", { name: "Save", exact: true }).click();
  await page.waitForFunction(() =>
    globalThis.__workspaceFixture.workspaceDebug.points.some(
      (point) => point.breakpoint.logExpression && point.breakpoint.condition === "value > 0",
    ),
  );
  const skip = workspace.getByRole("button", { name: "Skip all pauses", exact: true });
  await skip.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.workspaceDebug.skipPauses);
  assert.equal(await skip.getAttribute("aria-pressed"), "true");
  assert.equal(await workspace.evaluate((element) => element.debuggerPaused), true);
  assert.equal(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.workspaceDebug.points.every((point) => point.enabled),
    ),
    true,
    "Skipping pauses must preserve saved breakpoint choices",
  );
  await skip.click();
  await page.waitForFunction(() => !globalThis.__workspaceFixture.workspaceDebug.skipPauses);
  await menu(3, "Continue to here");
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.workspaceDebug.paused.event.frames[0].location.line === 3,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.points.length),
    2,
  );
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  await prompt.press("F8");
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerPaused,
  );
  await prompt.press("F8");
  await workspace
    .getByRole("button", { name: "Cancel callback pause (F8)", exact: true })
    .waitFor();
  await prompt.press("F8");
  await page.waitForFunction(() => !globalThis.__workspaceFixture.workspaceDebug.pauseNext);
  await skip.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.workspaceDebug.skipPauses);
  await prompt.press("F8");
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.workspaceDebug.pauseNext &&
      !globalThis.__workspaceFixture.workspaceDebug.skipPauses,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.logpoint("value: 42"));
  await workspace.locator(".command").getByText("value: 42", { exact: true }).waitFor();
  await workspace.evaluate((element) => element.refresh());
  assert.equal(
    await workspace.locator(".command").getByText("value: 42", { exact: true }).count(),
    1,
    "Polling must not duplicate logpoint output",
  );
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
  await workspace.locator(".status").getByText("next callback", { exact: true }).waitFor();
  await checkPauseRearming(page, workspace);
  await workspace
    .locator(".status")
    .getByText("Paused in onRequestHead", { exact: true })
    .waitFor();
  await workspace.locator(".status").getByText("GET example.test/first", { exact: true }).waitFor();
  await workspace.getByRole("button", { name: "Show request", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("traffic-workspace")
        .selectedSessionId === "first",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.requestUrl =
      "http://example.test/" + "long-path/".repeat(30) + "?query=long";
  });
  await workspace.evaluate((element) => element.refresh());
  for (const width of [800, 960, 1440]) {
    await page.setViewportSize({ width, height: width === 1440 ? 900 : 720 });
    await workspace.getByRole("button", { name: "Show request", exact: true }).waitFor();
    assert.equal(
      await workspace.evaluate(
        (element) =>
          element.shadowRoot.querySelector(".status").scrollWidth <=
          element.shadowRoot.querySelector(".status").clientWidth + 1,
      ),
      true,
    );
  }
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-context-1440.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-context-800.png"),
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.workspaceDebug.requestUrl;
    globalThis.__workspaceFixture.workspaceDebug.requestContext = false;
  });
  await workspace.evaluate((element) => element.refresh());
  assert.equal(
    await workspace.getByRole("button", { name: "Show request", exact: true }).count(),
    0,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.requestContext = true;
  });
  await workspace.evaluate((element) => element.refresh());
  assert.match(
    await workspace
      .getByRole("combobox", { name: "Debugger worker" })
      .locator("option:checked")
      .textContent(),
    /Paused\s*$/u,
  );
  await workspace.locator(".watch").getByText("42", { exact: true }).waitFor();
  await page.mouse.move(5, 5);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-controls-1440.png"),
  });
  await delayNextPointEdit();
  await points
    .getByRole("button", { name: "Remove breakpoint sample/main.js:3", exact: true })
    .click();
  assert.equal(
    await workspace.evaluate((element) => {
      element.$flushUpdates();
      return element.debuggerPoints.length;
    }),
    1,
  );
  assert.equal(
    await points.getByRole("button", { name: "Enable all", exact: true }).isDisabled(),
    true,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releasePointEdit());
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.workspaceDebug.points.length === 1,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.restorePointInvoke());
}

async function checkPauseRearming(page, workspace) {
  for (const omitReply of [false, true]) {
    await workspace.getByRole("button", { name: "Continue (F8)", exact: true }).click();
    await workspace
      .getByRole("button", { name: "Pause next callback (F8)", exact: true })
      .waitFor();
    await page.evaluate((omit) => {
      globalThis.__workspaceFixture.workspaceDebug.omitPauseReply = omit;
    }, omitReply);
    await workspace.getByRole("button", { name: "Pause next callback (F8)", exact: true }).click();
    await page.waitForFunction(() => {
      const element = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("debugger-workspace");
      return (
        !element.busy &&
        !element.debuggerControlSubmitting &&
        element.views.get(element.debuggerTarget)?.stream.pauseNext
      );
    });
    await workspace
      .getByRole("button", { name: "Cancel callback pause (F8)", exact: true })
      .waitFor();
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
    await workspace.getByRole("button", { name: "Continue (F8)", exact: true }).click();
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
          .debuggerPaused,
    );
    await workspace
      .getByRole("button", { name: "Pause next callback (F8)", exact: true })
      .waitFor();
    await workspace.getByRole("button", { name: "Pause next callback (F8)", exact: true }).click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.workspaceDebug.pauseNext);
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
    await workspace.getByRole("button", { name: "Continue (F8)", exact: true }).waitFor();
  }
}
