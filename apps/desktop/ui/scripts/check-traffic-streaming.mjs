import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficStreaming(page, root) {
  await page.goto("https://workspace.test/");
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return (
        t.queryLoaded &&
        !t.queryPending &&
        !t.rowRequests.size &&
        !t.liveRefreshRunning &&
        !t.liveRefreshDirty &&
        t.refreshTimer === undefined &&
        t.sessions.every((row) => !row.loading)
      );
    });
  const release = () =>
    page.evaluate(() => {
      const p = globalThis.__streamProbe;
      if (!p) return;
      p.blockRows = false;
      for (const resolve of p.held.splice(0)) resolve();
    });
  try {
    await settle();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = {
          ...s.sessions[0],
          method: "GET",
          status: 201,
          terminal: "completed",
          caller: { kind: "local-process", processName: "stream.exe", processId: 1234 },
        };
      s.dismissedIds = [];
      s.sessions = Array.from({ length: 250 }, (_, index) => ({
        ...seed,
        id: "stream-" + index,
        requestNumber: String(index + 1),
        path: "/stream/" + index,
        url: "https://example.test/stream/" + index,
      }));
      await t.resumeLatest();
      const raw = window.__TAURI_INTERNALS__.invoke,
        p = (globalThis.__streamProbe = {
          raw,
          held: [],
          blockRows: true,
          dropPages: true,
          loading: [],
          seen: new Set(),
          queries: [],
          active: 0,
          maximum: 0,
        });
      p.observer = new MutationObserver(() => {
        for (const row of t.trafficTable.querySelectorAll('tr[data-session-id^="arrival-"]')) {
          p.seen.add(row.dataset.sessionId);
          if (
            row.getAttribute("aria-busy") !== "false" ||
            /Unknown process|Pending|…/.test(row.textContent)
          )
            p.loading.push(row.dataset.sessionId);
        }
      });
      p.observer.observe(t.trafficTable, {
        subtree: true,
        childList: true,
        characterData: true,
        attributes: true,
      });
      window.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        if (name === "traffic_view_rows" && p.blockRows)
          await new Promise((resolve) => p.held.push(resolve));
        if (name === "query_traffic_view") {
          p.queries.push(performance.now());
          p.maximum = Math.max(p.maximum, ++p.active);
        }
        try {
          const result = await raw(name, args, options);
          if (name === "query_traffic_view" && p.dropPages) result.page.sessions = [];
          return result;
        } finally {
          if (name === "query_traffic_view") p.active--;
        }
      };
      p.arrive = (id, extra = {}) => {
        s.sessions.push({
          ...seed,
          id,
          requestNumber: String(s.sessions.length + 1),
          path: "/" + id,
          url: "https://example.test/" + id,
          ...extra,
        });
        s.channel.onmessage({ exchangeId: id, sequence: s.sessions.length, lagged: false });
      };
      for (let i = 0; i < 4; i++) p.arrive("arrival-staged-" + i);
    });
    await page.waitForFunction(() => globalThis.__streamProbe.held.length > 0);
    assert.deepEqual(
      await page.evaluate(() => {
        const t = document.querySelector("app-shell").traffic;
        return {
          count: t.totalMatched,
          newRows: t.trafficTable.querySelectorAll('tr[data-session-id^="arrival-"]').length,
        };
      }),
      { count: 250, newRows: 0 },
      "New membership appeared before its row summaries were ready",
    );
    await release();
    await settle();
    await page
      .locator("#session-arrival-staged-3")
      .getByText("stream.exe (1234)", { exact: true })
      .waitFor();
    assert.deepEqual(
      await page.evaluate(() => globalThis.__streamProbe.loading),
      [],
      "A new row flashed loading/default column values",
    );

    // Cold historical windows reserve blank slots until their summaries arrive.
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      globalThis.__streamProbe.blockRows = true;
      t.followLatest = false;
      t.sessionScroller.scrollTop = 0;
      await t.renderViewport();
    });
    await page.waitForFunction(() => globalThis.__streamProbe.held.length > 0);
    assert.equal((await page.locator('tr[data-loading="true"]').count()) > 0, true);
    assert.equal(
      await page
        .locator('tr[data-loading="true"]')
        .evaluateAll((rows) =>
          rows.every((row) => [...row.cells].every((cell) => !cell.textContent.trim())),
        ),
      true,
      "A loading slot fabricated a Pending status or Unknown process",
    );
    const geometry = await page
      .locator('tr[data-loading="true"]')
      .first()
      .evaluate((row) => {
        const t = document.querySelector("app-shell").traffic;
        return {
          height: row.getBoundingClientRect().height,
          expected: t.virtualList.size(t.virtualList.indexes.get(row.dataset.sessionId)),
          lineHeight: getComputedStyle(row.cells[0]).lineHeight,
          padding: getComputedStyle(row.cells[0]).padding,
        };
      });
    assert.ok(
      Math.abs(geometry.height - geometry.expected) < 0.01,
      "Loading a blank slot changed the row geometry: " + JSON.stringify(geometry),
    );
    await release();
    await settle();
    await page.evaluate(
      async () => await document.querySelector("app-shell").traffic.resumeLatest(),
    );
    await settle();

    // A disappearing admission invalidates its staged batch and retries quietly.
    await page.evaluate(() => {
      const p = globalThis.__streamProbe;
      p.blockRows = true;
      p.arrive("arrival-removed");
      p.arrive("arrival-retained");
    });
    await page.waitForFunction(() => globalThis.__streamProbe.held.length > 0);
    await page.evaluate(() => globalThis.__workspaceFixture.dismissedIds.push("arrival-removed"));
    await release();
    await settle();
    assert.equal(
      await page.evaluate(() => globalThis.__streamProbe.seen.has("arrival-removed")),
      false,
    );
    assert.equal(await page.locator("#session-arrival-retained").count(), 1);

    // Sustained hints must make progress, retain the final arrival and stay bounded,
    // including when one query takes longer than the 250 ms refresh budget.
    for (const delay of [40, 400]) {
      const count = await page.evaluate(async (delay) => {
        const p = globalThis.__streamProbe,
          s = globalThis.__workspaceFixture;
        p.dropPages = false;
        p.queries = [];
        p.maximum = 0;
        s.queryDelay = delay;
        const until = performance.now() + 1050;
        let count = 0;
        while (performance.now() < until) {
          p.arrive(`arrival-${delay}-${count++}`);
          await new Promise((resolve) => setTimeout(resolve, 20));
        }
        return count;
      }, delay);
      await settle();
      const stats = await page.evaluate(() => {
        const p = globalThis.__streamProbe;
        return { times: p.queries, maximum: p.maximum, loading: p.loading };
      });
      assert.ok(
        stats.times.length >= (delay === 40 ? 4 : 2),
        "Live arrivals were held for a quiet period",
      );
      assert.equal(stats.maximum, 1, "Live traffic queries overlapped");
      assert.ok(
        stats.times.slice(1).every((time, index) => time - stats.times[index] >= 245),
        "Live traffic queries exceeded four per second",
      );
      assert.deepEqual(stats.loading, []);
      assert.equal(
        await page.locator(`#session-arrival-${delay}-${count - 1}`).count(),
        1,
        "The trailing batch was lost after traffic became quiet",
      );
    }

    // A real in-flight request stays inspectable once its core metadata is known.
    await page.evaluate(() => {
      globalThis.__workspaceFixture.queryDelay = 0;
      globalThis.__streamProbe.arrive("active-stream", { status: null, terminal: "active" });
    });
    await settle();
    const active = page.locator("#session-active-stream");
    await active.getByText("Pending", { exact: true }).waitFor();
    assert.equal(await active.getAttribute("aria-busy"), "false");
    await active.getByText("stream.exe (1234)", { exact: true }).waitFor();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-streaming.png"),
    });
  } finally {
    await release();
    await page.evaluate(() => {
      const p = globalThis.__streamProbe;
      if (p) {
        window.__TAURI_INTERNALS__.invoke = p.raw;
        p.observer.disconnect();
        delete globalThis.__streamProbe;
      }
    });
    await page.goto("https://workspace.test/");
    await settle();
  }
}
