import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { execFile, spawn } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";
import { resolve } from "node:path";
const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const argument = (name) => process.argv[process.argv.indexOf(name) + 1];
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
const context = browser.contexts()[0],
  main = context.pages()[0];
const host = main.locator("app-shell converter-host");
const windows = [];
let restarted, secondBrowser;
try {
  for (let index = 0; index < 4; index++) {
    await host.getByRole("button", { name: "Format converter", exact: true }).click();
    await host
      .getByRole("textbox", { name: "Converter input", exact: true })
      .fill(`native ${index}`);
    await host.getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
    const created = context.waitForEvent("page");
    await host.getByRole("button", { name: "Open in new window", exact: true }).click();
    const page = await created;
    windows.push(page);
    await page.getByRole("textbox", { name: "Converter input", exact: true }).waitFor();
    await page.waitForFunction(() => document.querySelector("converter-host")?.converterReady);
    assert.equal(
      await page.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
      `native ${index}`,
    );
    assert.equal(
      await page.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
      Buffer.from(`native ${index}`).toString("base64"),
    );
    for (const command of [
      "start_proxy",
      "query_sessions",
      "change_response_transform",
      "record_frontend_diagnostic",
    ]) {
      assert.equal(
        await page.evaluate(async (command) => {
          try {
            await window.__TAURI_INTERNALS__.invoke(command, {});
            return false;
          } catch {
            return true;
          }
        }, command),
        true,
        `${command} was allowed in a converter window`,
      );
    }
    if (index === 0) {
      await main.bringToFront();
      assert.equal(await main.evaluate(() => document.hasFocus()), true);
      assert.equal(
        await page.evaluate(async () => {
          try {
            await window.__TAURI_INTERNALS__.invoke("acknowledge_converter_window");
            return false;
          } catch {
            return true;
          }
        }),
        true,
        "A completed handoff must not be acknowledged again",
      );
      assert.equal(
        await main.evaluate(() => document.hasFocus()),
        true,
        "A repeated handoff must not steal focus",
      );
    }
  }
  await windows[0]
    .getByRole("textbox", { name: "Converter input", exact: true })
    .fill("independent");
  await windows[0].waitForFunction(
    () => document.querySelector("converter-host").converterOutput === "aW5kZXBlbmRlbnQ=",
  );
  assert.equal(
    await windows[1].getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "native 1",
  );
  const closed = windows[1].waitForEvent("close");
  await windows[1]
    .evaluate(() => window.__TAURI_INTERNALS__.invoke("close_converter_window"))
    .catch(() => undefined);
  await closed;
  await main.screenshot({ path: resolve(argument("--output"), "converter-main.png") });
  await windows[0].screenshot({ path: resolve(argument("--output"), "converter-native.png") });
  const originClosed = main.waitForEvent("close");
  await promisify(execFile)(
    "pwsh.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-File",
      argument("--close-helper"),
      "-ProbeProcessId",
      argument("--process-id"),
      "-WindowTitle",
      "Transmog",
    ],
    { windowsHide: true },
  );
  await originClosed;
  const survivor = windows[0];
  await survivor
    .getByRole("textbox", { name: "Converter input", exact: true })
    .fill("after origin close");
  await survivor.waitForFunction(
    () => document.querySelector("converter-host").converterOutput === "YWZ0ZXIgb3JpZ2luIGNsb3Nl",
  );
  const created = context.waitForEvent("page");
  await survivor.getByRole("button", { name: "Open in new window", exact: true }).click();
  const another = await created;
  windows.push(another);
  await another.waitForFunction(() => document.querySelector("converter-host")?.converterReady);
  assert.equal(
    await another.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "after origin close",
  );
  for (const page of windows.filter((page) => !page.isClosed()))
    await page
      .evaluate(() => window.__TAURI_INTERNALS__.invoke("close_converter_window"))
      .catch(() => undefined);
  await browser.close();
  for (let attempt = 0; attempt < 50; attempt++) {
    try {
      process.kill(Number(argument("--process-id")), 0);
    } catch {
      break;
    }
    await delay(100);
  }
  restarted = spawn(argument("--executable"), [], { windowsHide: true, stdio: "ignore" });
  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      const response = await fetch(`http://127.0.0.1:${argument("--port")}/json/version`);
      if (response.ok) break;
    } catch {}
    if (restarted.exitCode !== null)
      throw new Error("Restarted converter probe exited during startup.");
    await delay(200);
  }
  secondBrowser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
  const fresh = secondBrowser.contexts()[0].pages()[0];
  await fresh.getByRole("button", { name: "Format converter", exact: true }).click();
  assert.equal(
    await fresh.getByRole("textbox", { name: "Converter input", exact: true }).inputValue(),
    "",
  );
  assert.equal(
    await fresh.getByRole("textbox", { name: "Converter output", exact: true }).inputValue(),
    "",
  );
  assert.equal(await fresh.getByLabel("Conversion", { exact: true }).inputValue(), "");
  const freshClosed = fresh.waitForEvent("close");
  await promisify(execFile)(
    "pwsh.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-File",
      argument("--close-helper"),
      "-ProbeProcessId",
      String(restarted.pid),
      "-WindowTitle",
      "Transmog",
    ],
    { windowsHide: true },
  );
  await freshClosed;
  process.stdout.write(
    "Native converters preserve independent drafts, survive the origin, create more windows, deny unrelated commands, and clear contents after restart.\n",
  );
} finally {
  await secondBrowser?.close();
  await browser.close();
  if (restarted && restarted.exitCode === null) restarted.kill();
}
