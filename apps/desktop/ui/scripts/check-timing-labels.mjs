import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTimingLabels(page, root) {
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  const saved = await page.evaluate(() => globalThis.__workspaceFixture.performance);
  try {
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.performance = {
        points: Object.entries({
          "request-headers": 0,
          "client-request-done": 2000,
          "route-begin": 0,
          "route-done": 1000,
          "upstream-begin": 1000,
          "upstream-connected": 10000,
          "upstream-request-consumed": 12000,
          "response-headers": 80000,
          "client-response-begin": 81000,
          "upstream-response-done": 100000,
          "client-response-queued": 101000,
          "exchange-done": 102000,
        }).map(([milestone, offsetMicros]) => ({
          milestone,
          offsetMicros,
          unixMillis: 1800000000000 + offsetMicros / 1000,
        })),
        transports: [],
        work: [],
      };
      await t.inspectTraffic(s.sessions[0].id, false);
      await t.showTimings();
    });
    const dialog = page.locator(".timing-dialog[open]");
    const expected = [
      "Request duration",
      "Client upload",
      "Routing",
      "Connection wait",
      "Forwarding request body",
      "Waiting for response",
      "Receiving response",
      "Queueing to client",
    ];
    assert.deepEqual(
      await dialog.locator(".waterfall-row > span:first-child").allTextContents(),
      expected,
    );
    await dialog.getByText("Latency phase details", { exact: true }).click();
    assert.deepEqual(await dialog.locator(".timing-facts dt").allTextContents(), expected);
    await dialog.getByRole("button", { name: "Copy report", exact: true }).click();
    await dialog.getByText("Timing report copied.", { exact: true }).waitFor();
    const report = await page.evaluate(() => navigator.clipboard.readText());
    for (const label of expected) assert.ok(report.includes(label + ": "));
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await dialog.getByText("Request waterfall", { exact: true }).scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/timing-labels-" + width + ".png"),
      });
    }
    await page.keyboard.press("Escape");
  } finally {
    await page.evaluate((saved) => {
      globalThis.__workspaceFixture.performance = saved;
    }, saved);
    await page.setViewportSize({ width: 1280, height: 800 });
  }
}
