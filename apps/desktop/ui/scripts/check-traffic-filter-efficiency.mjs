import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fillFilterValues } from "./filter-picker-helpers.mjs";

export async function checkTrafficFilterEfficiency(page, root, { picker, open, wait, settle }) {
  const favorites = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.favorites),
  );
  const follow = await page.evaluate(
    () => document.querySelector("app-shell").traffic.followLatest,
  );
  const field = picker.getByLabel("Filter field", { exact: true });
  const apply = () =>
    picker.getByRole("button", { name: /^(?:Apply filter|Save and apply)$/, exact: true }).click();
  const reset = () =>
    page.evaluate(async () => {
      globalThis.__workspaceFixture.favorites = [];
      await document.querySelector("app-shell").traffic.clearFilters();
    });
  const switchTo = async (dimension) => {
    await field.selectOption(dimension);
    await wait();
  };
  try {
    await reset();
    await open("host");
    await picker
      .getByRole("checkbox", { name: "Pick observed host api.example.test", exact: true })
      .check();
    await switchTo("method");
    await picker.getByRole("checkbox", { name: "POST", exact: true }).check();
    await wait();
    assert.equal(
      await picker.locator(".filter-preview").innerText(),
      "After Apply · 1 of 2 requests",
    );
    assert.match(await picker.locator(".filter-pending").innerText(), /all 2 edited fields/);
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-efficiency-combined.png"),
      animations: "disabled",
    });
    await apply();
    await settle();
    assert.deepEqual(
      await page.evaluate(() => {
        const t = document.querySelector("app-shell").traffic;
        return {
          count: t.totalMatched,
          fields: t.filterGroups.map((group) => group.dimension).sort(),
          pending: t.filterDraftPending,
        };
      }),
      { count: 1, fields: ["host", "method"], pending: false },
    );

    await reset();
    await open("duration");
    await picker.getByLabel("Range from inclusive").fill("10");
    await picker.getByLabel("Range to inclusive").fill("2");
    await switchTo("method");
    await picker.getByRole("checkbox", { name: "GET", exact: true }).check();
    await apply();
    assert.equal(await field.inputValue(), "duration");
    assert.equal(await picker.getByLabel("Range from inclusive").inputValue(), "10");
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterGroups.length),
      0,
    );
    await picker.getByLabel("Range to inclusive").fill("20");
    await apply();
    await settle();
    assert.equal(
      await page.evaluate(() =>
        document
          .querySelector("app-shell")
          .traffic.filters.some((filter) => filter.column === "duration"),
      ),
      true,
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterDraftPending),
      false,
    );

    await reset();
    await open("host");
    await picker.locator(".filter-requirements > summary").click();
    await picker.getByLabel("Column filter value").fill("unfinished");
    await switchTo("method");
    await picker.getByRole("checkbox", { name: "GET", exact: true }).check();
    await apply();
    assert.equal(await field.inputValue(), "host");
    assert.equal(await picker.getByLabel("Column filter value").isVisible(), true);
    assert.equal(await picker.getByLabel("Column filter value").inputValue(), "unfinished");
    await picker.getByRole("button", { name: "Discard changes", exact: true }).click();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterDraftPending),
      false,
    );

    await open("host");
    await fillFilterValues(picker, "api.example.test");
    await switchTo("process");
    await fillFilterValues(picker, "review-process");
    await page.evaluate(() => (globalThis.__workspaceFixture.favoriteWriteError = true));
    await apply();
    await wait();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterGroups.length),
      0,
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.pickerDrafts.size),
      2,
    );
    await page.evaluate(() => (globalThis.__workspaceFixture.favoriteWriteError = false));
    await apply();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterDraftPending),
      false,
    );
    assert.deepEqual(
      await page.evaluate(() =>
        globalThis.__workspaceFixture.favorites.map((entry) => entry.dimension).sort(),
      ),
      ["host", "process"],
    );
    assert.deepEqual(
      await page.evaluate(() =>
        document
          .querySelector("app-shell")
          .traffic.filterGroups.map((group) => group.dimension)
          .sort(),
      ),
      ["host", "process"],
    );

    await reset();
    await open("host");
    await fillFilterValues(picker, "api.example.test");
    await switchTo("process");
    await fillFilterValues(picker, "review-process");
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.deferFavoriteWrite = true;
      s.efficiencyOperation = document
        .querySelector("app-shell")
        .traffic.applyFilter(new Event("submit", { cancelable: true }));
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
    await switchTo("host");
    await fillFilterValues(picker, "otherdomain.example");
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture;
      s.releaseFavoriteWrite();
      await s.efficiencyOperation;
    });
    await wait();
    assert.equal(
      await picker.getByLabel("Is in", { exact: true }).inputValue(),
      "otherdomain.example",
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filterGroups.length),
      0,
      "A delayed batch applied after the user moved to another field and edited it",
    );

    await reset();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.favorites = Array.from({ length: 96 }, (_, i) => ({
        dimension: "host",
        value: { kind: "text", value: `service-${String(i + 1).padStart(2, "0")}.example.test` },
        condition: null,
      }));
    });
    await open("host");
    await wait();
    const custom = picker.locator(".filter-custom");
    assert.equal(await custom.evaluate((element) => element.open), false);
    assert.match(await custom.locator(":scope > summary").innerText(), /96 Favorites/);
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      assert.equal(await picker.getByLabel("Search observed values").isVisible(), true);
      const bounds = await picker.getByLabel("Search observed values").boundingBox();
      const footer = await picker.locator(".filter-picker-footer").boundingBox();
      assert.ok(bounds.y + bounds.height < footer.y);
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/traffic-efficiency-idle-${width}.png`),
        animations: "disabled",
      });
    }
    await custom.locator(":scope > summary").click();
    assert.equal(await picker.getByLabel("Is in", { exact: true }).isVisible(), false);
    const search = picker.getByLabel("Search Favorites", { exact: true });
    await search.fill("service-96");
    await search.press("Enter");
    assert.equal(await picker.isVisible(), true, "Searching Favorites submitted the filter form");
    assert.equal(
      await picker
        .getByRole("group", { name: "Saved values", exact: true })
        .getByRole("checkbox")
        .count(),
      1,
    );
    const choice = picker.getByRole("checkbox", {
      name: "Use saved term service-96.example.test",
      exact: true,
    });
    await choice.check();
    await search.fill("nothing-matches");
    assert.equal(await choice.isVisible(), true);
    await page.evaluate(() => document.querySelector("app-shell").traffic.loadFavorites());
    await wait();
    assert.equal(await search.inputValue(), "nothing-matches");
    assert.equal(await choice.isChecked(), true);
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-efficiency-search.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-efficiency-contrast.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ forcedColors: "none" });
    await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-efficiency-zoom.png"),
      animations: "disabled",
    });
    await page.evaluate(() => (document.documentElement.style.zoom = ""));
    await choice.uncheck();
    assert.equal(
      await search.evaluate((input) => input.getRootNode().activeElement === input),
      true,
      "Removing an out-of-search selection left focus on its hidden checkbox",
    );
    await page.evaluate(async () => {
      globalThis.__workspaceFixture.favorites = globalThis.__workspaceFixture.favorites.slice(0, 2);
      await document.querySelector("app-shell").traffic.loadFavorites();
    });
    await wait();
    await search.fill("");
    assert.equal(
      await search.isVisible(),
      true,
      "A library refresh removed the active search control",
    );
    assert.equal(
      await search.evaluate((input) => input.getRootNode().activeElement === input),
      true,
    );

    await reset();
    await open("response-bytes");
    assert.equal(await picker.locator(".filter-observed-group").isVisible(), false);
    assert.equal(
      await picker.evaluate(
        (picker) =>
          !!(
            picker
              .querySelector(".filter-range")
              .compareDocumentPosition(picker.querySelector(".filter-custom")) &
            Node.DOCUMENT_POSITION_FOLLOWING
          ),
      ),
      true,
    );
    await page.setViewportSize({ width: 960, height: 720 });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-efficiency-range.png"),
      animations: "disabled",
    });
  } finally {
    await page.emulateMedia({ forcedColors: "none" });
    await page.evaluate(() => (document.documentElement.style.zoom = ""));
    await page.evaluate(async (favorites) => {
      const s = globalThis.__workspaceFixture;
      if (s.favoriteWritePending) {
        s.releaseFavoriteWrite();
      }
      await s.efficiencyOperation;
      delete s.efficiencyOperation;
      globalThis.__workspaceFixture.favoriteWriteError = false;
      globalThis.__workspaceFixture.favorites = favorites;
      await document.querySelector("app-shell").traffic.clearFilters();
    }, favorites);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate((follow) => {
      const t = document.querySelector("app-shell").traffic;
      t.followLatest = follow;
      t.renderFollowState();
    }, follow);
  }
}
