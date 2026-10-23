import assert from "node:assert/strict";
import { resolve } from "node:path";
import { openActionGroup } from "./check-action-menus.mjs";

export async function checkTrafficAdvanced(page, root) {
  const traffic = page.locator("traffic-workspace"),
    picker = traffic.locator("#traffic-filter-picker");
  const saved = await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    return {
      rows: s.sessions,
      preferences: t.preferences,
      filters: t.filters,
      groups: t.filterGroups,
      follow: t.followLatest,
      lifecycle: t.lifecycle,
    };
  });
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return !t.queryPending && !t.rowRequests.size && t.sessions.every((row) => !row.loading);
    });
  const open = async (dimension) => {
    await page.evaluate(
      (dimension) => document.querySelector("app-shell").traffic.openFilterPicker(dimension),
      dimension,
    );
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
  };
  const apply = async () => {
    await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
    await settle();
  };
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      s.sessions = Array.from({ length: 240 }, (_, i) => ({
        ...seed,
        id: "advanced-" + i,
        requestNumber: String(i + 1),
        host: "host-" + String(i).padStart(3, "0") + ".test",
        caller: { kind: "local-process", processName: "tool-" + i + ".exe", processId: i },
        contentType: i % 2 ? "application/json" : "text/plain",
        streamProtocol: null,
        streamLifetime: null,
        terminal: "completed",
        durationMs: i,
        requestBytes: i,
        responseBytes: i * 1024,
        startedAt: Date.UTC(2026, 9, 17, 20, 0, 0, i),
        streamSent: null,
        streamReceived: null,
      }));
      s.sessions[0].caller = { kind: "remote", processName: null, processId: null };
      s.sessions[0].contentType = null;
      s.sessions[0].status = null;
      t.filters = [];
      t.filterGroups = [];
      t.fetchDestinations = [];
      t.liveFilter = "";
      t.searchText = "";
      t.searchInput.value = "";
      t.lifecycle = "stopped";
      t.followLatest = true;
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await settle();
    await checkAdvancedReviewFixes(page, traffic, picker, open, apply, settle);
    await open("host");
    assert.equal(await picker.locator(".filter-options input").count(), 50);
    await picker.getByRole("button", { name: "Load more", exact: true }).click();
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.pickerObservedShown === 100,
    );
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.facetRequests.at(-1).offset),
      50,
    );
    await page.evaluate(() => (globalThis.__workspaceFixture.deferFacets = true));
    await picker.getByRole("button", { name: "Load more", exact: true }).click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await page.evaluate(() => {
      document.querySelector("app-shell").traffic.trafficRevision++;
      globalThis.__workspaceFixture.releaseFacets();
    });
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.facetRequests.at(-1).offset),
      0,
      "A capture change appended an obsolete observation page",
    );
    for (const n of [150, 200]) {
      await picker.getByRole("button", { name: "Load more", exact: true }).click();
      await page.waitForFunction(
        (n) => document.querySelector("app-shell").traffic.pickerObservedShown === n,
        n,
      );
    }
    assert.equal(
      await picker.getByRole("button", { name: "Load more", exact: true }).isVisible(),
      false,
    );
    assert.match(await picker.textContent(), /Search to narrow/);
    await picker.getByLabel("Search observed values").fill("host-239");
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    assert.equal(
      await picker.getByRole("checkbox", { name: "host-239.test", exact: true }).count(),
      1,
    );
    assert.equal(await picker.locator(".filter-options input").count(), 1);
    await page.evaluate(() => (globalThis.__workspaceFixture.deferFacets = true));
    await picker.getByLabel("Search observed values").fill("host-00");
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await picker.getByLabel("Search observed values").fill("host-120");
    await page.evaluate(() => globalThis.__workspaceFixture.releaseFacets());
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    assert.equal(
      await picker.getByRole("checkbox", { name: "host-120.test", exact: true }).count(),
      1,
    );
    assert.equal(
      await picker.getByRole("checkbox", { name: "host-000.test", exact: true }).count(),
      0,
    );
    await picker.getByLabel("Search observed values").fill("future.test");
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    await picker
      .getByRole("button", { name: "Use exact value “future.test”", exact: true })
      .click();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      0,
    );
    await open("host");
    assert.equal(
      await picker.getByRole("checkbox", { name: "future.test", exact: true }).isChecked(),
      true,
    );
    assert.match(
      await picker
        .getByRole("checkbox", { name: "future.test", exact: true })
        .locator("..")
        .textContent(),
      /0/,
    );
    await page.keyboard.press("Escape");
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.sessions.push({
        ...s.sessions[1],
        id: "future-arrival",
        requestNumber: "241",
        host: "future.test",
        startedAt: Date.UTC(2026, 9, 17, 20, 0, 0, 999),
      });
      await t.refreshSessions();
    });
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("process");
    await picker.getByRole("checkbox", { name: "No process name", exact: true }).check();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("content-type");
    await picker.getByRole("checkbox", { name: "Missing content type", exact: true }).check();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("status");
    await picker.getByRole("checkbox", { name: "Unavailable status", exact: true }).check();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("response-bytes");
    await picker.getByLabel("Range units", { exact: true }).selectOption("KiB");
    await picker.getByLabel("Range from inclusive").fill("1.5");
    await picker.getByLabel("Range to inclusive").fill("2");
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await open("response-bytes");
    await picker.getByLabel("Range from inclusive").fill("5");
    await picker.getByLabel("Range to inclusive").fill("1");
    await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
    assert.equal(
      await picker.getByLabel("Range from inclusive").evaluate((el) => el.validity.customError),
      true,
    );
    await picker.getByLabel("Range from inclusive").fill("1.1");
    await picker.getByLabel("Range units", { exact: true }).selectOption("B");
    await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
    assert.equal(
      await picker.getByLabel("Range from inclusive").evaluate((el) => el.validity.customError),
      true,
    );
    await page.keyboard.press("Escape");
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("request-number");
    await picker.getByLabel("Range from inclusive").fill("9007199254740993");
    await picker.getByLabel("Range to inclusive").fill("18446744073709551615");
    await apply();
    assert.equal(
      await page.evaluate(
        () =>
          document.querySelector("app-shell").traffic.filters.find((f) => f.operator === "minimum")
            .value,
      ),
      "9007199254740993",
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await open("started-at");
    await picker.getByLabel("Range from inclusive").fill("2026-10-17T13:00:00.001");
    await picker.getByLabel("Range to inclusive").fill("2026-10-17T13:00:00.002");
    await apply();
    assert.equal(
      await page.evaluate(
        () =>
          document.querySelector("app-shell").traffic.filters.find((f) => f.operator === "maximum")
            .value,
      ),
      String(Date.UTC(2026, 9, 17, 20, 0, 0, 2)),
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      2,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await page.locator("#session-advanced-239").click({ button: "right" });
    await openActionGroup(traffic.locator("#traffic-entry-menu"), "Filter by");
    await traffic
      .locator("#traffic-entry-menu")
      .getByRole("button", { name: "Host: host-239.test…", exact: true })
      .click();
    assert.equal(
      await picker.getByRole("checkbox", { name: "host-239.test", exact: true }).isChecked(),
      true,
    );
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      1,
    );
    await traffic.getByRole("button", { name: "Clear filters", exact: true }).click();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.sessions = [
        {
          ...s.sessions[0],
          id: "narrow-ws",
          streamProtocol: "web-socket",
          streamSent: 0,
          streamReceived: 4096,
        },
      ];
      t.preferences = {
        ...t.preferences,
        columns: t.preferences.columns.map((c) =>
          c.id === "protocol"
            ? { ...c, width: 72, visible: true }
            : c.id === "stream-sent" || c.id === "stream-received"
              ? { ...c, visible: true }
              : c,
        ),
      };
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await settle();
    const identity = traffic.locator(".stream-identity");
    assert.equal(await identity.textContent(), "WS⌄");
    assert.match(await identity.getAttribute("aria-label"), /WebSocket/);
    assert.equal(
      await identity.evaluate((el) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        const cell = el.closest("td").getBoundingClientRect();
        return [...range.getClientRects()].every(
          (r) => r.right <= cell.right && r.left >= cell.left,
        );
      }),
      true,
    );
    await identity.click({ modifiers: ["Shift"] });
    await traffic
      .locator("#stream-disclosure")
      .getByRole("button", { name: "Close", exact: true })
      .click();
    assert.equal(await identity.evaluate((el) => el === el.getRootNode().activeElement), true);
    assert.match(
      await traffic.locator('[data-session-id] [data-column-id="stream-sent"]').textContent(),
      /0 B/,
    );
    assert.match(
      await traffic.locator('[data-session-id] [data-column-id="stream-received"]').textContent(),
      /4.0 KB/,
    );
    for (const viewport of [
      { width: 1440, height: 900 },
      { width: 960, height: 720 },
      { width: 800, height: 600 },
    ]) {
      await page.setViewportSize(viewport);
      await open("host");
      const box = await picker.boundingBox();
      assert.ok(
        box.x >= 0 &&
          box.y >= 0 &&
          box.x + box.width <= viewport.width + 1 &&
          box.y + box.height <= viewport.height + 1,
      );
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/traffic-advanced-" + viewport.width + ".png"),
      });
      await page.keyboard.press("Escape");
    }
    await identity.scrollIntoViewIfNeeded();
    await identity.click();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-advanced-narrow-ws.png"),
    });
    await traffic
      .locator("#stream-disclosure")
      .getByRole("button", { name: "Close", exact: true })
      .click();
    await page.evaluate(() => {
      document.documentElement.style.zoom = "1.5";
    });
    await open("response-bytes");
    await picker.getByLabel("Range preset").selectOption("mib");
    await picker
      .getByRole("button", { name: "Apply filter", exact: true })
      .scrollIntoViewIfNeeded();
    const zoom = await picker.boundingBox();
    assert.ok(
      zoom.x >= 0 && zoom.y >= 0 && zoom.x + zoom.width <= 801 && zoom.y + zoom.height <= 601,
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-advanced-range-zoom.png"),
    });
    await page.keyboard.press("Escape");
    await page.evaluate(() => {
      document.documentElement.style.zoom = "";
    });
  } finally {
    await page.evaluate(() => {
      document.documentElement.style.zoom = "";
    });
    await page.evaluate(async (saved) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.sessions = saved.rows;
      t.preferences = saved.preferences;
      t.filters = saved.filters;
      t.filterGroups = saved.groups;
      t.followLatest = saved.follow;
      t.lifecycle = saved.lifecycle;
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    }, saved);
    await page.setViewportSize({ width: 1280, height: 800 });
  }
}

async function checkAdvancedReviewFixes(page, traffic, picker, open, apply, settle) {
  const set = async (column, bounds) => {
    await page.evaluate(
      async ({ column, bounds }) => {
        const t = document.querySelector("app-shell").traffic;
        t.filters = bounds.map(([operator, value]) => ({ column, operator, value, label: "" }));
        t.filterGroups = [];
        t.queryRevision++;
        await t.refreshSessions(undefined, true);
      },
      { column, bounds },
    );
    await settle();
  };
  await set("duration", [
    ["minimum", "1000"],
    ["maximum", "2000"],
  ]);
  await open("duration");
  await picker.getByRole("button", { name: "Remove minimum condition", exact: true }).click();
  assert.equal(await picker.getByLabel("Range from inclusive").inputValue(), "");
  await apply();
  assert.deepEqual(
    await page.evaluate(() =>
      document.querySelector("app-shell").traffic.filters.map((f) => [f.operator, f.value]),
    ),
    [["maximum", "2000"]],
  );
  await open("duration");
  await picker.getByRole("button", { name: "Remove maximum condition", exact: true }).click();
  assert.equal(await picker.getByLabel("Range to inclusive").inputValue(), "");
  await apply();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.filters.length),
    0,
  );
  const later = String(Date.UTC(2026, 10, 1, 9, 30)),
    upper = String(Date.UTC(2026, 10, 1, 9, 40));
  await set("started-at", [
    ["minimum", later],
    ["maximum", upper],
  ]);
  await open("started-at");
  await apply();
  assert.deepEqual(
    await page.evaluate(() =>
      document.querySelector("app-shell").traffic.filters.map((f) => f.value),
    ),
    [later, upper],
  );
  await open("started-at");
  await picker.getByLabel("Range from inclusive").fill("2026-11-01T01:31");
  await apply();
  assert.deepEqual(
    await page.evaluate(() =>
      document.querySelector("app-shell").traffic.filters.map((f) => f.value),
    ),
    [String(Date.UTC(2026, 10, 1, 8, 31)), upper],
  );
  await set("duration", []);
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.filterGroups = [{ dimension: "host", values: [{ kind: "text", value: "host-239.test" }] }];
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  await settle();
  await open("host");
  await picker.getByLabel("Search observed values").fill("host-1");
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
  const selected = picker.getByRole("checkbox", { name: "host-239.test", exact: true });
  await selected.focus();
  await selected.press("Space");
  assert.equal(await selected.isChecked(), false);
  assert.equal(
    await selected.evaluate((el) => el === el.getRootNode().activeElement),
    true,
    "Deselected searched-out observation lost keyboard focus",
  );
  await selected.press("Tab");
  await selected.waitFor({ state: "detached" });
  await page.keyboard.press("Escape");
  await set("duration", []);
  const a = page.locator("#session-advanced-239"),
    b = page.locator("#session-advanced-238");
  await a.click();
  await b.click({ modifiers: ["Control"] });
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.selectedDetail?.id === "advanced-238",
  );
  await a.click({ button: "right" });
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedTrafficCount),
    2,
  );
  await traffic.getByRole("heading", { name: "Traffic", exact: true }).click();
  const actions = traffic
      .locator(".selection-actions")
      .getByRole("button", { name: "Actions", exact: true }),
    inspectorMenu = traffic.locator("#selected-request-actions"),
    rowMenu = traffic.locator("#traffic-entry-menu");
  await actions.click();
  await openActionGroup(inspectorMenu, "Filter by");
  await inspectorMenu.getByRole("button", { name: "Host: host-238.test…", exact: true }).click();
  assert.equal(
    await picker.getByRole("checkbox", { name: "host-238.test", exact: true }).isChecked(),
    true,
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic;
      return [t.selectedSessionId, t.selectedTrafficCount, t.followLatest];
    }),
    ["advanced-238", 2, false],
  );
  await page.keyboard.press("Escape");
  await a.click({ button: "right" });
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic;
    t.applyDetail(structuredClone(t.selectedDetail));
  });
  await openActionGroup(rowMenu, "Filter by");
  await rowMenu.getByRole("button", { name: "Host: host-239.test…", exact: true }).click();
  assert.equal(
    await picker.getByRole("checkbox", { name: "host-239.test", exact: true }).isChecked(),
    true,
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedSessionId),
    "advanced-238",
  );
  await page.keyboard.press("Escape");
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.clearTrafficSelection();
    t.followLatest = true;
    await t.refreshSessions(undefined, true);
  });
  await settle();
}
