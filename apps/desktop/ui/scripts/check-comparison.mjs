import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkComparison(page, root) {
  await page.addInitScript(() => sessionStorage.setItem("startup-update-tested", "true"));
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  await page.evaluate(() => {
    const traffic = document.querySelector("app-shell").traffic;
    traffic.trafficSelection.ids = new Set(["first", "second"]);
    traffic.updateTrafficSelection();
  });
  await page
    .locator(".traffic-selection-bar")
    .getByRole("button", { name: "Compare requests", exact: true })
    .click();
  const dialog = page.locator("comparison-workspace .comparison-dialog").first();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.comparisonBusy);
  assert.match(await dialog.textContent(), /fixture-left.saz/);
  assert.match(await dialog.textContent(), /No source body retained/);
  assert.equal(await dialog.locator("script").count(), 0);
  const fieldDetails = dialog.locator(".comparison-fields");
  await fieldDetails.locator("summary").click();
  await dialog.getByRole("combobox", { name: "Field comparison format" }).selectOption("msgpack");
  const starts = await page.evaluate(
    () => globalThis.__workspaceFixture.visualizerStarts?.length ?? 0,
  );
  await dialog.getByRole("button", { name: "Compare fields", exact: true }).click();
  const shared = fieldDetails.locator("extension-visualizer");
  await shared
    .locator(".visualizer-heading > strong")
    .filter({ hasText: "field comparison" })
    .waitFor();
  assert.equal(await shared.locator("script").count(), 0);
  assert.equal(await shared.evaluate((element) => element.view), null);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.visualizerStarts?.length ?? 0),
    starts,
    "Native presentation started an extension worker",
  );
  const tree = shared.getByRole("tree");
  await tree.focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await shared.getByRole("region", { name: "Selected structure" }).waitFor();
  await shared.getByRole("combobox", { name: "Structure view" }).selectOption("table");
  await shared.getByRole("combobox", { name: "Structure view" }).selectOption("text");
  await page.keyboard.press("Escape");
  await shared.getByRole("button", { name: "About this visualizer", exact: true }).click();
  await shared
    .getByRole("dialog", { name: "About this visualizer" })
    .getByText("Built-in field comparison", { exact: true })
    .waitFor();
  await page.keyboard.press("Escape");
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await shared.scrollIntoViewIfNeeded();
    assert.ok(await tree.isVisible());
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/native-fields-${width}.png`),
    });
  }
  await page.evaluate(() => (globalThis.__workspaceFixture.deferFields = true));
  await dialog.getByRole("button", { name: "Compare fields", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseFields);
  await dialog.getByRole("combobox", { name: "Field comparison format" }).selectOption("bson");
  await page.evaluate(() => globalThis.__workspaceFixture.releaseFields());
  await page.waitForFunction(
    () => !document.querySelector("app-shell").comparison.comparisonFieldsBusy,
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonFields),
    null,
    "Late field results replaced a newer format choice",
  );
  await fieldDetails.locator("summary").click();
  await page.setViewportSize({ width: 1440, height: 900 });
  await fieldDetails.locator("summary").click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferFields = true;
    globalThis.__workspaceFixture.releaseFields = null;
  });
  await dialog.getByRole("button", { name: "Compare fields", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseFields);
  await dialog.getByRole("button", { name: "Cancel field comparison", exact: true }).click();
  await fieldDetails
    .getByText("Field comparison canceled; the previous completed result is preserved.", {
      exact: true,
    })
    .waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.releaseFields());
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonFields),
    null,
  );
  await fieldDetails.locator("summary").click();
  await dialog.getByRole("button", { name: "Next change", exact: true }).click();
  assert.match(await dialog.textContent(), /Change at evidence row 3 of 70/);
  await dialog.getByRole("button", { name: "Next change", exact: true }).click();
  assert.match(await dialog.textContent(), /Change at evidence row 68 of 70/);
  await dialog.getByRole("button", { name: "Previous change", exact: true }).click();
  assert.match(await dialog.textContent(), /Change at evidence row 3 of 70/);
  await dialog.getByText("Boundaries and comparison options", { exact: true }).click();
  await dialog
    .getByLabel("Exclude header names (comma separated)", { exact: true })
    .fill("Date, X-Request-ID");
  await dialog.getByRole("checkbox", { name: "Normalize newlines in derived text view" }).check();
  await dialog
    .getByRole("button", { name: "Apply options / refresh snapshot", exact: true })
    .click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.comparisonBusy);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonActive),
    2,
    "Refresh lost the selected change",
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.comparisonInput.options),
    { excludedHeaders: ["Date", "X-Request-ID"], normalizeNewlines: true },
  );
  await dialog.getByText("Boundaries and comparison options", { exact: true }).click();
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.evaluate(async () => {
      const c = document.querySelector("app-shell").comparison;
      c.comparisonOffset = 0;
      await c.page(0);
      c.comparisonActive = -1;
      await c.change(1);
    });
    await dialog.getByRole("button", { name: "Close", exact: true }).scrollIntoViewIfNeeded();
    const bounds = await dialog.boundingBox();
    assert.ok(bounds.height <= height && bounds.width <= width);
    const metrics = await dialog.evaluate((d) => {
      const c = document.querySelector("app-shell").comparison,
        active = d.querySelector("article[data-active]"),
        r = active?.getBoundingClientRect(),
        scroll = c.comparisonScroll.getBoundingClientRect();
      return {
        root: c.getRootNode().constructor.name,
        lookup: !!c.getRootNode().getElementById("comparison-change-" + c.comparisonActive),
        active: c.comparisonActive,
        activeId: active?.id,
        y: r?.y,
        bottom: r?.bottom,
        scrollY: scroll.y,
        scrollBottom: scroll.bottom,
        scrollTop: c.comparisonScroll.scrollTop,
      };
    });
    assert.ok(
      metrics.y >= metrics.scrollY - 1 && metrics.bottom <= metrics.scrollBottom + 1,
      JSON.stringify(metrics),
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/comparison-${width}.png`),
    });
  }
  await page.evaluate(() => (document.documentElement.style.zoom = "1.25"));
  await dialog.getByRole("button", { name: "Close", exact: true }).scrollIntoViewIfNeeded();
  const close = await dialog.getByRole("button", { name: "Close", exact: true }).boundingBox();
  assert.ok(close.y >= 0 && close.y + close.height <= 600);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/comparison-zoom.png") });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.emulateMedia({ forcedColors: "active", colorScheme: "dark" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/comparison-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none", colorScheme: "light" });
  const previous = await page.evaluate(
    () => document.querySelector("app-shell").comparison.comparisonResult.id,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferComparison = true;
    delete globalThis.__workspaceFixture.releaseComparison;
    void document.querySelector("app-shell").comparison.refreshComparison();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.releaseComparison);
  await dialog.getByRole("button", { name: "Cancel comparison", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.releaseComparison());
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.comparisonBusy);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonResult.id),
    previous,
    "Cancellation accepted a late completed result",
  );
  await page.evaluate(() => {
    const c = document.querySelector("app-shell").comparison;
    c.comparisonScroll.scrollTop = 0;
  });
  await dialog.getByText("Label and acquisition time", { exact: true }).first().click();
  await dialog.getByLabel("Left label", { exact: true }).fill("Working");
  assert.equal(await dialog.getByLabel("Left label", { exact: true }).inputValue(), "Working");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferComparison = true;
    delete globalThis.__workspaceFixture.releaseComparison;
    void document.querySelector("app-shell").comparison.refreshComparison();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.releaseComparison);
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => {
    const c = document.querySelector("app-shell").comparison;
    return !c.comparisonDialog.open && c.comparisonResult === null;
  });
  const files = page
    .locator("traffic-workspace")
    .getByRole("button", { name: "Files", exact: true });
  await files.focus();
  const closedBeforeLateResult = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.close_comparison ?? 0,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.releaseComparison();
  });
  await page.waitForFunction(
    (count) => (globalThis.__workspaceFixture.calls.close_comparison ?? 0) > count,
    closedBeforeLateResult,
  );
  assert.equal(await dialog.evaluate((d) => d.open), false);
  assert.equal(await files.evaluate((e) => e === e.getRootNode().activeElement), true);
  assert.ok(await page.evaluate(() => globalThis.__workspaceFixture.calls.cancel_comparison >= 1));
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic;
    t.trafficSelection.ids = new Set(["first"]);
    t.updateTrafficSelection();
    t.setComparisonBaseline();
    t.trafficSelection.ids = new Set(["second"]);
    t.updateTrafficSelection();
  });
  await page
    .locator(".content-search-status")
    .getByRole("button", { name: "Compare with baseline", exact: true })
    .click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.comparisonBusy);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.comparisonInput.left.id),
    "first",
  );

  const retainedComparison = await page.evaluate(
    () => document.querySelector("app-shell").comparison.comparisonResult.id,
  );
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    await shell.onComparisonSource({ detail: "not-retained", stopPropagation() {} });
  });
  assert.equal(await dialog.evaluate((d) => d.open), true, "Missing source closed the comparison");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonResult.id),
    retainedComparison,
  );
  assert.match(await dialog.textContent(), /source is no longer retained/);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.waitForFunction(
    () => document.querySelector("app-shell").comparison.subjects === null,
  );
  await page.evaluate(async () =>
    document.querySelector("app-shell").traffic.compareCaptureScopes(),
  );
  const scopes = page.locator("comparison-workspace .capture-comparison-dialog");
  await scopes.getByRole("button", { name: "Compare selected scopes", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.captureBusy);
  assert.match(await scopes.textContent(), /Left fixture snapshot/);
  await scopes.getByText("Frozen scope times and fidelity", { exact: true }).click();
  assert.match(await scopes.textContent(), /Stored formats: saz: 2/);
  await scopes.getByText("Frozen scope times and fidelity", { exact: true }).click();
  assert.match(await scopes.textContent(), /unresolved/);
  assert.match(await scopes.textContent(), /Bodies not compared/);
  await scopes.getByRole("button", { name: "Keep unmatched", exact: true }).first().click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.captureBusy);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.lastPairAction),
    "unmatched",
  );
  await scopes.getByRole("button", { name: "Undo pairing correction", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.captureBusy);
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.lastPairAction), "undo");
  await scopes.getByRole("button", { name: "Change pairing…", exact: true }).first().click();
  await scopes.getByRole("button", { name: "Pair selected requests", exact: true }).waitFor();
  await scopes.getByRole("button", { name: "Pair selected requests", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").comparison.captureBusy);
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.lastPairAction), "pair");
  await page.evaluate(() => {
    const c = document.querySelector("app-shell").comparison;
    c.captureResult = { ...c.captureResult, leftLabel: "Long filter snapshot " + "q".repeat(1024) };
  });
  await scopes.getByRole("button", { name: "Close", exact: true }).scrollIntoViewIfNeeded();
  const scopeOverflow = await scopes
    .locator(".comparison-scroll")
    .evaluate((e) => e.scrollWidth - e.clientWidth);
  assert.ok(scopeOverflow <= 1, "Long frozen scope label caused horizontal overflow");
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/comparison-capture-scopes.png"),
  });
  await scopes.getByRole("button", { name: "Close", exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 900 });
}
