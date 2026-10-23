import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkFilterCoverage } from "./check-filter-coverage.mjs";

export async function checkTrafficFilters(page, root) {
  const traffic = page.locator("traffic-workspace"),
    picker = traffic.locator("#traffic-filter-picker");
  const saved = await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    return {
      sessions: s.sessions,
      streams: s.streams,
      streamItems: s.streamItems,
      dismissedIds: s.dismissedIds,
      preferences: t.preferences,
      sort: t.sort,
      filters: t.filters,
      groups: t.filterGroups,
      follow: t.followLatest,
      lifecycle: t.lifecycle,
      selected: t.selectedSessionId,
    };
  });
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return !t.queryPending && !t.rowRequests.size && t.sessions.every((row) => !row.loading);
    });
  const open = async (dimension) => {
    await traffic.getByRole("button", { name: /^Filters(?: \(\d+\))?$/, exact: false }).click();
    await picker.getByLabel("Filter field", { exact: true }).selectOption(dimension);
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
  };
  const apply = async () => {
    await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
    await settle();
  };
  const optionCount = async (label) =>
    picker
      .getByRole("checkbox", { name: label, exact: true, includeHidden: true })
      .locator("..")
      .locator(".filter-option-count")
      .textContent();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      s.sessions = Array.from({ length: 450 }, (_, i) => ({
        ...seed,
        id: "filter-" + i,
        requestNumber: String(i + 1),
        method: i % 2 ? "POST" : "GET",
        status: i % 2 ? 404 : 200,
        startedAt: i + 1,
        path: "/api/" + i,
        url: "https://example.test/api/" + i,
        streamProtocol: null,
        terminal: "completed",
        httpDurationMs: 7,
        durationMs: i + 1,
      }));
      s.dismissedIds = [];
      s.streams = {};
      s.streamItems = [];
      t.filters = [];
      t.filterGroups = [];
      t.fetchDestinations = [];
      t.liveFilter = "";
      t.searchText = "";
      t.searchInput.value = "";
      t.searchResultId = null;
      t.contentSearchActive = false;
      t.lifecycle = "running";
      t.followLatest = true;
      t.sort = { column: "started-at", direction: "descending" };
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await settle();
    await checkFilterCoverage(page, root, { traffic, picker, open, apply, optionCount, settle });
    await open("traffic-type");
    assert.equal(await optionCount("HTTP"), "450");
    assert.equal(await optionCount("WebSocket"), "0");
    await picker.getByRole("checkbox", { name: "WebSocket", exact: true }).check();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      450,
      "Draft choices applied before Apply",
    );
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      0,
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.followLatest),
      true,
      "A zero filter paused following",
    );
    assert.match(await traffic.locator(".empty-cell").textContent(), /New matches appear/);
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      s.sessions.push({
        ...seed,
        id: "quiet-socket",
        requestNumber: "451",
        path: "/socket",
        url: "https://example.test/socket",
        startedAt: 1000,
        streamProtocol: "web-socket",
        terminal: "active",
        status: 101,
        httpDurationMs: 7,
        durationMs: 0,
        quietStartedAt: Date.now(),
        streamLifetime: {
          durationMs: 0,
          establishedAt: 1000,
          endedAt: null,
          observedUntil: 1000,
          live: true,
          boundary: "live",
        },
      });
      s.streams["quiet-socket"] = {
        state: { protocol: "web-socket", status: "open", atMillis: 1000, reason: null },
        imported: false,
        revision: 1,
        observed: 0,
        evicted: 0,
        sequenceLoss: 0,
        retained: 0,
        clientBytes: 0,
        serverBytes: 0,
        openedAt: 1000,
        lastActivity: null,
      };
      s.channel.onmessage({ exchangeId: "quiet-socket", sequence: 1, lagged: false });
    });
    await page.locator("#session-quiet-socket").waitFor();
    await settle();
    const label = traffic.locator("#session-quiet-socket .stream-identity");
    assert.equal(await label.textContent(), "WebSocket⌄");
    const before = await page.evaluate(
      () => document.querySelector("app-shell").traffic.rowCache.get("quiet-socket").durationMs,
    );
    await page.waitForFunction(
      (value) =>
        document.querySelector("app-shell").traffic.rowCache.get("quiet-socket")?.durationMs >
        value + 500,
      before,
    );
    // A disclosure is independent of row range/multiselection.
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic;
      t.trafficSelection.ids = new Set(["quiet-socket"]);
      t.trafficSelection.anchor = "quiet-socket";
      t.updateTrafficSelection();
    });
    await label.click({ modifiers: ["Shift"] });
    assert.deepEqual(
      await page.evaluate(() => [
        ...document.querySelector("app-shell").traffic.trafficSelection.ids,
      ]),
      ["quiet-socket"],
    );
    await traffic
      .locator("#stream-disclosure")
      .getByRole("button", { name: "Close", exact: true })
      .click();
    assert.equal(await label.evaluate((el) => el === el.getRootNode().activeElement), true);
    await label.focus();
    await page.keyboard.press("Enter");
    assert.equal(
      await traffic
        .locator("#stream-disclosure")
        .getByRole("button", { name: "Filter WebSocket", exact: true })
        .evaluate((el) => el === el.getRootNode().activeElement),
      true,
    );
    await page.keyboard.press("Escape");
    assert.equal(await label.evaluate((el) => el === el.getRootNode().activeElement), true);
    await page.locator("#session-quiet-socket").click();
    await page.waitForFunction(
      () => !!document.querySelector("app-shell").traffic.selectedDetail?.stream,
    );
    assert.match(
      await traffic.locator(".stream-timing").textContent(),
      /Lifetime.*Live.*Handshake.*7 ms/,
    );
    const selected = await page.evaluate(
      () => document.querySelector("app-shell").traffic.selectedSessionId,
    );
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.sessions.push({
        ...s.sessions.at(-1),
        id: "second-socket",
        requestNumber: "452",
        startedAt: 1001,
      });
      s.channel.onmessage({ exchangeId: "second-socket", sequence: 1, lagged: false });
    });
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.newTrafficCount === 1,
    );
    assert.equal(
      await page.locator("#session-second-socket").count(),
      0,
      "Paused view inserted arrivals",
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.selectedSessionId),
      selected,
    );
    await traffic.getByRole("button", { name: /Show updates/ }).click();
    await settle();
    assert.equal(await page.locator("#session-second-socket").count(), 1);
    // An excluded quiet stream crosses a duration threshold without any message hint.
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      t.followLatest = true;
      await t.clearFilters();
      const row = globalThis.__workspaceFixture.sessions.find((row) => row.id === "quiet-socket");
      row.quietStartedAt = Date.now();
      const other = globalThis.__workspaceFixture.sessions.find(
        (row) => row.id === "second-socket",
      );
      other.quietStartedAt = Date.now();
    });
    await open("duration");
    await picker.getByLabel("Range units", { exact: true }).selectOption("s");
    await picker.getByLabel("Range from inclusive", { exact: true }).fill("1.8");
    await apply();
    assert.equal(await page.locator("#session-quiet-socket").count(), 0);
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.virtualList.ids.includes("quiet-socket"),
      { timeout: 6000 },
    );
    await open("traffic-type");
    assert.equal(await optionCount("WebSocket"), "2");
    await picker.getByRole("button", { name: "Close filter", exact: true }).click();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      t.lifecycle = "stopped";
      t.selectedDetail = null;
      await t.clearFilters();
    });
    await open("status");
    assert.equal(await optionCount("2xx"), "225");
    assert.equal(await optionCount("404"), "225");
    await picker.getByRole("checkbox", { name: "2xx", exact: true }).check();
    await picker.getByRole("checkbox", { name: "404", exact: true }).check();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      450,
    );
    await open("method");
    assert.equal(await optionCount("GET"), "225");
    await picker.getByRole("checkbox", { name: "GET", exact: true }).check();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      225,
    );
    assert.equal(await traffic.locator(".filter-chip").count(), 2);
    await traffic.getByRole("button", { name: /^Edit Status:/ }).click();
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    assert.equal(await optionCount("2xx"), "225");
    assert.equal(await optionCount("404"), "0", "Facet did not respect another dimension");
    await picker.getByRole("checkbox", { name: "2xx", exact: true }).uncheck();
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      0,
    );
    await open("status");
    assert.equal(
      await picker.getByRole("checkbox", { name: "404", exact: true }).isVisible(),
      true,
      "Selected zero option was hidden",
    );
    assert.equal(
      await picker.getByRole("checkbox", { name: "404", exact: true }).isChecked(),
      true,
    );
    await picker.getByRole("button", { name: "Close filter", exact: true }).click();
    await traffic.getByRole("button", { name: /^Remove Status:/ }).click();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      227,
    );
    // One shared picker serves a column header, with a custom fallback.
    await traffic.locator("#header-method .column-trigger").click();
    await traffic
      .locator("#column-actions")
      .getByRole("button", { name: "Filter Method…", exact: true })
      .click();
    assert.equal(await picker.getByLabel("Filter field", { exact: true }).inputValue(), "method");
    await picker.getByRole("button", { name: "Clear selection", exact: true }).click();
    await picker.locator(".filter-custom summary").click();
    await picker.getByLabel("Column filter value", { exact: true }).fill("PO");
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      225,
    );
    await traffic.getByRole("button", { name: /^Edit Method:/ }).click();
    assert.match(await picker.locator(".filter-condition").textContent(), /contains PO/);
    await page.keyboard.press("Escape");
    assert.equal(
      await traffic
        .getByRole("button", { name: /^Edit Method:/ })
        .evaluate((el) => el === el.getRootNode().activeElement),
      true,
    );
    await page.evaluate(async () => {
      await document.querySelector("app-shell").traffic.clearFilters();
      globalThis.__workspaceFixture.deferFacets = true;
    });
    await traffic.getByRole("button", { name: /^Filters/ }).click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await picker.getByLabel("Filter field", { exact: true }).selectOption("protocol");
    await page.evaluate(() => globalThis.__workspaceFixture.releaseFacets());
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerLoading);
    assert.equal(await picker.getByRole("checkbox", { name: "HTTP/1.1", exact: true }).count(), 1);
    assert.equal(
      await picker.getByRole("checkbox", { name: "POST", exact: true }).count(),
      0,
      "Stale facets restored a previous field",
    );
    await picker.getByRole("button", { name: "Close filter", exact: true }).click();
    await page.evaluate(() => (globalThis.__workspaceFixture.facetError = true));
    await open("traffic-type");
    assert.match(await picker.locator(".filter-picker-error").textContent(), /Counts unavailable/);
    assert.equal(await optionCount("WebSocket"), "—", "An error appeared as zero");
    await page.evaluate(() => (globalThis.__workspaceFixture.facetError = false));
    await picker.getByRole("button", { name: "Retry counts", exact: true }).click();
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerError);
    assert.equal(await optionCount("WebSocket"), "2");
    // Production templates at all required sizes, with a populated count picker.
    for (const viewport of [
      { width: 1440, height: 900 },
      { width: 960, height: 720 },
      { width: 800, height: 600 },
    ]) {
      await page.setViewportSize(viewport);
      const box = await picker.boundingBox();
      assert.ok(
        box.x >= 0 &&
          box.y >= 0 &&
          box.x + box.width <= viewport.width + 1 &&
          box.y + box.height <= viewport.height + 1,
      );
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/traffic-filters-" + viewport.width + ".png"),
      });
    }
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-filters-contrast.png"),
    });
    await page.emulateMedia({ forcedColors: "none" });
    await picker.getByRole("button", { name: "Close filter", exact: true }).click();
    await page.evaluate(() => {
      document.documentElement.style.zoom = "1.5";
    });
    await open("state");
    const zoom = await picker.boundingBox();
    assert.ok(
      zoom.x >= 0 &&
        zoom.y >= 0 &&
        zoom.x + zoom.width <= 800 + 1 &&
        zoom.y + zoom.height <= 600 + 1,
      "Zoomed picker escaped the window",
    );
    await picker
      .getByRole("button", { name: "Apply filter", exact: true })
      .scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-filters-zoom.png"),
    });
    await page.keyboard.press("Escape");
    await page.evaluate(() => {
      document.documentElement.style.zoom = "";
    });
    await open("fetch-destination");
    await picker.locator(".filter-options details summary").click();
    await picker.getByRole("checkbox", { name: "Stylesheet", exact: true }).check();
    assert.equal(
      await picker
        .getByRole("checkbox", { name: "Stylesheet", exact: true })
        .evaluate((input) => !input.closest("details")),
      true,
      "A selected zero value remained hidden inside More values",
    );
    await apply();
    // Unchecking a selected zero value must keep the keyboard target visible.
    await open("fetch-destination");
    const moreValues = picker.locator(".filter-options details");
    if (await moreValues.evaluate((details) => details.open))
      await moreValues.locator("summary").click();
    const stylesheet = picker.getByRole("checkbox", { name: "Stylesheet", exact: true });
    await stylesheet.focus();
    await page.keyboard.press("Space");
    assert.equal(await stylesheet.isChecked(), false);
    assert.equal(
      await stylesheet.isVisible(),
      true,
      "Unchecking a zero value hid the focused checkbox",
    );
    assert.equal(
      await stylesheet.evaluate((input) => input === input.getRootNode().activeElement),
      true,
      "Unchecking a zero value lost keyboard focus",
    );
    assert.deepEqual(
      await page.evaluate(() => document.querySelector("app-shell").traffic.pickerValues),
      [],
      "Unchecking did not clear the draft",
    );
    await apply();
    assert.deepEqual(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterGroups),
      [],
      "Applying an empty draft did not clear the filter",
    );
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.beforeManyStatuses = s.sessions;
      const seed = s.sessions[0];
      s.sessions = Array.from({ length: 600 }, (_, index) => ({
        ...seed,
        id: "status-" + index,
        requestNumber: String(index + 1),
        status: 100 + index,
      }));
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await open("status");
    assert.equal(
      await optionCount("2xx"),
      "100",
      "A family count was lost after the first facet page",
    );
    assert.equal(await optionCount("304"), "1");
    assert.equal(await optionCount("Other"), "100");
    await page.keyboard.press("Escape");
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.sessions = s.beforeManyStatuses;
      delete s.beforeManyStatuses;
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    // Fixed and selected method counts must resolve beyond the first facet page.
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.beforeManyMethods = s.sessions;
      const seed = s.sessions[0];
      s.sessions = Array.from({ length: 600 }, (_, index) => ({
        ...seed,
        id: "method-" + index,
        requestNumber: String(index + 1),
        method: "A" + String(index).padStart(3, "0"),
      }));
      s.sessions.push({ ...seed, id: "method-get", requestNumber: "601", method: "GET" });
      t.filterGroups = [{ dimension: "method", values: [{ kind: "text", value: "a599" }] }];
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await open("method");
    assert.equal(await optionCount("Other"), "600");
    assert.equal(
      await optionCount("GET"),
      "1",
      "A fixed method count beyond the first facet page appeared as zero",
    );
    assert.equal(
      await optionCount("A599"),
      "1",
      "A selected method count beyond the first facet page appeared as zero",
    );
    assert.match(await picker.textContent(), /601 entries with this field unrestricted/);
    await page.keyboard.press("Escape");
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.sessions = s.beforeManyMethods;
      delete s.beforeManyMethods;
      t.filterGroups = [];
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    // Content search keeps frozen membership, and static viewers never promise live arrivals.
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.searchResults = [{ id: "phase2-snapshot", ids: ["filter-0", "filter-1"] }];
      t.searchResultId = "phase2-snapshot";
      t.contentSearchActive = true;
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await open("method");
    assert.equal(await optionCount("GET"), "1");
    assert.equal(await optionCount("POST"), "1");
    await picker.getByRole("checkbox", { name: "DELETE", exact: true }).check();
    await apply();
    assert.match(await traffic.locator(".empty-cell").textContent(), /search snapshot/);
    assert.doesNotMatch(await traffic.locator(".empty-cell").textContent(), /New matches appear/);
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      await t.clearContentSearch(false);
      t.viewerMode = true;
      t.filterGroups = [{ dimension: "method", values: [{ kind: "text", value: "delete" }] }];
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    assert.match(await traffic.locator(".empty-cell").textContent(), /No matching saved traffic/);
    assert.equal(
      await page.evaluate(
        () => document.querySelector("app-shell").traffic.trafficClock === undefined,
      ),
      true,
    );
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      t.viewerMode = false;
      await t.clearFilters();
      s.sessions[0].path = "/" + "x".repeat(300);
    });
    await open("path");
    assert.match(await picker.textContent(), /1 entry has a value outside exact filter limits/);
    await page.keyboard.press("Escape");
    // Closed/imported stream identity and dual timing remain available, without clocks.
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.sessions = s.sessions.filter((row) => row.id === "quiet-socket");
      Object.assign(s.sessions[0], {
        terminal: "open-at-capture",
        traceId: "saved",
        streamProtocol: "sse",
        quietStartedAt: null,
        durationMs: 4000,
        responseHeadersMs: 12,
        streamLifetime: {
          durationMs: 4000,
          establishedAt: 1000,
          endedAt: null,
          observedUntil: 5000,
          live: false,
          boundary: "open-at-capture",
        },
      });
      s.streams["quiet-socket"] = {
        ...s.streams["quiet-socket"],
        state: { protocol: "sse", status: "open", atMillis: 1000, reason: null },
        imported: true,
      };
      t.filterGroups = [];
      t.filters = [];
      t.followLatest = false;
      await t.refreshSessions(undefined, true);
      await t.inspectTraffic("quiet-socket");
    });
    await settle();
    assert.equal(await traffic.locator(".stream-identity").textContent(), "SSE⌄");
    assert.match(
      await traffic.locator(".stream-timing").textContent(),
      /Open at capture.*4 s.*Response headers.*12 ms/,
    );
    assert.match(
      await traffic.locator(".stream-timing").textContent(),
      /Saved traffic does not age/,
    );
    const imported = await traffic.locator('[data-column-id="duration"] .cell-value').textContent();
    await page.evaluate(
      async () =>
        await document.querySelector("app-shell").traffic.refreshSessions(undefined, true),
    );
    assert.equal(
      await traffic.locator('[data-column-id="duration"] .cell-value').textContent(),
      imported,
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-stream-imported-800.png"),
    });
  } finally {
    await page.evaluate(async (saved) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      Object.assign(s, {
        sessions: saved.sessions,
        streams: saved.streams,
        streamItems: saved.streamItems,
        dismissedIds: saved.dismissedIds,
        facetError: false,
        deferFacets: false,
      });
      if (t.filterPicker.matches(":popover-open")) t.closePicker();
      t.lifecycle = saved.lifecycle;
      t.preferences = saved.preferences;
      t.sort = saved.sort;
      t.filters = saved.filters;
      t.filterGroups = saved.groups;
      t.fetchDestinations = [];
      t.followLatest = saved.follow;
      t.selectedDetail = null;
      t.showStream(false);
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    }, saved);
    await page.setViewportSize({ width: 1280, height: 800 });
  }
}
