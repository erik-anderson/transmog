import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerInspection(page, workspace, root) {
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  const inspector = workspace.getByRole("complementary", { name: "Call stack and variables" });
  await inspector.locator(".call-stack button").filter({ hasText: "run" }).waitFor();
  await inspector
    .locator("debug-value")
    .filter({ hasText: "local" })
    .getByText("42", { exact: true })
    .waitFor();
  await workspace.locator(".debug-inline-value").waitFor();
  const closure = inspector.locator("debug-value").filter({ hasText: "closure" });
  await closure.getByRole("button", { name: /closure/u }).click();
  await closure.getByText("42", { exact: true }).waitFor();
  const local = inspector.locator("debug-value").filter({ hasText: "local" });
  await local.getByRole("button", { name: /local/u }).click();

  assert.equal(
    await closure.getByRole("button", { name: /closure/u }).getAttribute("aria-expanded"),
    "true",
  );
  await local.getByRole("button", { name: /local/u }).click();

  const beforeSelection = await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.holdSourceRead = true;
    return debug.actions.length;
  });
  await workspace.evaluate((element) => {
    element.pendingFrameSelection = element.selectFrame(1);
  });
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead,
  );
  await workspace.evaluate((element) => element.selectFrame(0));
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releaseSourceRead());
  await workspace.evaluate(async (element) => {
    await element.pendingFrameSelection;
    delete element.pendingFrameSelection;
  });
  assert.deepEqual(
    await page.evaluate(
      (before) =>
        globalThis.__workspaceFixture.workspaceDebug.actions
          .slice(before)
          .filter((action) => action.kind === "scopes")
          .map((action) => action.frame),
      beforeSelection,
    ),
    [0],
    "A delayed frame selection must not request scopes after a newer selection",
  );

  await inspector.getByRole("textbox", { name: "Watch expression" }).fill("value");
  await inspector.getByRole("button", { name: "Add watch", exact: true }).click();
  await inspector.locator(".watch").getByText("42", { exact: true }).waitFor();
  await inspector.locator(".call-stack button").filter({ hasText: "outer" }).click();
  await inspector.locator(".watch").getByText("21", { exact: true }).waitFor();
  assert.match(await inspector.locator(".call-stack [data-selected]").textContent(), /outer/u);
  await inspector.locator(".call-stack button").filter({ hasText: "run" }).click();
  await inspector.locator(".watch").getByText("42", { exact: true }).waitFor();

  await prompt.fill("sam");
  await prompt.press("Control+Space");
  await workspace.getByRole("option", { name: "sample", exact: true }).waitFor();
  await prompt.press("Tab");
  assert.equal(await prompt.inputValue(), "sample");
  await prompt.press("Enter");
  const result = workspace.locator(".command").filter({ hasText: "› sample" }).last();
  await result.locator("debug-value").getByRole("button", { name: "Result", exact: false }).click();
  await result.getByText("Accessor; not evaluated", { exact: true }).waitFor();
  await result.getByRole("button", { name: "nested", exact: false }).click();
  await result.getByText("answer", { exact: true }).waitFor();
  await prompt.fill("scratch draft");
  await prompt.press("ArrowUp");
  assert.equal(await prompt.inputValue(), "sample");
  await prompt.press("ArrowUp");
  assert.equal(await prompt.inputValue(), "value");
  await prompt.press("ArrowDown");
  await prompt.press("ArrowDown");
  assert.equal(await prompt.inputValue(), "scratch draft");
  await prompt.fill("sam");
  await prompt.press("Control+Space");
  await workspace.getByRole("option", { name: "sample", exact: true }).waitFor();
  await prompt.press("Escape");
  assert.equal(await workspace.getByRole("listbox", { name: "Runtime completions" }).count(), 0);
  assert.equal(
    await prompt.isVisible(),
    true,
    "Escape dismisses suggestions before the Console drawer",
  );
  await prompt.fill("");
  const position = await workspace.evaluate((element) => {
    element.editor.focus();
    const p = element.editor.getScrolledVisiblePosition({ lineNumber: 2, column: 10 });
    const b = element.editor.getDomNode().getBoundingClientRect();
    return { x: b.left + p.left, y: b.top + p.top + p.height / 2 };
  });
  await page.mouse.move(position.x, position.y);
  await page.waitForFunction(() =>
    globalThis.__workspaceFixture.workspaceDebug.actions.some(
      (action) => action.kind === "inspect" && action.expression === "value",
    ),
  );
  await page.mouse.move(5, 5);

  await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.logs = [
      {
        context: {
          worker: "opaque-worker",
          revision: "opaque-revision",
          callback: "onResponse",
          invocation: 1,
          exchangeId: null,
        },
        records: [
          {
            sequence: 1,
            timeUnixMs: Date.now(),
            level: "info",
            text: "Readable callback output",
            frames: [],
            truncated: false,
          },
        ],
        dropped: 2,
        incomplete: true,
      },
    ];
  });
  const log = workspace.locator(".command").filter({ hasText: "Readable callback output" });
  await log.waitFor();
  assert.equal(await log.getByText("opaque-worker", { exact: false }).isVisible(), false);
  await log.getByText("Details", { exact: true }).click();
  await log.getByText("opaque-worker", { exact: false }).waitFor();
  await workspace
    .getByText("2 console records omitted · Console capture incomplete", { exact: true })
    .waitFor();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check", "debugger-inspection-1440.png"),
  });
  await prompt.press("F8");
  await result.getByText("Expired", { exact: true }).waitFor();
  await inspector.locator(".watch").getByText("value: Not paused", { exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.pause());
  await inspector.locator(".watch").getByText("42", { exact: true }).waitFor();
}
