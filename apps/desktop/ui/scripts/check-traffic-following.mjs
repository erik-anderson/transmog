import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficFollowing(page, root) {
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  const traffic = page.locator("traffic-workspace"),
    table = traffic.locator(".traffic-table"),
    follow = traffic.getByRole("button", { name: "Follow newest", exact: true });
  const settle = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return !t.queryPending && !t.rowRequests.size && t.sessions.every((row) => !row.loading);
    });
  const position = () =>
    page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic,
        scroller = t.sessionScroller;
      return {
        top: scroller.scrollTop,
        bottom: scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop,
        follow: t.followLatest,
        anchor: t.virtualList.anchor(scroller.scrollTop),
        selected: t.selectedSessionId,
      };
    });
  const readingPosition = async () => {
    const { top, follow, anchor, selected } = await position();
    return { top, follow, anchor, selected };
  };
  const atBottom = async (id) => {
    await page.waitForFunction((id) => {
      const t = document.querySelector("app-shell").traffic,
        scroller = t.sessionScroller,
        row = t.trafficTable.querySelector(`#session-${id}`);
      return (
        t.followLatest &&
        t.sessions.at(-1)?.id === id &&
        row &&
        scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop <= 1 &&
        row.getBoundingClientRect().bottom <= scroller.getBoundingClientRect().bottom + 1
      );
    }, id);
    await settle();
  };
  const arrive = async (count = 1) => {
    const last = await page.evaluate((count) => {
      const s = globalThis.__workspaceFixture,
        start = s.sessions.length;
      for (let index = start; index < start + count; index++)
        s.sessions.push({
          ...s.sessions[0],
          id: "following-" + index,
          requestNumber: String(index + 1),
          // Older and equal timestamps must still append in admission order.
          startedAt: 1,
        });
      const id = s.sessions.at(-1).id;
      s.channel.onmessage({ exchangeId: id, sequence: start, lagged: false });
      return id;
    }, count);
    await page.waitForFunction(
      (id) => document.querySelector("app-shell").traffic.virtualList.ids.at(-1) === id,
      last,
    );
    await settle();
    return last;
  };
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture,
      seed = s.sessions[0];
    s.sessions = Array.from({ length: 250 }, (_, index) => ({
      ...seed,
      id: "following-" + index,
      requestNumber: String(index + 1),
      startedAt: 1000 - index,
      path: "/traffic/" + index,
      url: "https://example.test/traffic/" + index,
    }));
    await t.refreshSessions(undefined, true);
  });
  await atBottom("following-249");
  assert.deepEqual(await page.evaluate(() => document.querySelector("app-shell").traffic.sort), {
    column: "request-number",
    direction: "ascending",
  });
  await table.focus();
  await page.keyboard.press("Control+a");
  assert.equal((await position()).follow, false, "Selecting all requests did not pause following");
  await page.keyboard.press("Escape");
  await page.keyboard.press("F5");
  await atBottom("following-249");
  await page.keyboard.press("Control+Home");
  await settle();
  assert.equal(
    (await position()).follow,
    false,
    "Focus-only keyboard navigation did not pause following",
  );
  assert.equal(
    await traffic
      .locator("#session-following-0")
      .evaluate((row) => row === row.getRootNode().activeElement),
    true,
    "Ctrl+Home did not reveal and focus the oldest request",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedTrafficCount),
    0,
    "Focus-only navigation selected a request",
  );
  await page.keyboard.press("F5");
  await atBottom("following-249");

  // A row read can finish after scrolling moves but before its scroll event is delivered.
  const interrupted = await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic;
    t.sessionScroller.scrollTop -= 80;
    const top = t.sessionScroller.scrollTop;
    t.renderViewport();
    t.sessionScrolled();
    return { top, after: t.sessionScroller.scrollTop, follow: t.followLatest };
  });
  assert.equal(
    interrupted.follow,
    false,
    "An incoming row render resumed scrolling over user navigation",
  );
  assert.equal(
    interrupted.after,
    interrupted.top,
    "An incoming row render moved the reading position",
  );
  await table.focus();
  await page.keyboard.press("F5");
  await atBottom("following-249");
  await atBottom(await arrive(75));
  assert.equal(await table.evaluate((node) => node === node.getRootNode().activeElement), true);
  assert.equal(
    await traffic.getByRole("button", { name: /Show updates|View live traffic/i }).count(),
    0,
  );

  // Background arrivals must preserve following when leaving and returning to Traffic.
  await page.locator('a[data-view="settings"]').click();
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.sessions.push({
      ...s.sessions[0],
      id: "hidden-arrival",
      requestNumber: "9999",
      startedAt: 1,
    });
    s.channel.onmessage({ exchangeId: "hidden-arrival", sequence: 7000, lagged: false });
  });
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.virtualList.ids.at(-1) === "hidden-arrival",
  );
  await page.locator('a[data-view="traffic"]').click();
  await atBottom("hidden-arrival");
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic;
    s.sessions = s.sessions.filter((row) => row.id !== "hidden-arrival");
    await t.refreshSessions(undefined, true);
  });
  await atBottom("following-324");

  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await atBottom("following-324");
    const divider = page.getByRole("separator", {
      name: "Resize traffic list and inspector",
      exact: true,
    });
    await divider.focus();
    await page.keyboard.press("ArrowDown");
    await atBottom("following-324");
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/traffic-following-${width}.png`),
    });
  }

  // A layout change can queue a scroll event before ResizeObserver follows the new bottom.
  const resized = await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      top = t.sessionScroller.scrollTop;
    t.preferences = { ...t.preferences, listSplit: t.preferences.listSplit - 10 };
    await t.$whenRendered();
    const gap =
      t.sessionScroller.scrollHeight - t.sessionScroller.clientHeight - t.sessionScroller.scrollTop;
    const unchanged = t.sessionScroller.scrollTop === top;
    t.sessionScroller.dispatchEvent(new Event("scroll"));
    return { gap, unchanged, follow: t.followLatest };
  });
  assert.ok(
    resized.gap > 1 && resized.unchanged,
    "The layout did not move the bottom independently of scrolling",
  );
  assert.equal(resized.follow, true, "A layout scroll event paused following");
  await atBottom("following-324");

  // Wheel input pauses following; arrivals preserve both the row and pixel offset.
  const bounds = await traffic.locator(".table-wrap").boundingBox();
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.mouse.wheel(0, -220);
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.followLatest);
  await settle();
  const paused = await readingPosition();
  await arrive(400);
  assert.deepEqual(await readingPosition(), paused, "Arrivals moved the paused reading position");
  assert.equal(await follow.isEnabled(), true);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/traffic-following-paused.png"),
  });

  // Reaching the bottom manually must not silently resume following.
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic;
    t.sessionScroller.scrollTop = t.sessionScroller.scrollHeight;
  });
  await settle();
  assert.equal((await position()).follow, false);
  await arrive(40);
  assert.equal((await position()).follow, false);
  assert.ok((await position()).bottom > 100, "Paused arrivals scrolled into view");
  await follow.click();
  await atBottom("following-764");

  // Selection pauses scrolling without withholding incoming traffic or replacing inspection.
  await traffic.locator("#session-following-764").click();
  await page.waitForFunction(() => !!document.querySelector("app-shell").traffic.selectedDetail);
  const selected = await readingPosition();
  await arrive(20);
  assert.deepEqual(
    await readingPosition(),
    selected,
    "An arrival changed selection or inspection position",
  );
  await follow.click();
  await atBottom("following-784");

  // A query completing after the user scrolls must honor the newer pause.
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.queryDelay = 500;
    s.channel.onmessage({ exchangeId: "following-784", sequence: 999, lagged: false });
  });
  await page.waitForFunction(() => !!document.querySelector("app-shell").traffic.queryPending);
  const scroller = await traffic.locator(".table-wrap").boundingBox();
  await page.mouse.move(scroller.x + scroller.width / 2, scroller.y + scroller.height / 2);
  await page.mouse.wheel(0, -300);
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.followLatest);
  const delayed = await readingPosition();
  await settle();
  assert.deepEqual(await readingPosition(), delayed, "A late query resumed automatic scrolling");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.queryDelay = 0;
  });

  // An empty filtered view keeps loading later matches even while paused.
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.liveFilter = "/later-match";
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
    0,
  );
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.sessions.push({
      ...s.sessions[0],
      id: "later-match",
      requestNumber: "9999",
      path: "/later-match",
      url: "https://example.test/later-match",
    });
    s.channel.onmessage({ exchangeId: "later-match", sequence: 1000, lagged: false });
  });
  await traffic.locator("#session-later-match").waitFor({ state: "visible" });
  assert.equal((await position()).follow, false);
  await follow.click();
  await atBottom("later-match");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.liveFilter),
    "/later-match",
  );

  // Alternate sorts keep loading in their chosen order without moving the read anchor.
  for (const [column, direction] of [
    ["status", "ascending"],
    ["path", "descending"],
  ]) {
    await page.evaluate(
      async ({ column, direction }) => {
        const t = document.querySelector("app-shell").traffic;
        t.liveFilter = "";
        await t.applySort(direction, column);
        await t.revealCurrent("following-400");
      },
      { column, direction },
    );
    await settle();
    const before = await readingPosition();
    assert.equal(before.follow, false, "Choosing another sort did not pause following");
    const id = "sorted-" + column;
    await page.evaluate(
      ({ id, column }) => {
        const s = globalThis.__workspaceFixture;
        s.sessions.push({
          ...s.sessions[0],
          id,
          requestNumber: column === "status" ? "10000" : "10001",
          status: column === "status" ? 100 : 200,
          path: "/zzzz-new",
          url: "https://example.test/zzzz-new",
        });
        s.channel.onmessage({ exchangeId: id, sequence: 2000, lagged: false });
      },
      { id, column },
    );
    await page.waitForFunction(
      (id) => document.querySelector("app-shell").traffic.virtualList.ids[0] === id,
      id,
    );
    await settle();
    const after = await readingPosition();
    assert.deepEqual(after.anchor, before.anchor, "A sorted arrival moved the read anchor");
    assert.equal(after.selected, before.selected);
    assert.equal(after.follow, false);
    assert.deepEqual(await page.evaluate(() => document.querySelector("app-shell").traffic.sort), {
      column,
      direction,
    });
  }
  await follow.click();
  await atBottom("sorted-path");
  await page.evaluate(
    async () => await document.querySelector("app-shell").traffic.applySort("descending", "status"),
  );
  await traffic.getByRole("button", { name: "Clear all", exact: true }).click();
  await page.waitForFunction(() => {
    const t = document.querySelector("app-shell").traffic;
    return !t.removingTraffic && !t.totalMatched;
  });
  assert.equal((await position()).follow, true, "Clearing traffic did not resume scrolling");
  assert.deepEqual(await page.evaluate(() => document.querySelector("app-shell").traffic.sort), {
    column: "request-number",
    direction: "ascending",
  });
  await atBottom(await arrive(40));

  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "clear_traffic";
  });
  await traffic.getByRole("button", { name: "Clear all", exact: true }).click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  await page.evaluate(
    async () => await document.querySelector("app-shell").traffic.applySort("descending", "status"),
  );
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.removingTraffic);
  assert.deepEqual(
    await page.evaluate(() => document.querySelector("app-shell").traffic.sort),
    { column: "status", direction: "descending" },
    "A late Clear all overwrote a newer sort choice",
  );
  assert.equal(
    (await position()).follow,
    false,
    "A late Clear all resumed following after a newer sort choice",
  );

  // Clear keeps active sockets; newer bulk selection must survive its completion.
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic,
      seed = s.sessions[0];
    s.sessions = Array.from({ length: 80 }, (_, index) => ({
      ...seed,
      id: "preserved-" + index,
      requestNumber: String(index + 1),
      terminal: "active",
      streamProtocol: "web-socket",
      traceId: null,
    }));
    s.dismissedIds = [];
    t.clearTrafficSelection();
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
    delete s.finishHeldCommand;
    s.holdCommand = "clear_traffic";
  });
  await traffic.getByRole("button", { name: "Clear all", exact: true }).click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  await table.focus();
  await page.keyboard.press("Control+a");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.removingTraffic);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedTrafficCount),
    80,
    "A late Clear all replaced a newer selection of preserved sockets",
  );
  assert.equal((await position()).follow, false);

  // A second scroll while already paused is newer intent than an in-flight Clear.
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic;
    t.sessionScroller.scrollTop = t.sessionScroller.scrollHeight / 2;
    delete s.finishHeldCommand;
    s.holdCommand = "clear_traffic";
  });
  await settle();
  await traffic.getByRole("button", { name: "Clear all", exact: true }).click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  const beforeScroll = await position(),
    scrollBounds = await traffic.locator(".table-wrap").boundingBox();
  await page.mouse.move(
    scrollBounds.x + scrollBounds.width / 2,
    scrollBounds.y + scrollBounds.height / 2,
  );
  await page.mouse.wheel(0, -100);
  await page.waitForFunction(
    (top) => document.querySelector("app-shell").traffic.sessionScroller.scrollTop < top - 20,
    beforeScroll.top,
  );
  const afterScroll = await readingPosition();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.removingTraffic);
  assert.deepEqual(
    await readingPosition(),
    afterScroll,
    "A delayed Clear resumed following over a newer scroll in an already-paused list",
  );

  // Clear supersedes an older cold-row navigation while retaining the surviving inspector.
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic,
      seed = s.sessions[0];
    s.sessions = Array.from({ length: 250 }, (_, index) => ({
      ...seed,
      id: "preserved-" + index,
      requestNumber: String(index + 1),
    }));
    t.clearTrafficSelection();
    await t.resumeLatest();
  });
  await atBottom("preserved-249");
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic;
    delete s.finishHeldCommand;
    s.holdCommand = "traffic_view_rows";
    globalThis.__coldFollowNavigation = t.inspectTraffic("preserved-120", true);
  });
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  await traffic.getByRole("button", { name: "Clear all", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.removingTraffic);
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.finishHeldCommand();
    await globalThis.__coldFollowNavigation;
    delete globalThis.__coldFollowNavigation;
  });
  await atBottom("preserved-249");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedDetail?.id),
    "preserved-120",
    "Clear did not finish the preserved inspector after superseding old navigation",
  );

  // New sorted arrivals must not change the reading position while the inspector is expanded.
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    await t.applySort("ascending", "status");
    await t.revealCurrent("preserved-120", true);
  });
  await settle();
  const expandedPosition = await readingPosition();
  await traffic.getByRole("button", { name: "Expand inspector", exact: true }).click();
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    for (let i = 0; i < 10; i++)
      s.sessions.push({
        ...s.sessions[0],
        id: "expanded-new-" + i,
        requestNumber: String(1000 + i),
        status: 100,
      });
    s.channel.onmessage({ exchangeId: "expanded-new-9", sequence: 9000, lagged: false });
  });
  await page.waitForFunction(() =>
    document.querySelector("app-shell").traffic.virtualList.ids[0]?.startsWith("expanded-new-"),
  );
  await traffic.getByRole("button", { name: "Restore", exact: true }).click();
  await settle();
  assert.deepEqual(
    (await readingPosition()).anchor,
    expandedPosition.anchor,
    "Sorted arrivals changed the list position while the inspector was expanded",
  );
  assert.equal((await readingPosition()).selected, expandedPosition.selected);

  // Leaving an incident window restores following, including arrivals outside that window.
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    s.sessions.forEach((row, index) => {
      row.traceId = index < 100 ? "follow-review" : null;
    });
    await t.resumeLatest();
    await t.onIncidentWindow(
      new CustomEvent("incident-window", {
        detail: { window: { traceId: "follow-review" }, label: "Review" },
      }),
    );
    s.sessions.push({ ...s.sessions.at(-1), id: "after-incident", requestNumber: "2000" });
    await t.refreshSessions(undefined, true);
    await t.clearIncidentWindow();
  });
  assert.equal(
    (await position()).follow,
    true,
    "Leaving an incident window did not restore automatic scrolling",
  );
  await atBottom("after-incident");

  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    await t.applySort("ascending", "status");
    await t.revealCurrent("preserved-120", true);
  });
  await settle();
  const beforeIncident = await readingPosition();
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    await t.onIncidentWindow(
      new CustomEvent("incident-window", {
        detail: { window: { traceId: "follow-review" }, label: "Review" },
      }),
    );
    s.sessions.push({
      ...s.sessions.at(-1),
      id: "sorted-after-incident",
      requestNumber: "2001",
      status: 100,
    });
    await t.refreshSessions(undefined, true);
    await t.clearIncidentWindow();
  });
  await settle();
  assert.deepEqual(
    (await readingPosition()).anchor,
    beforeIncident.anchor,
    "Leaving an incident window lost the paused reading position after sorted arrivals",
  );
  assert.equal((await position()).follow, false);

  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
}
