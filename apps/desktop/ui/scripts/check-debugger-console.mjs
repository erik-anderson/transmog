import assert from "node:assert/strict";

export async function checkDebuggerConsole(page, workspace) {
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  await prompt.fill("draft during handoff");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.holdMove = true;
  });
  await workspace.evaluate((element) => {
    globalThis.__pendingDebuggerMove = element.place("window");
  });
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.failMove);
  assert.equal(await workspace.locator(".workspace").evaluate((element) => element.inert), true);
  await page.keyboard.type("must not replace the draft");
  assert.equal(
    await workspace.evaluate((element) => element.debuggerInput),
    "draft during handoff",
  );
  await page.evaluate(async () => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.failMove();
    await globalThis.__pendingDebuggerMove;
    delete debug.failMove;
    delete globalThis.__pendingDebuggerMove;
  });
  await workspace.getByRole("alert").filter({ hasText: "Debugger move failed" }).first().waitFor();
  assert.equal(await workspace.locator(".workspace").evaluate((element) => element.inert), false);
  assert.equal(await prompt.inputValue(), "draft during handoff");
  await workspace.evaluate((element) => {
    element.debuggerError = "";
  });
  const logs = await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    const previous = structuredClone(debug.logs);
    debug.logs = [
      {
        context: {
          worker: "console-worker",
          revision: "one",
          callback: "onRequestHead",
          invocation: 2,
          exchangeId: null,
        },
        records: ["First synchronous log", "Second synchronous log"].map((text, index) => ({
          sequence: index + 1,
          timeUnixMs: 1234,
          level: "info",
          text,
          frames: [],
          truncated: false,
        })),
        dropped: 0,
        incomplete: false,
      },
    ];
    return previous;
  });
  await workspace.evaluate((element) => element.refresh());
  await workspace
    .locator(".command > pre")
    .getByText("Second synchronous log", { exact: true })
    .waitFor();
  assert.deepEqual(
    (await workspace.locator(".command > pre").allTextContents()).filter((text) =>
      text.endsWith("synchronous log"),
    ),
    ["First synchronous log", "Second synchronous log"],
  );
  await workspace.getByRole("button", { name: "Clear output", exact: true }).click();
  await page.evaluate(() =>
    globalThis.__workspaceFixture.workspaceDebug.logs[0].records.push({
      sequence: 3,
      timeUnixMs: 1235,
      level: "info",
      text: "New after clear",
      frames: [],
      truncated: false,
    }),
  );
  await workspace.evaluate((element) => element.refresh());
  await workspace.getByText("New after clear", { exact: true }).waitFor();
  await prompt.fill("value");
  await workspace.getByRole("button", { name: "Run", exact: true }).click();
  await workspace.locator(".command > pre").filter({ hasText: "42" }).waitFor();
  await workspace
    .getByRole("combobox", { name: "Console output", exact: true })
    .selectOption("selected");
  await workspace
    .getByRole("combobox", { name: "Console level", exact: true })
    .selectOption("warn");
  await prompt.fill("history draft");
  await prompt.press("ArrowUp");
  assert.equal(await prompt.inputValue(), "value");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.recreateSurface = true;
  });
  await workspace.evaluate((element) => {
    globalThis.__previousDebuggerWidget = element;
  });
  await workspace.evaluate((element) => element.place("window"));
  await page.waitForFunction(() => {
    const current = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("debugger-workspace");
    return (
      current !== globalThis.__previousDebuggerWidget &&
      current?.debuggerPaused &&
      !current.debuggerTransition &&
      !current.debuggerClosed
    );
  });
  await page.evaluate(() => {
    delete globalThis.__previousDebuggerWidget;
    globalThis.__workspaceFixture.workspaceDebug.recreateSurface = false;
  });
  assert.equal(await prompt.inputValue(), "value");
  await prompt.press("ArrowDown");
  assert.equal(await prompt.inputValue(), "history draft");
  assert.equal(
    await workspace.getByRole("combobox", { name: "Console output", exact: true }).inputValue(),
    "selected",
  );
  assert.equal(
    await workspace.getByRole("combobox", { name: "Console level", exact: true }).inputValue(),
    "warn",
  );
  await workspace.getByRole("combobox", { name: "Console level", exact: true }).selectOption("all");
  assert.equal(await workspace.getByText("First synchronous log", { exact: true }).count(), 0);
  assert.equal(await workspace.getByText("Second synchronous log", { exact: true }).count(), 0);
  await workspace.getByText("New after clear", { exact: true }).waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.workspaceDebug.logs[0].dropped = 4;
  });
  await workspace.getByText("4 console records omitted", { exact: true }).waitFor();
  await workspace.getByRole("button", { name: "Clear output", exact: true }).click();
  await page.evaluate(() => {
    const run = globalThis.__workspaceFixture.workspaceDebug.logs[0];
    run.dropped = 7;
    for (let sequence = 4; sequence <= 40; sequence++)
      run.records.push({
        sequence,
        timeUnixMs: 1235 + sequence,
        level: "info",
        text: "New record " + sequence,
        frames: [],
        truncated: false,
      });
    globalThis.__workspaceFixture.workspaceDebug.recreateSurface = true;
  });
  await workspace.getByText("3 console records omitted", { exact: true }).waitFor();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace").busy,
  );
  await workspace.evaluate((element) => {
    element.consoleOutput.scrollTop = element.consoleOutput.scrollHeight;
  });
  await page.evaluate(() => {
    const original = window.__TAURI_INTERNALS__.invoke;
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.restoreConsoleRead = () => {
      window.__TAURI_INTERNALS__.invoke = original;
    };
    let delayed = false;
    window.__TAURI_INTERNALS__.invoke = (command, args) => {
      if (command === "workspace_debug_session" && !delayed) {
        delayed = true;
        return new Promise((resolve) => {
          debug.releaseConsoleRead = () => resolve(original(command, args));
        });
      }
      return original(command, args);
    };
  });
  await workspace.evaluate((element) => {
    globalThis.__consoleRead = element.refresh();
  });
  await page.waitForFunction(
    () => !!globalThis.__workspaceFixture.workspaceDebug.releaseConsoleRead,
  );
  await workspace.evaluate((element) => {
    element.consoleOutput.scrollTop = 100;
  });
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.releaseConsoleRead());
  await workspace.evaluate(async () => {
    await globalThis.__consoleRead;
    delete globalThis.__consoleRead;
  });
  await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.restoreConsoleRead());
  const scroll = await workspace.evaluate((element) => element.consoleOutput.scrollTop);
  assert.equal(scroll, 100, "A delayed poll must preserve a newer reading position");
  await workspace.evaluate((element) => {
    globalThis.__previousDebuggerWidget = element;
  });
  await workspace.evaluate((element) => element.place("window"));
  await page.waitForFunction(() => {
    const current = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("debugger-workspace");
    return (
      current !== globalThis.__previousDebuggerWidget &&
      current?.debuggerPaused &&
      !current.debuggerTransition &&
      !current.debuggerClosed
    );
  });
  assert.equal(await workspace.evaluate((element) => element.consoleOutput.scrollTop), scroll);
  await page.evaluate((logs) => {
    delete globalThis.__previousDebuggerWidget;
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    debug.recreateSurface = false;
    debug.logs = logs;
  }, logs);
  await workspace
    .getByRole("combobox", { name: "Console output", exact: true })
    .selectOption("all");
}
