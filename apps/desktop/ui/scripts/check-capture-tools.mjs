import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkCaptureTools(page, root, view) {
  await view("captures");
  const captures = page.locator("capture-workspace");
  const task = (name) =>
    captures.locator(".capture-tabs").getByRole("button", { name, exact: true });
  const source = captures.getByLabel("Source file (.tmcap)", { exact: true });
  const destination = captures.getByLabel("Destination file", { exact: true });
  const idle = () =>
    page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("capture-workspace")
          .captureBusy,
    );
  const exports = () =>
    page.evaluate(() => globalThis.__workspaceFixture.calls.export_capture ?? 0);
  const failCaptureStatus = () =>
    page.evaluate(async () => {
      const workspace = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("capture-workspace");
      while (workspace.refreshPending) await new Promise((resolve) => setTimeout(resolve, 10));
      clearTimeout(workspace.timer);
      globalThis.__workspaceFixture.captureStatusError = true;
      try {
        await workspace.refreshCapture();
      } finally {
        delete globalThis.__workspaceFixture.captureStatusError;
      }
    });
  const before = await exports();

  await task("Convert").click();
  assert.equal(await source.inputValue(), "");
  assert.equal(await destination.inputValue(), "");
  await captures.getByRole("button", { name: "Convert file", exact: true }).click();
  assert.equal(await exports(), before, "An empty conversion reached the backend");
  assert.equal(
    await source.evaluate((input) => input.validity.valueMissing && input.matches(":focus")),
    true,
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/capture-convert-empty.png"),
  });

  await page.evaluate(() => (globalThis.__workspaceFixture.capturePick = null));
  await captures.getByRole("button", { name: "Browse…", exact: true }).click();
  await idle();
  assert.equal(await source.inputValue(), "", "Canceling the picker invented a source");
  const sourcePath =
    "C:/captures/Customer investigation with a long descriptive name/session.tmcap";
  await page.evaluate((path) => (globalThis.__workspaceFixture.capturePick = path), sourcePath);
  await captures.getByRole("button", { name: "Browse…", exact: true }).click();
  await idle();
  assert.equal(await source.inputValue(), sourcePath);
  await source.press("Enter");
  assert.equal(await exports(), before, "A missing destination reached the backend");
  assert.equal(
    await destination.evaluate((input) => input.validity.valueMissing && input.matches(":focus")),
    true,
  );
  await captures.getByLabel("Format", { exact: true }).selectOption("json-lines");
  assert.equal(await destination.inputValue(), "", "Changing format invented a destination");
  await page.evaluate(
    () => (globalThis.__workspaceFixture.capturePick = "C:/captures/converted.jsonl"),
  );
  await captures.getByRole("button", { name: "Choose…", exact: true }).click();
  await idle();

  const trafficState = () =>
    page.evaluate(() => {
      const traffic = document.querySelector("app-shell").traffic;
      return {
        selected: traffic.selectedSessionId,
        selectedIds: [...traffic.trafficSelection.ids].sort(),
        search: traffic.searchInput.value,
        filter: traffic.liveFilter,
        count: traffic.selectedTrafficCount,
      };
    });
  const previousTraffic = await trafficState();
  await task("Export").focus();
  await page.keyboard.press("Enter");
  assert.equal(await task("Export").getAttribute("aria-pressed"), "true");
  assert.equal(await source.isVisible(), false);
  assert.match(
    await captures.getByRole("region", { name: "Export traffic" }).innerText(),
    /Traffic → Files → Save copy…/,
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/capture-export-${width}.png`),
    });
    assert.equal(
      await captures.evaluate((element) => {
        const content = element.querySelector(".capture-content");
        return content.scrollWidth <= content.clientWidth + 1;
      }),
      true,
      "Export instructions overflow horizontally",
    );
  }
  const openFiles = captures.getByRole("button", { name: "Open Traffic Files", exact: true });
  await failCaptureStatus();
  await captures.getByRole("button", { name: "Dismiss capture error", exact: true }).click();
  assert.equal(
    await openFiles.evaluate((button) => button.matches(":focus")),
    true,
    "Dismissing an Export error left focus on hidden content",
  );
  await openFiles.focus();
  await page.keyboard.press("Enter");
  const menu = page.locator("#trace-files-menu");
  await menu.waitFor({ state: "visible" });
  assert.deepEqual(
    await trafficState(),
    previousTraffic,
    "Export navigation changed Traffic selection or filters",
  );
  await page.keyboard.press("Escape");
  assert.equal(
    await page
      .getByRole("button", { name: "Files", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  await view("captures");
  await openFiles.click();
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  assert.equal(
    await menu
      .getByRole("button", { name: "Save copy…", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
    "Keyboard navigation did not enter the Files menu",
  );
  await page.keyboard.press("Enter");
  await page.locator(".trace-save-dialog").waitFor({ state: "visible" });
  await page.keyboard.press("Escape");
  await view("captures");
  await task("Convert").click();
  assert.equal(await source.inputValue(), sourcePath, "Navigation lost the conversion draft");
  assert.equal(await destination.inputValue(), "C:/captures/converted.jsonl");
  assert.equal(await captures.getByLabel("Format", { exact: true }).inputValue(), "json-lines");

  await page.evaluate(() => (globalThis.__workspaceFixture.captureExportError = true));
  await captures.getByRole("button", { name: "Convert file", exact: true }).click();
  await page
    .getByRole("alertdialog", { name: "File conversion failed", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await idle();
  assert.equal(await source.inputValue(), sourcePath);
  assert.equal(await destination.inputValue(), "C:/captures/converted.jsonl");
  await captures.getByRole("button", { name: "Dismiss capture error", exact: true }).click();
  assert.equal(await source.evaluate((input) => input.matches(":focus")), true);
  await page.evaluate(() => delete globalThis.__workspaceFixture.captureExportError);
  await captures.getByRole("button", { name: "Convert file", exact: true }).click();
  await idle();
  assert.match(await captures.locator(".capture-feedback").innerText(), /Converted copy saved/);
  assert.equal(await exports(), before + 2);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/capture-convert-${width}.png`),
    });
  }
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/capture-convert-dark.png"),
  });
  await task("Export").click();
  assert.equal(await captures.locator(".capture-receipt").isVisible(), false);
  const zoom = await page.evaluate(() => {
    const previous = document.documentElement.style.zoom;
    document.documentElement.style.zoom = "1.5";
    return previous;
  });
  try {
    await openFiles.scrollIntoViewIfNeeded();
    const bounds = await openFiles.boundingBox();
    assert.ok(
      bounds.x >= 0 && bounds.x + bounds.width <= 800,
      "Export action is clipped at 150% zoom",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/capture-export-zoom.png"),
    });
  } finally {
    await page.evaluate((previous) => (document.documentElement.style.zoom = previous), zoom);
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/capture-export-forced-colors.png"),
  });
  await page.emulateMedia({ colorScheme: "light", forcedColors: "none" });

  await task("Inspect / recover").click();
  const inspectSource = captures.getByLabel("Trace file", { exact: true });
  assert.equal(await inspectSource.inputValue(), "");
  const inspections = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.inspect_trace_metadata ?? 0,
  );
  await captures.getByRole("button", { name: "Inspect trace metadata", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.inspect_trace_metadata ?? 0),
    inspections,
  );
  assert.equal(await inspectSource.evaluate((input) => input.validity.valueMissing), true);
  await task("Record to file").click();
  await failCaptureStatus();
  await captures.getByRole("button", { name: "Dismiss capture error", exact: true }).click();
  assert.equal(
    await task("Record to file").evaluate((button) => button.matches(":focus")),
    true,
    "Dismissing a recording error left focus on hidden content",
  );
  await page.setViewportSize({ width: 1280, height: 800 });
  console.log(
    "Capture tools: empty paths, picker cancellation, conversion retry, Export navigation and responsive layouts verified.",
  );
}
