import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkConsolePresentation(page, workspace, root) {
  const previous = await page.evaluate(() => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    const previous = structuredClone(debug.logs);
    debug.logs = [
      {
        context: {
          worker: "mixed-console",
          revision: "one",
          callback: "onRequestHead",
          invocation: 77,
          exchangeId: null,
        },
        records: ["info", "warn", "error", "debug", "trace"].map((level, index) => ({
          sequence: index + 1,
          timeUnixMs: Date.now() + index,
          level,
          text: {
            info: "Request matched",
            warn: "Slow response [Object]",
            error: "Request failed",
            debug: "Request details",
            trace: "Request stack",
          }[level],
          frames: [],
          truncated: false,
          ...(level === "warn"
            ? {
                values: [
                  {
                    name: "",
                    parent: null,
                    kind: "string",
                    display: "Slow response",
                    truncated: false,
                  },
                  { name: "", parent: null, kind: "object", display: "[Object]", truncated: false },
                  {
                    name: "headers",
                    parent: 1,
                    kind: "object",
                    display: "[Object]",
                    truncated: false,
                  },
                  {
                    name: "accept",
                    parent: 2,
                    kind: "string",
                    display: "application/json",
                    truncated: false,
                  },
                  { name: "", parent: 1, kind: "number", display: "42", truncated: false },
                  {
                    name: "lines",
                    parent: 1,
                    kind: "string",
                    display: "first\nsecond",
                    truncated: false,
                  },
                ],
              }
            : {}),
        })),
        dropped: 0,
        incomplete: false,
      },
    ];
    return previous;
  });
  try {
    await workspace.evaluate((element) => element.refresh());
    await workspace.getByText("Request matched", { exact: true }).waitFor();
    assert.equal(await workspace.getByText("Request details", { exact: true }).count(), 0);
    const warning = workspace.locator(".command").filter({ hasText: "Slow response" });
    await warning.locator("console-values .snapshot-toggle").first().click();
    await warning.locator("console-values .snapshot-toggle").filter({ hasText: "headers" }).click();
    const value = warning.getByText("application/json", { exact: true });
    await value.waitFor();
    await value.click({ button: "right" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/console-value-menu.png"),
    });
    await page.getByRole("menuitem", { name: "Copy value", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "application/json");
    const property = value.locator("..");
    await property.focus();
    await property.press("Shift+F10");
    await page.getByRole("menuitem", { name: "Copy value", exact: true }).press("Escape");
    assert.equal(
      await property.evaluate((node) => node.getRootNode().activeElement === node),
      true,
    );
    await warning.getByText('"":', { exact: true }).click({ button: "right" });
    await page.getByRole("menuitem", { name: "Copy name", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "");
    await warning.getByText("first\nsecond", { exact: true }).click({ button: "right" });
    await page.getByRole("menuitem", { name: "Copy value", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "first\\nsecond");
    await warning.locator("small").first().click({ button: "right" });
    await page.getByRole("menuitem", { name: "Copy message", exact: true }).click();
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      "Slow response [Object]",
    );
    await page.evaluate(() => navigator.clipboard.writeText("Retain this clipboard"));
    const output = workspace.locator(".console-output");
    await output.focus();
    await output.press("Shift+F10");
    await page.getByRole("menuitem", { name: "Copy visible output", exact: true }).waitFor();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.logs[0].records.push({
        sequence: 6,
        timeUnixMs: Date.now() + 10,
        level: "info",
        text: "New message",
        frames: [],
        truncated: false,
      });
    });
    await workspace.evaluate((element) => element.refresh());
    await page.getByRole("menuitem", { name: "Copy visible output", exact: true }).click();
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      "Retain this clipboard",
    );
    await workspace.getByRole("button", { name: "Console levels", exact: true }).click();
    await workspace.getByRole("checkbox", { name: "Verbose", exact: true }).check();
    await workspace.getByRole("checkbox", { name: "Info", exact: true }).uncheck();
    await workspace.getByRole("checkbox", { name: "Verbose", exact: true }).uncheck();
    assert.equal(await workspace.getByText("Request matched", { exact: true }).count(), 0);
    await workspace.getByRole("checkbox", { name: "Warnings", exact: true }).press("Escape");
    assert.equal(await value.isVisible(), true, "Filtering preserves expanded snapshot branches");
    for (const viewport of [
      { width: 1440, height: 900 },
      { width: 800, height: 600 },
    ]) {
      await page.setViewportSize(viewport);
      await warning.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/console-messages-${viewport.width}.png`),
      });
    }
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/console-messages-forced-colors.png"),
    });
    await page.emulateMedia({ forcedColors: "none" });
    await page.setViewportSize({ width: 1440, height: 900 });
    await workspace.getByRole("button", { name: "Console levels", exact: true }).click();
    await workspace.getByRole("checkbox", { name: "Info", exact: true }).check();
    await workspace.getByRole("checkbox", { name: "Verbose", exact: true }).check();
    await workspace.getByText("Request details", { exact: true }).waitFor();
    await workspace.getByRole("checkbox", { name: "Verbose", exact: true }).uncheck();
    await workspace.getByRole("checkbox", { name: "Info", exact: true }).press("Escape");
  } finally {
    await page.evaluate((logs) => {
      globalThis.__workspaceFixture.workspaceDebug.logs = logs;
    }, previous);
    await workspace.evaluate((element) => element.refresh());
  }
}
