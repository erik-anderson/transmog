import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficRefresh(page, root) {
  const viewport = page.viewportSize();
  await page.goto("https://workspace.test/");
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic,
        row = t.trafficTable.querySelector("tr[data-session-id]");
      return (
        t.queryLoaded &&
        !t.queryPending &&
        !t.rowRequests.size &&
        t.renderFrame === undefined &&
        row &&
        Math.abs(
          t.virtualList.size(t.virtualList.indexes.get(row.dataset.sessionId)) -
            row.getBoundingClientRect().height,
        ) < 0.01 &&
        t.sessions.length &&
        t.sessions.every((row) => !row.loading)
      );
    });
  const snapshot = () =>
    page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic;
      return {
        rows: [...t.trafficTable.querySelectorAll("tr[data-session-id]")].map((row) => ({
          id: row.dataset.sessionId,
          text: row.textContent.replace(/\s+/g, " ").trim(),
          loading: row.getAttribute("aria-busy"),
        })),
        top: t.sessionScroller.scrollTop,
        selected: t.selectedSessionId,
        focus: t.getRootNode().activeElement?.id,
      };
    });
  const hold = () =>
    page.evaluate(() => {
      const raw = window.__TAURI_INTERNALS__.invoke,
        delay = { raw, held: [] };
      globalThis.__refreshDelay = delay;
      window.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        const result = await raw(name, args, options);
        if (name === "traffic_view_rows") {
          await new Promise((resolve) => delay.held.push(resolve));
        }
        return result;
      };
    });
  const held = () => page.waitForFunction(() => globalThis.__refreshDelay.held.length > 0);
  const release = () =>
    page.evaluate(() => {
      const delay = globalThis.__refreshDelay;
      if (!delay) return;
      window.__TAURI_INTERNALS__.invoke = delay.raw;
      for (const resolve of delay.held) resolve();
      delete globalThis.__refreshDelay;
    });
  try {
    await settle();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      s.sessions = Array.from({ length: 600 }, (_, index) => ({
        ...seed,
        id: "refresh-" + index,
        requestNumber: String(index + 1),
        caller: { kind: "local-process", processName: "chrome.exe", processId: 28416 },
        method: "GET",
        status: 200,
        terminal: "completed",
        durationMs: 11,
        responseBytes: 319,
        path: "/refresh/" + index,
        url: "https://example.test/refresh/" + index,
      }));
      await t.refreshSessions(undefined, true);
    });
    await settle();

    // Both following and paused views beyond the query's first page must stay populated.
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      for (const following of [true, false]) {
        if (following) {
          await page.evaluate(
            async () => await document.querySelector("app-shell").traffic.resumeLatest(),
          );
        } else {
          await page.locator("#session-refresh-599").click();
          await page.waitForFunction(
            () => !!document.querySelector("app-shell").traffic.selectedDetail,
          );
        }
        await settle();
        const before = await snapshot();
        assert.ok(before.rows.every((row) => Number(row.id.slice(8)) >= 100));
        await page.evaluate(() => {
          const t = document.querySelector("app-shell").traffic;
          globalThis.__refreshRows = [...t.trafficTable.querySelectorAll("tr[data-session-id]")];
          globalThis.__refreshFlashes = [];
          globalThis.__refreshObserver = new MutationObserver(() => {
            for (const row of globalThis.__refreshRows) {
              if (
                !row.isConnected ||
                row.getAttribute("aria-busy") !== "false" ||
                /Pending|Unknown process/.test(row.textContent)
              ) {
                globalThis.__refreshFlashes.push(row.dataset.sessionId);
              }
            }
          });
          globalThis.__refreshObserver.observe(t.trafficTable, {
            subtree: true,
            childList: true,
            characterData: true,
            attributes: true,
          });
        });
        await hold();
        await page.evaluate(
          async () => await document.querySelector("app-shell").traffic.refreshSessions(),
        );
        await held();
        assert.deepEqual(
          await snapshot(),
          before,
          "A refresh discarded displayed traffic while reading its page",
        );
        await release();
        await settle();
        assert.deepEqual(
          await snapshot(),
          before,
          "An unchanged refresh moved or changed the traffic view",
        );
        assert.deepEqual(
          await page.evaluate(() => {
            globalThis.__refreshObserver.disconnect();
            return globalThis.__refreshFlashes;
          }),
          [],
          "A retained row briefly rendered loading placeholders",
        );
      }
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/traffic-refresh-${width}.png`),
      });
    }

    // Cached presentation must not prevent real updates or allow older reads to win.
    const id = (await snapshot()).rows.at(-1).id;
    await hold();
    await page.evaluate(async (id) => {
      const s = globalThis.__workspaceFixture;
      s.sessions = s.sessions.map((row) => (row.id === id ? { ...row, status: 201 } : row));
      await document.querySelector("app-shell").traffic.refreshSessions();
    }, id);
    await held();
    await page.evaluate(async (id) => {
      const delay = globalThis.__refreshDelay,
        s = globalThis.__workspaceFixture;
      window.__TAURI_INTERNALS__.invoke = delay.raw;
      s.sessions = s.sessions.map((row) =>
        row.id === id
          ? {
              ...row,
              status: 204,
              caller: { ...row.caller, processName: "msedge.exe" },
            }
          : row,
      );
      await document.querySelector("app-shell").traffic.refreshSessions();
    }, id);
    const row = page.locator(`#session-${id}`);
    await row.locator('[data-column-id="status"]').getByText("204", { exact: true }).waitFor();
    await release();
    await settle();
    assert.equal((await row.locator('[data-column-id="status"]').textContent()).trim(), "204");
    assert.match(await row.locator('[data-column-id="process"]').textContent(), /msedge\.exe/);

    // A failed refresh keeps known evidence and can be retried without placeholders.
    const beforeFailure = await snapshot();
    await page.evaluate(async () => {
      const raw = window.__TAURI_INTERNALS__.invoke;
      globalThis.__refreshDelay = { raw, held: [] };
      window.__TAURI_INTERNALS__.invoke = (name, args, options) =>
        name === "traffic_view_rows"
          ? Promise.reject(new Error("Fixture row refresh failure"))
          : raw(name, args, options);
      await document.querySelector("app-shell").traffic.refreshSessions();
    });
    await page.waitForFunction(() =>
      document
        .querySelector("app-shell")
        .traffic.queryError.includes("Fixture row refresh failure"),
    );
    assert.deepEqual(
      await snapshot(),
      beforeFailure,
      "A row refresh failure erased known evidence",
    );
    await release();
    await page.evaluate(
      async () => await document.querySelector("app-shell").traffic.refreshSessions(),
    );
    await settle();
    assert.deepEqual(await snapshot(), beforeFailure);
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.queryError),
      "",
    );

    // Removed identities must leave both presentation and its bounded cache.
    await page.evaluate(async (id) => {
      globalThis.__workspaceFixture.dismissedIds = [id];
      await document.querySelector("app-shell").traffic.refreshSessions();
    }, id);
    await settle();
    assert.equal(await row.count(), 0);
    assert.equal(
      await page.evaluate((id) => document.querySelector("app-shell").traffic.rowCache.has(id), id),
      false,
    );

    // Returning to an older cached page must remain stable when the first page
    // is read again and the metadata cache is already full.
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      for (let index = 600; index < 1800; index++) {
        s.sessions.push({ ...seed, id: "refresh-" + index, requestNumber: String(index + 1) });
      }
      await t.refreshSessions();
      for (let offset = 700; offset < 1800; offset += 100) await t.loadWindow(offset);
      await t.revealCurrent("refresh-800", true);
    });
    await settle();
    const beforeEviction = await snapshot();
    await hold();
    await page.evaluate(
      async () => await document.querySelector("app-shell").traffic.refreshSessions(),
    );
    await held();
    assert.deepEqual(
      await snapshot(),
      beforeEviction,
      "Refreshing a full cache evicted displayed summaries",
    );
    await release();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.rowCache.size),
      1024,
    );

    // A retained ID can disappear between the membership query and its row read.
    // Navigation must wait for the corrective query rather than return stale evidence.
    await page.evaluate(() => {
      const raw = window.__TAURI_INTERNALS__.invoke,
        delay = { raw, held: [], queries: 0 };
      globalThis.__refreshDelay = delay;
      globalThis.__refreshRevealSettled = false;
      window.__TAURI_INTERNALS__.invoke = async (name, args, options) => {
        if (name === "query_traffic_view" && delay.queries++ > 0) {
          await new Promise((resolve) => delay.held.push(resolve));
        }
        const result = await raw(name, args, options);
        if (name === "query_traffic_view" && delay.queries === 1) {
          globalThis.__workspaceFixture.dismissedIds.push("refresh-800");
        }
        return result;
      };
      globalThis.__missingRefresh = document.querySelector("app-shell").traffic.refreshSessions();
    });
    await held();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      globalThis.__missingReveal = t.revealCurrent("refresh-800", true).then((row) => {
        globalThis.__refreshRevealSettled = true;
        return row?.id ?? null;
      });
      await t.$whenRendered();
      await t.loadWindow(Math.floor(t.virtualList.indexes.get("refresh-800") / 100) * 100);
      await t.$whenRendered();
      await new Promise(requestAnimationFrame);
    });
    assert.equal(
      await page.evaluate(() => globalThis.__refreshRevealSettled),
      false,
      "Navigation returned a cached summary absent from the authoritative row read",
    );
    await release();
    assert.equal(await page.evaluate(async () => await globalThis.__missingReveal), null);
    await page.evaluate(async () => await globalThis.__missingRefresh);
    await settle();

    // A newer navigation can complete during the older navigation's final render.
    const newerFocus = await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic,
        scroller = t.sessionScroller.getBoundingClientRect(),
        header = t.trafficTable.tHead.getBoundingClientRect(),
        [older, newer] = [...t.trafficTable.querySelectorAll("tr[data-session-id]")].filter(
          (row) => {
            const bounds = row.getBoundingClientRect();
            return (
              bounds.top >= Math.max(scroller.top, header.bottom) &&
              bounds.bottom <= scroller.bottom
            );
          },
        ),
        raw = t.renderViewport,
        delay = { raw, held: [], calls: 0 };
      globalThis.__refreshRenderDelay = delay;
      t.renderViewport = async (...args) => {
        const call = ++delay.calls;
        await raw.apply(t, args);
        if (call === 2) await new Promise((resolve) => delay.held.push(resolve));
      };
      globalThis.__olderReveal = t.revealCurrent(older.dataset.sessionId, true);
      return newer.dataset.sessionId;
    });
    await page.waitForFunction(() => globalThis.__refreshRenderDelay.held.length > 0);
    await page.evaluate(
      async (id) => await document.querySelector("app-shell").traffic.revealCurrent(id, true),
      newerFocus,
    );
    await page.evaluate(() => {
      for (const resolve of globalThis.__refreshRenderDelay.held) resolve();
    });
    assert.equal(
      await page.evaluate(async () => (await globalThis.__olderReveal)?.id ?? null),
      null,
      "An older navigation completed after a newer navigation owned the final render",
    );
    assert.equal(
      await page.evaluate(
        () => document.querySelector("app-shell").traffic.getRootNode().activeElement.id,
      ),
      "session-" + newerFocus,
      "A delayed render stole focus from the newer request",
    );
  } finally {
    await release();
    await page.evaluate(() => {
      globalThis.__refreshObserver?.disconnect();
      const delay = globalThis.__refreshRenderDelay;
      if (delay) {
        document.querySelector("app-shell").traffic.renderViewport = delay.raw;
        for (const resolve of delay.held) resolve();
        delete globalThis.__refreshRenderDelay;
      }
    });
    if (viewport) await page.setViewportSize(viewport);
    await page.goto("https://workspace.test/");
    await settle();
  }
}
