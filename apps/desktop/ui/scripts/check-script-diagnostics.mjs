import { testInSandbox } from "./check-script-authoring.mjs";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";

/** Production template coverage for inert evidence, source identity and stale reads. */
export async function checkScriptDiagnostics(page, root) {
  const scripts = page.locator("script-editor");
  await page.waitForFunction(
    () => document.querySelector("app-shell").shadowRoot.querySelector("script-editor").scriptReady,
  );
  const source = "file:///transmog-scripts/user-script/main.ts";
  const generated = "file:///transmog-script/main.js";
  const frame = {
    function: "nested",
    generated: { source: generated, line: 2, column: 3 },
    original: { source, line: 2, column: 3 },
  };
  const log = {
    context: {
      artifact: "exact-executed-artifact",
      worker: "worker-one",
      revision: "build-one",
      callback: "onRequestHead",
      invocation: 1,
      exchangeId: "editor-test",
    },
    records: Array.from({ length: 105 }, (_, index) => ({
      sequence: index + 1,
      timeUnixMs: index === 1 ? 1e19 : 1700000000000 + index,
      level: index === 1 ? "error" : "debug",
      text: `accepted-${index} <img src=x onerror=alert(1)>`,
      frames: index === 0 ? [frame] : [],
      truncated: index === 104,
    })),
    dropped: 5,
    incomplete: true,
  };
  const exception = {
    artifact: "exact-executed-artifact",
    exception: {
      name: "Error",
      message: "authored <script>alert(1)</script>",
      frames: [frame],
      unavailable: true,
    },
    causes: [
      {
        name: "TypeError",
        message: "inner cause",
        frames: [
          {
            function: "helper",
            generated: { source: "transmog:invoke", line: 1, column: 1 },
            mappingIssue: "missing",
          },
        ],
        unavailable: false,
      },
    ],
    truncated: true,
  };
  await page.evaluate(
    ({ log, exception }) =>
      Object.assign(globalThis.__workspaceFixture, {
        scriptDeveloperError: { message: "Host-controlled failure", exception, logs: [log] },
        deferScriptTest: false,
      }),
    { log, exception },
  );
  const input = scripts.locator(".script-test");
  if (!(await input.evaluate((element) => element.open)))
    await input.locator(":scope > summary").click();
  await testInSandbox(scripts);
  const details = scripts.locator(".script-test-result script-diagnostics");
  await details.locator(".developer-details > summary").click();
  assert.match(await details.textContent(), /inner cause/);
  assert.match(await details.textContent(), /5 messages omitted/);
  assert.match(await details.textContent(), /stream incomplete/);
  assert.equal(await details.locator("img,script").count(), 0, "Authored evidence became markup");
  assert.equal(await details.locator(".developer-record").count(), 100);
  await details.getByRole("button", { name: "Next console page" }).click();
  assert.equal(await details.locator(".developer-record").count(), 5);
  await details.getByRole("combobox", { name: "Console level" }).selectOption("error");
  assert.equal(
    await scripts.evaluate((editor) => editor.testOutdated),
    false,
    "Changing a console filter changed the sandbox input version",
  );
  assert.equal(await details.locator(".developer-record").count(), 1);
  assert.match(await details.locator(".developer-record").textContent(), /Timestamp unavailable/);
  await details.getByRole("button", { name: "Copy developer details" }).click();
  assert.match(await page.evaluate(() => navigator.clipboard.readText()), /accepted-104/);

  const original = details.getByRole("button", { name: `${source}:2:3`, exact: true }).first();
  await original.click();
  const dialog = details.getByRole("dialog", { name: "Executing source snapshot" });
  await dialog.getByText("  throw new Error('executed snapshot');", { exact: true }).waitFor();
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.lastScriptSource), {
    artifact: "exact-executed-artifact",
    source,
    line: 2,
    column: 3,
  });
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    assert.ok(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth));
    assert.equal(
      await dialog.getByRole("button", { name: "Close", exact: true }).isVisible(),
      true,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-source-${name}.png`),
    });
  }
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/script-source-dark.png") });
  await page.emulateMedia({ colorScheme: "light" });
  // Half the CSS viewport at twice the device scale exercises 200% equivalent layout.
  const metrics = await page.context().newCDPSession(page);
  try {
    await metrics.send("Emulation.setDeviceMetricsOverride", {
      width: 400,
      height: 300,
      deviceScaleFactor: 2,
      mobile: false,
    });
    assert.equal(await page.evaluate(() => innerWidth), 400);
    assert.ok(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth));
    assert.equal(
      await dialog.getByRole("button", { name: "Close", exact: true }).isVisible(),
      true,
    );
    const screenshot = await metrics.send("Page.captureScreenshot", {
      format: "png",
      captureBeyondViewport: false,
    });
    await writeFile(
      resolve(root, "../../../target/ui-check/script-source-zoom.png"),
      Buffer.from(screenshot.data, "base64"),
    );
  } finally {
    await metrics.send("Emulation.clearDeviceMetricsOverride");
    await metrics.detach();
    await page.setViewportSize({ width: 800, height: 600 });
  }
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  assert.equal(await original.evaluate((button) => button.matches(":focus")), true);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.scriptSourceError = true;
  });
  await details
    .getByRole("button", { name: `Generated: ${generated}:2:3`, exact: true })
    .first()
    .click();
  await dialog.getByText("The exact source snapshot is unavailable", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await page.evaluate(() =>
    Object.assign(globalThis.__workspaceFixture, {
      scriptSourceError: false,
      deferScriptSource: true,
    }),
  );
  await original.click();
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.finishScriptSource();
    globalThis.__workspaceFixture.deferScriptSource = false;
  });
  assert.equal(await dialog.isVisible(), false, "A late source read reopened its dialog");
  await details.getByRole("button", { name: "Clear displayed logs" }).click();
  assert.equal(await details.locator(".developer-record").count(), 0);
  await details.getByRole("button", { name: "Copy developer details" }).click();
  assert.doesNotMatch(await page.evaluate(() => navigator.clipboard.readText()), /accepted-104/);
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/script-details-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() =>
    Object.assign(globalThis.__workspaceFixture, {
      scriptDeveloperError: null,
      scriptTestLogs: [],
      scriptTestDiagnostics: [],
    }),
  );
}
