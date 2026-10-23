import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

export async function checkTrafficPerformance(page, output) {
  await page.goto("https://workspace.test/");
  await page.locator("traffic-workspace").waitFor();
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture,
      seed = s.sessions[0];
    s.sessions = Array.from({ length: 100000 }, (_, index) => ({
      ...seed,
      id: "performance-" + index,
      requestNumber: String(index + 1),
      startedAt: index + 1,
      path: "/traffic/" + index,
      url: "https://example.test/traffic/" + index,
    }));
    s.dismissedIds = [];
    t.sort = { column: "started-at", direction: "ascending" };
    t.followLatest = true;
    t.clearTrafficSelection();
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return (
        !t.queryPending &&
        t.sessions.length &&
        t.sessions.every((row) => !row.loading) &&
        !t.rowRequests.size
      );
    });
  await settle();
  const supported = await page.evaluate(
    () => !!document.querySelector("app-shell").traffic.classifications,
  );
  const report = { entries: 100000, supported, cases: [] };
  const cases = supported ? ["off", "installed-off", "one", "eight"] : ["off"];
  for (const mode of cases) {
    await page.evaluate(async (mode) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.extensionColumnChoices =
        mode === "off"
          ? []
          : Array.from({ length: 2048 }, (_, index) => ({
              key: "perf." + Math.floor(index / 16) + ":column-" + index,
              name: "Protocol " + index,
              extensionName: "Extension " + Math.floor(index / 16),
            }));
      t.preferences = {
        ...t.preferences,
        extensionColumns: s.extensionColumnChoices
          .slice(-(mode === "eight" ? 8 : mode === "one" ? 1 : 0))
          .filter(() => mode === "one" || mode === "eight")
          .map((column) => column.key),
      };
      if (t.classifications) await t.classifications.refresh();
      t.renderViewport();
    }, mode);
    await settle();
    if (mode === "one" || mode === "eight")
      await page.waitForFunction(() => {
        const t = document.querySelector("app-shell").traffic;
        return (
          !t.classifications.loading &&
          t.sessions.every((row) => row.extensionCells.every((cell) => cell.text !== "…"))
        );
      });
    const result = await page.evaluate(async (mode) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      const stats = (values) => {
        values.sort((a, b) => a - b);
        return {
          p50: values[Math.floor(values.length * 0.5)],
          p95: values[Math.floor(values.length * 0.95)],
          max: values.at(-1),
        };
      };
      const callsBefore = s.calls.classify_traffic_rows ?? 0,
        render = [];
      for (let sample = 0; sample < 35; sample++) {
        const start = performance.now();
        for (let i = 0; i < 30; i++) t.renderViewport();
        if (sample >= 5) render.push((performance.now() - start) / 30);
        await new Promise((resolve) => setTimeout(resolve, 0));
      }
      const refresh = [];
      for (let sample = 0; sample < 10; sample++) {
        const start = performance.now();
        await t.refreshSessions(undefined, true);
        if (sample >= 2) refresh.push(performance.now() - start);
      }
      return {
        mode,
        renderMs: stats(render),
        refreshMs: stats(refresh),
        domRows: t.sessions.length,
        settingsInputs: (t.shadowRoot ?? t).querySelectorAll(".extension-column-options input")
          .length,
        classificationCalls: (s.calls.classify_traffic_rows ?? 0) - callsBefore,
        cache: t.classifications?.cache.size ?? 0,
      };
    }, mode);
    assert.ok(result.domRows < 150);
    assert.equal(
      result.settingsInputs,
      0,
      "Installed declarations mounted hidden controls in the main list",
    );
    if (mode === "off" || mode === "installed-off")
      assert.equal(result.classificationCalls, 0, "Unselected columns performed traffic work");
    const queries = await page.evaluate(
      () => globalThis.__workspaceFixture.calls.query_traffic_view,
    );
    const scroll = [];
    for (let sample = 0; sample < 15; sample++) {
      const fraction = [0.25, 0.75, 0.5, 0, 1][sample % 5];
      const began = await page.evaluate((fraction) => {
        const t = document.querySelector("app-shell").traffic;
        t.sessionScroller.scrollTop =
          fraction * (t.sessionScroller.scrollHeight - t.sessionScroller.clientHeight);
        t.sessionScrolled();
        return performance.now();
      }, fraction);
      await page.waitForFunction((fraction) => {
        const t = document.querySelector("app-shell").traffic;
        return (
          !t.rowRequests.size &&
          t.sessions.length &&
          t.sessions.every((row) => !row.loading) &&
          (fraction === 0
            ? t.sessions[0]?.index === 0
            : fraction === 1
              ? t.sessions.at(-1)?.index === 99999
              : t.sessions.some((row) => Math.abs(row.index - 100000 * fraction) < 100))
        );
      }, fraction);
      scroll.push(await page.evaluate((began) => performance.now() - began, began));
    }
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.calls.query_traffic_view),
      queries,
      "Scrolling rebuilt the full traffic query",
    );
    if (mode === "one" || mode === "eight")
      await page.waitForFunction(() => {
        const t = document.querySelector("app-shell").traffic;
        return (
          !t.classifications.loading &&
          t.sessions.every((row) => row.extensionCells.every((cell) => cell.text !== "…"))
        );
      });
    result.classifiedRows = await page.evaluate(
      () =>
        globalThis.__workspaceFixture.classificationRequests?.reduce(
          (sum, request) => sum + request.ids.length,
          0,
        ) ?? 0,
    );
    result.cachedLabels = await page.evaluate(
      () => document.querySelector("app-shell").traffic.classifications?.cache.size ?? 0,
    );
    assert.ok(result.cachedLabels <= 1024, "Label results retained an unbounded traffic history");
    if (mode === "off" || mode === "installed-off")
      assert.equal(
        result.classifiedRows,
        0,
        "Unselected declarations classified rows while scrolling",
      );
    scroll.sort((a, b) => a - b);
    result.scrollMs = { p50: scroll[7], p95: scroll[14] };
    report.cases.push(result);
  }
  if (supported) {
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.deferClassifications = true;
      t.classifications.invalidate();
      t.renderViewport();
    });
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishClassifications);
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      await t.refreshSessions(undefined, true);
    });
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.virtualList.ids.length),
      100000,
      "Pending labels blocked the main list",
    );
    await page.evaluate(() => globalThis.__workspaceFixture.finishClassifications());
  }
  if (output) await writeFile(output, JSON.stringify(report, null, 2) + "\n");
  process.stdout.write(JSON.stringify(report) + "\n");
}
