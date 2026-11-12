import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fillFilterValues } from "./filter-picker-helpers.mjs";

export async function checkTrafficFilterReview(
  page,
  root,
  { traffic, picker, open, wait, settle },
) {
  const saved = await page.evaluate(() => ({
    favorites: structuredClone(globalThis.__workspaceFixture.favorites),
    follow: document.querySelector("app-shell").traffic.followLatest,
    zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  }));
  const field = picker.getByLabel("Filter field", { exact: true });
  const apply = async () => {
    await picker
      .getByRole("button", { name: /^(?:Apply filter|Save and apply)$/, exact: true })
      .click();
    await wait();
    await settle();
  };
  const reset = () =>
    page.evaluate(async () => {
      globalThis.__workspaceFixture.favorites = [];
      await document.querySelector("app-shell").traffic.clearFilters();
    });
  const switchTo = async (dimension) => {
    await field.selectOption(dimension);
    await wait();
  };
  const incomplete = () => picker.locator(".filter-preview").innerText();
  let cdp;
  try {
    await reset();
    await open("host");
    const api = picker.getByRole("checkbox", {
      name: "Pick observed host api.example.test",
      exact: true,
    });
    await api.check();
    await apply();
    await open("host");
    await api.uncheck();
    await picker
      .getByRole("checkbox", { name: "Pick observed host cdn.domain.example", exact: true })
      .check();
    await switchTo("method");
    await picker.getByRole("checkbox", { name: "GET", exact: true }).check();
    await picker.getByRole("button", { name: "Close filter", exact: true }).click();
    const remove = traffic.getByRole("button", { name: /^Remove Host:.* filter$/ });
    await remove.focus();
    await page.keyboard.press("Enter");
    await settle();
    assert.equal(
      await traffic
        .getByRole("button", { name: /^Filters/ })
        .evaluate((button) => button.getRootNode().activeElement === button),
      true,
      "Removing a chip left keyboard focus on a removed button",
    );
    assert.deepEqual(
      await page.evaluate(() => [
        ...document.querySelector("app-shell").traffic.pickerDrafts.keys(),
      ]),
      ["method"],
    );
    await open("method");
    assert.equal(
      await picker.getByRole("checkbox", { name: "GET", exact: true }).isChecked(),
      true,
    );
    await apply();
    assert.deepEqual(
      await page.evaluate(() => {
        const t = document.querySelector("app-shell").traffic;
        return { count: t.totalMatched, fields: t.filterGroups.map((group) => group.dimension) };
      }),
      { count: 4, fields: ["method"] },
      "Applying another field resurrected a removed filter",
    );

    await reset();
    await open("duration");
    await picker.getByLabel("Range from inclusive").fill("10");
    await picker.getByLabel("Range to inclusive").fill("2");
    await switchTo("method");
    await picker.getByRole("checkbox", { name: "GET", exact: true }).check();
    await wait();
    assert.match(await incomplete(), /Finish or correct pending fields/);
    await page.setViewportSize({ width: 800, height: 600 });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-review-incomplete-800.png"),
      animations: "disabled",
    });
    const footer = await picker.locator(".filter-picker-footer").boundingBox();
    assert.ok(footer.y >= 0 && footer.y + footer.height <= 600);

    await reset();
    await open("host");
    await picker.locator(".filter-requirements > summary").click();
    await picker.getByLabel("Column filter value").fill("cdn");
    assert.match(await incomplete(), /Finish or correct pending fields/);
    await picker.getByLabel("Column filter value").fill("");
    assert.match(await incomplete(), /6 of 6 requests/);
    await fillFilterValues(picker, "api.example.test, bad:443");
    await wait();
    assert.match(await incomplete(), /Finish or correct pending fields/);

    await reset();
    const requestsBeforeLimits = await page.evaluate(
      () => globalThis.__workspaceFixture.facetRequests.length,
    );
    for (const dimension of ["host", "method"]) {
      if (dimension === "host") {
        await open(dimension);
      } else {
        await switchTo(dimension);
      }
      await picker.locator(".filter-requirements > summary").click();
      for (let index = 0; index < 8; index++) {
        await picker.getByLabel("Column filter value").fill(`value-${index}`);
        await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
        await wait();
      }
    }
    assert.match(await incomplete(), /Finish or correct pending fields/);
    assert.equal(
      await page.evaluate(
        (start) =>
          globalThis.__workspaceFixture.facetRequests
            .slice(start)
            .every((request) => request.query.filters.length <= 14),
        requestsBeforeLimits,
      ),
      true,
      "An invalid aggregate draft reached the native count command",
    );
    await apply();
    assert.match(await picker.getByRole("alert").innerText(), /14 additional requirements/);
    assert.equal(
      await picker.getByRole("button", { name: "Retry counts", exact: true }).count(),
      0,
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.filters.length),
      0,
    );

    await reset();
    cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "America/Los_Angeles" });
    const first = String(Date.UTC(2026, 10, 1, 8, 30)),
      second = String(Date.UTC(2026, 10, 1, 9, 30));
    await page.evaluate(async (second) => {
      const t = document.querySelector("app-shell").traffic;
      t.filters = [{ column: "started-at", operator: "minimum", value: second, label: "" }];
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    }, second);
    await open("host");
    await fillFilterValues(picker, "api.example.test");
    await switchTo("started-at");
    const from = picker.getByLabel("Range from inclusive"),
      text = await from.inputValue();
    assert.match(text, /^2026-11-01T01:30/);
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.deferFavoriteWrite = true;
      s.filterReviewOperation = document
        .querySelector("app-shell")
        .traffic.applyFilter(new Event("submit", { cancelable: true }));
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
    await from.fill("2026-11-01T01:31");
    await from.fill(text);
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture;
      s.releaseFavoriteWrite();
      await s.filterReviewOperation;
    });
    await wait();
    assert.equal(
      await picker.isVisible(),
      true,
      "A delayed Apply committed the old occurrence of an edited ambiguous time",
    );
    assert.equal(
      await page.evaluate(
        () =>
          document
            .querySelector("app-shell")
            .traffic.filters.find((filter) => filter.column === "started-at").value,
      ),
      second,
    );
    assert.equal(
      await page.evaluate(
        () =>
          document
            .querySelector("app-shell")
            .traffic.pickerQuery()
            .filters.find((filter) => filter.column === "started-at").value,
      ),
      first,
    );
    assert.match(await picker.locator(".filter-favorite-status").innerText(), /draft changed/);
    await apply();
    assert.equal(
      await page.evaluate(
        () =>
          document
            .querySelector("app-shell")
            .traffic.filters.find((filter) => filter.column === "started-at").value,
      ),
      first,
    );
  } finally {
    await page.evaluate(async (favorites) => {
      const s = globalThis.__workspaceFixture;
      if (s.favoriteWritePending) {
        s.releaseFavoriteWrite();
      }
      await s.filterReviewOperation;
      delete s.filterReviewOperation;
      s.favorites = favorites;
      await document.querySelector("app-shell").traffic.clearFilters();
    }, saved.favorites);
    if (cdp) {
      await cdp.send("Emulation.setTimezoneOverride", { timezoneId: saved.zone });
      await cdp.detach();
    }
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate((follow) => {
      const t = document.querySelector("app-shell").traffic;
      t.followLatest = follow;
      t.renderFollowState();
    }, saved.follow);
  }
}
