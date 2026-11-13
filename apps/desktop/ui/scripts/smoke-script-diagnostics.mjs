import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";
import {
  checkScriptApiReference,
  checkScriptAuthoringLayout,
  testInSandbox,
} from "./check-script-authoring.mjs";

// Run only through the owned-profile Windows desktop harness.
const argument = (name) => process.argv[process.argv.indexOf(name) + 1];
const { chromium } = createRequire(
  new URL("../../../../e2e/playwright/package.json", import.meta.url),
)("playwright");
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${argument("--port")}`);
const page = browser.contexts()[0].pages()[0];
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));

async function checkNativeStreaming(scripts) {
  const prior = await scripts.evaluate((editor) => ({
    source: editor.sourceEditor.getValue(),
    read: editor.scriptForm.elements.namedItem("readBodies").checked,
    write: editor.scriptForm.elements.namedItem("writeBody").checked,
    headers: editor.scriptForm.elements.namedItem("scriptHeaders").value,
    mode: editor.testMode,
    response: editor.scriptScenarioResponseBody.value,
    sizes: editor.scriptScenarioChunkSizes.value,
    delay: editor.scriptScenarioChunkDelay.value,
    duration: editor.scriptScenarioDuration.value,
  }));
  const source = `import { pass, edit, needMore, deferResponseHeaders, replaceTextChunk, setHeader } from "transmog:api";
import type { ResponseHeadEvent, ResponseHeadResult, ResponseChunkEvent, ResponseChunkResult } from "transmog:api";
export function onResponseHead(event: ResponseHeadEvent): ResponseHeadResult { return deferResponseHeaders(); }
export function onResponseChunk(event: ResponseChunkEvent): ResponseChunkResult {
  console.info("native chunk window", event.bytes.length, event.end);
  if (event.canEditHeaders) {
    if (event.bytes.length < 4 && !event.end) return needMore();
    return edit({headers:[setHeader("x-native-inspected", "yes")]});
  }
  return replaceTextChunk(event, "before", "after");
}`;
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await scripts.evaluate((editor, source) => {
      editor.scriptForm.elements.namedItem("readBodies").checked = true;
      editor.scriptForm.elements.namedItem("writeBody").checked = true;
      editor.scriptForm.elements.namedItem("scriptHeaders").value = "x-native-inspected";
      editor.sourceEditor.setValue(source);
    }, source);
    await scripts
      .getByRole("combobox", { name: "Test mode", exact: true })
      .selectOption("scenario");
    await scripts.getByLabel("Scenario duration (ms)", { exact: true }).fill("100");
    await scripts.getByLabel("Response body", { exact: true }).fill("before-before");
    await scripts
      .getByLabel("Response chunk byte counts (optional)", { exact: true })
      .fill("2, 5, 2, 4");
    await scripts.getByLabel("Delay between chunks (ms, simulated)", { exact: true }).fill("5");
    await testInSandbox(scripts);
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").automation.querySelector("script-editor").scriptBusy,
    );
    assert.deepEqual(await scripts.evaluate((editor) => editor.typeScriptIssues), []);
    assert.match(await scripts.locator(".script-test-result").textContent(), /Scenario passed/);
    const result = await scripts.evaluate((editor) =>
      JSON.parse(editor.testEvidence.split("Scenario evidence:\n")[1]),
    );
    assert.equal(new TextDecoder().decode(Uint8Array.from(result.responseBody)), "before-after");
    assert.equal(result.headersReleasedAtMs, 55);
    assert.equal(
      result.response.headers.some((header) => header.name === "x-native-inspected"),
      true,
    );
    assert.ok(result.streamCalls.some((call) => call.action === "need-more"));
    assert.equal(result.streamCalls.at(-1).end, true);
    const reports = result.steps.find((step) => step.handler === "onResponseChunk").reports;
    const messages = reports.flatMap((report) =>
      (report.logs ?? []).flatMap((run) => run.records.map((record) => record.text)),
    );
    assert.ok(messages.includes("native chunk window 2 false"));
    assert.ok(messages.includes("native chunk window 0 true"));
    const details = scripts.locator(".script-test-result script-diagnostics");
    const wasOpen = (await details.locator(".developer-details").getAttribute("open")) !== null;
    if (!wasOpen) await details.locator(".developer-details > summary").click();
    await details.getByText("native chunk window 2 false", { exact: true }).first().waitFor();
    await details.getByText("native chunk window 0 true", { exact: true }).waitFor();
    assert.equal(
      await details.getByText("native chunk window 2 false", { exact: true }).count(),
      2,
    );
    for (const width of [1440, 800]) {
      await page.setViewportSize({ width, height: width === 1440 ? 900 : 600 });
      await details.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(argument("--output"), `script-chunk-diagnostics-native-${width}.png`),
      });
    }
    if (!wasOpen) await details.locator(".developer-details > summary").click();
    await scripts.getByText("Streaming callbacks", { exact: true }).click();
    for (const width of [1440, 800]) {
      await page.setViewportSize({ width, height: width === 1440 ? 900 : 600 });
      await scripts.locator(".script-test-result table").scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(argument("--output"), `script-streaming-native-${width}.png`),
      });
    }
  } finally {
    await scripts.evaluate((editor, prior) => {
      editor.scriptForm.elements.namedItem("readBodies").checked = prior.read;
      editor.scriptForm.elements.namedItem("writeBody").checked = prior.write;
      editor.scriptForm.elements.namedItem("scriptHeaders").value = prior.headers;
      editor.scriptScenarioResponseBody.value = prior.response;
      editor.scriptScenarioChunkSizes.value = prior.sizes;
      editor.scriptScenarioChunkDelay.value = prior.delay;
      editor.scriptScenarioDuration.value = prior.duration;
      editor.testMode = prior.mode;
      editor.sourceEditor.setValue(prior.source);
    }, prior);
  }
}
try {
  await page.waitForFunction(
    () => typeof document.querySelector("app-shell")?.activateView === "function",
  );
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    if (!(await shell.activateView("automation"))) throw new Error(shell.diagnosticText);
    await shell.automation.showAutomationSection("scripts");
    await shell.automation.$whenRendered();
  });
  const scripts = page.locator("script-editor");
  await page.waitForFunction(
    () => document.querySelector("app-shell").automation.querySelector("script-editor").scriptReady,
  );
  const priorSource = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  const source = [
    "console.info('native initializer log');",
    "export function onRequestHead(): never {",
    "  console.debug('native before throw');",
    "  console.trace('native trace');",
    "  throw new Error('native outer detail', {cause: new TypeError('native inner detail')});",
    "}",
  ].join("\n");
  try {
    await scripts.evaluate((editor, source) => editor.sourceEditor.setValue(source), source);
    await checkScriptApiReference(page, scripts, argument("--output"));
    await page.keyboard.press("Escape");
    await checkScriptAuthoringLayout(
      page,
      scripts,
      scripts.locator("editor-surface").first(),
      argument("--output"),
    );
    const input = scripts.locator(".script-test");
    await input.locator(":scope > summary").click();
    await testInSandbox(scripts);
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").automation.querySelector("script-editor").scriptBusy,
    );
    const issues = await scripts.evaluate((editor) => editor.typeScriptIssues);
    assert.deepEqual(issues, [], "Native diagnostic fixture did not type-check");
    const details = scripts.locator(".script-test-result script-diagnostics");
    await details.locator(".developer-details > summary").click();
    await details.getByText("Error: native outer detail", { exact: true }).waitFor();
    assert.match(await details.textContent(), /native inner detail/);
    assert.match(await details.textContent(), /native initializer log/);
    assert.match(await details.textContent(), /native before throw/);
    assert.match(await details.textContent(), /native trace/);
    // Edit the draft after execution, then navigate a retained frame.
    await scripts.evaluate((editor) =>
      editor.sourceEditor.setValue(
        "export function onRequestHead() { return {decision:{action:'continue'}}; }",
      ),
    );
    const frame = details.locator(".developer-exception .developer-frame").first();
    await frame.click();
    const dialog = details.getByRole("dialog", { name: "Executing source snapshot" });
    await dialog
      .getByText(
        "  throw new Error('native outer detail', {cause: new TypeError('native inner detail')});",
        { exact: true },
      )
      .waitFor();
    assert.match(await dialog.textContent(), /Read-only executing snapshot/);
    for (const [name, width, height] of [
      ["wide", 1440, 900],
      ["compact", 960, 720],
      ["small", 800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      assert.equal(
        await dialog.getByRole("button", { name: "Close", exact: true }).isVisible(),
        true,
      );
      await page.screenshot({
        path: resolve(argument("--output"), `script-source-native-${name}.png`),
      });
    }
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    assert.equal(await frame.evaluate((button) => button.matches(":focus")), true);
    assert.equal(
      await scripts.evaluate((editor) =>
        editor.sourceEditor.getValue().includes("native outer detail"),
      ),
      false,
    );
    await checkNativeStreaming(scripts);
  } finally {
    await scripts.evaluate((editor, source) => editor.sourceEditor.setValue(source), priorSource);
  }

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.locator('a[data-view="traffic"]').click();
  const traffic = page.locator("traffic-workspace");
  await traffic.evaluate(async (traffic, trace) => {
    await traffic.importTrace(trace);
    const row = traffic.sessions.find((row) => row.path === "/extension-fixture");
    if (!row) throw new Error("Native diagnostic fixture did not import");
    traffic.preferences = { ...traffic.preferences, layout: "stacked", listSplit: 30 };
    await traffic.selectTraffic(row, new MouseEvent("click"));
  }, argument("--trace"));
  const request = traffic.locator('message-inspector[side="request"]');
  const select = request.getByRole("combobox", { name: "Headers viewer", exact: true });
  await select.selectOption({ label: "Example headers" });
  const visualizer = request.locator('extension-visualizer[slot$="headers"]');
  const developer = visualizer.locator("script-diagnostics");
  await developer.locator(".developer-details > summary").click();
  assert.equal(
    await visualizer.locator(".visualizer-heading").count(),
    0,
    "Startup failure pretended to have a valid document",
  );
  assert.match(await developer.textContent(), /Management must not evaluate extension code/);
  assert.match(await developer.textContent(), /native initializer log/);
  assert.doesNotMatch(
    await visualizer
      .locator(".visualizer-feedback")
      .allTextContents()
      .then((values) => values.join(" ")),
    /Management must not evaluate extension code|native initializer log/,
  );
  await developer.locator(".developer-exception .developer-frame").first().click();
  const sourceDialog = developer.getByRole("dialog", { name: "Executing source snapshot" });
  await sourceDialog.getByText(/Management must not evaluate extension code/).waitFor();
  await page.screenshot({
    path: resolve(argument("--output"), "visualizer-startup-source-native.png"),
  });
  await page.keyboard.press("Escape");
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(argument("--output"), "script-diagnostics-native.json"),
    JSON.stringify(
      {
        trafficThrow: true,
        causes: true,
        console: true,
        immutableSourceAfterDraftEdit: true,
        keyboardReturnFocus: true,
        visualizerStartup: true,
        sourceSnapshots: true,
        streamingScenario: true,
        pageErrors: errors,
      },
      null,
      2,
    ) + "\n",
  );
} catch (error) {
  await page.screenshot({
    path: resolve(argument("--output"), "script-diagnostics-native-failure.png"),
  });
  await writeFile(
    resolve(argument("--output"), "script-diagnostics-native-failure.json"),
    JSON.stringify(
      {
        message: error.message,
        pageErrors: errors,
        url: page.url(),
        state: await page.evaluate(() => {
          const shell = document.querySelector("app-shell");
          return {
            view: shell?.activeView,
            automation: !!shell?.automation,
            section: shell?.automation?.automationSection,
            scripts: !!shell?.automation?.querySelector("script-editor"),
          };
        }),
      },
      null,
      2,
    ) + "\n",
  );
  throw error;
} finally {
  await browser.close();
}
