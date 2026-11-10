import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficSavedValues(
  page,
  root,
  { traffic, picker, open, apply, settle },
) {
  const saved = await page.evaluate(() => structuredClone(globalThis.__workspaceFixture.sessions));
  const wait = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return !t.pickerLoading && !t.pickerSaving && !t.pickerLibraryLoading;
    });
  const preview = async (count, denominator = 6) => {
    await wait();
    assert.equal(
      await picker.locator(".filter-preview").textContent(),
      `After Apply · ${count} of ${denominator} requests`,
    );
  };
  const savedTerm = (host) =>
    picker.getByRole("checkbox", { name: "Use saved term " + host, exact: true });
  const picked = (host) =>
    picker.getByRole("checkbox", { name: "Pick observed host " + host, exact: true });
  const reset = async (mode = "either", pick) => {
    await picker.getByRole("button", { name: "Clear selection", exact: true }).click();
    await savedTerm("api.example.test").check();
    await savedTerm("*.domain.example").check();
    await picker.getByLabel("Combine selections").selectOption(mode);
    if (pick) await picked(pick).check();
  };
  try {
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      s.favoriteSeed = s.sessions[0];
      s.sessions = [];
      s.favorites = [];
      await t.clearFilters();
    });
    await open("host");
    assert.equal(await picker.locator(".filter-custom").evaluate((details) => details.open), false);
    assert.equal(await picker.getByLabel("Is in", { exact: true }).isVisible(), false);
    const customSettings = picker.getByText("Advanced filters", { exact: true });
    await customSettings.focus();
    await page.keyboard.press("Enter");
    assert.equal(await picker.getByRole("checkbox", { name: /^Require / }).count(), 0);
    await picker
      .getByLabel("Is in", { exact: true })
      .fill("API.EXAMPLE.TEST., *.domain.example, future.example.test, api.example.test");
    await preview(0, 0);
    await picker.getByRole("button", { name: "Save and apply", exact: true }).click();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      0,
    );
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture,
        t = document.querySelector("app-shell").traffic;
      s.sessions = [
        ["api.example.test", "GET"],
        ["api.example.test", "POST"],
        ["cdn.domain.example", "GET"],
        ["eu.cdn.domain.example", "GET"],
        ["domain.example", "POST"],
        ["otherdomain.example", "GET"],
      ].map(([host, method], index) => ({
        ...s.favoriteSeed,
        id: "saved-" + index,
        requestNumber: String(index + 1),
        host,
        method,
        url: `https://${host}/`,
        startedAt: index + 1,
      }));
      t.trafficRevision++;
      await t.refreshSessions(undefined, true);
    });
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      4,
    );
    await open("host");
    assert.equal(
      await picker.locator(".filter-custom").evaluate((details) => details.open),
      true,
      "Saved values did not expand Advanced filters",
    );
    await customSettings.click();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      await t.loadFavorites();
      await t.loadFacets();
    });
    assert.equal(
      await picker.locator(".filter-custom").evaluate((details) => details.open),
      false,
      "Refreshing Favorites overrode the user's collapsed section",
    );
    await customSettings.click();
    await reset();
    await preview(4);
    await picked("domain.example").check();
    await preview(5);
    await picker.getByRole("button", { name: "Cancel", exact: true }).click();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      4,
    );
    await open("host");
    assert.equal(await picked("domain.example").isChecked(), false);
    await picked("domain.example").check();
    await preview(5);
    await traffic.getByRole("heading", { name: "Traffic", exact: true }).click();
    assert.equal(await picker.isVisible(), false);
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      4,
    );
    await open("host");
    assert.equal(await picked("domain.example").isChecked(), false);
    assert.match(
      await picked("cdn.domain.example").locator("..").locator("..").textContent(),
      /Included by \*\.domain.example/,
    );
    assert.equal(await picked("otherdomain.example").isVisible(), true);
    await picked("domain.example").check();
    await preview(5);
    const outsideStar = picker
      .getByRole("group", { name: "Filter values", exact: true })
      .getByRole("button", { name: "Favorite domain.example", exact: true });
    await outsideStar.focus();
    await page.keyboard.press("Space");
    await wait();
    assert.equal(
      await outsideStar.evaluate((button) => button === button.getRootNode().activeElement),
      true,
    );
    assert.equal(await savedTerm("domain.example").isChecked(), false);
    assert.equal(await picked("domain.example").isChecked(), true);
    await preview(5);
    await page.keyboard.press("Escape");
    await open("host");
    assert.equal(await savedTerm("domain.example").isChecked(), false);
    for (const [mode, pick, expected] of [
      ["either", null, 4],
      ["either", "domain.example", 5],
      ["both", "domain.example", 0],
      ["both", "cdn.domain.example", 1],
      ["either", "cdn.domain.example", 4],
      ["ignore-saved", "domain.example", 1],
      ["ignore-saved", null, 6],
    ]) {
      await reset(mode, pick);
      await preview(expected);
      await apply();
      assert.equal(
        await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
        expected,
      );
      if (mode === "ignore-saved" && !pick)
        assert.equal(await traffic.getByRole("button", { name: /^Edit Host:/ }).count(), 0);
      await open("host");
    }
    await reset("both", "domain.example");
    await savedTerm("domain.example").check();
    await preview(1);
    await reset();
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic;
      t.filters = [{ column: "method", operator: "equals", value: "GET", label: "" }];
      t.queryRevision++;
      void t.loadFacets();
    });
    await preview(3, 4);
    await picker.getByLabel("Search observed values").fill("other");
    await preview(3, 4);
    await picker.getByLabel("Search observed values").fill("");
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic;
      t.filters = [];
      t.queryRevision++;
      void t.loadFacets();
    });
    await wait();
    if (!(await picker.locator(".filter-custom").evaluate((details) => details.open))) {
      await picker.locator(".filter-custom summary").click();
    }
    await page.evaluate(() => document.querySelector("app-shell").traffic.loadFacets());
    assert.equal(
      await picker.locator(".filter-custom").evaluate((details) => details.open),
      true,
      "Counts collapsed the authored requirement editor",
    );
    await picker.getByLabel("Column filter value").fill("cdn");
    await preview(4);
    await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
    await preview(2);
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      2,
    );
    await open("host");
    await picker.getByRole("button", { name: "Clear selection", exact: true }).click();
    await picker.getByLabel("Is in", { exact: true }).fill("ok.test, bad:443");
    await picker.getByRole("button", { name: "Save and apply", exact: true }).click();
    assert.match(
      await picker
        .locator(".filter-picker-error")
        .allTextContents()
        .then((values) => values.join(" ")),
      /bad:443/,
    );
    assert.equal(
      await picker.getByLabel("Is in", { exact: true }).inputValue(),
      "ok.test, bad:443",
    );
    await picker.getByLabel("Is in", { exact: true }).fill("retry.example.test");
    await page.evaluate(() => (globalThis.__workspaceFixture.favoriteWriteError = true));
    await picker.getByRole("button", { name: "Save and apply", exact: true }).click();
    assert.equal(
      await picker.getByLabel("Is in", { exact: true }).inputValue(),
      "retry.example.test",
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      2,
    );
    await page.evaluate(() => (globalThis.__workspaceFixture.favoriteWriteError = false));
    await picker.getByRole("button", { name: "Save and select", exact: true }).click();
    await wait();
    await reset();
    await savedTerm("api.example.test")
      .locator("..")
      .locator("..")
      .getByRole("button", { name: "Favorite api.example.test", exact: true })
      .click();
    await wait();
    assert.equal(
      await picker
        .getByRole("group", { name: "Selected for this filter", exact: true })
        .getByRole("checkbox", { name: "Use saved term api.example.test", exact: true })
        .isChecked(),
      true,
    );
    await preview(4);
    await picker.getByRole("button", { name: "Clear saved Host values", exact: true }).click();
    await wait();
    await preview(4);
    await picker.getByLabel("Is in", { exact: true }).fill("intervening.test");
    await picker.getByRole("button", { name: "Save and select", exact: true }).click();
    await wait();
    await picker.getByRole("button", { name: "Undo", exact: true }).click();
    await wait();
    assert.equal(await savedTerm("intervening.test").isChecked(), true);
    await page.evaluate(() => (globalThis.__workspaceFixture.deferFacets = true));
    await picker.getByLabel("Is in", { exact: true }).fill("first.test");
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await picker.getByLabel("Is in", { exact: true }).fill("newer.test");
    await page.evaluate(() => globalThis.__workspaceFixture.releaseFacets());
    await preview(4);
    assert.equal(await picker.getByLabel("Is in", { exact: true }).inputValue(), "newer.test");
    assert.equal(
      await picker
        .getByLabel("Is in", { exact: true })
        .evaluate((input) => input === input.getRootNode().activeElement),
      true,
    );
    await page.keyboard.press("Escape");
    await open("host");
    assert.equal(await picker.getByLabel("Is in", { exact: true }).inputValue(), "");
    const overLimit = Array.from({ length: 129 }, (_, index) => `host-${index}.test`).join(",");
    await picker.getByLabel("Is in", { exact: true }).fill(overLimit);
    await picker.getByRole("button", { name: "Save and select", exact: true }).click();
    assert.equal(await picker.getByLabel("Is in", { exact: true }).inputValue(), overLimit);
    assert.match(
      await picker
        .locator(".filter-picker-error")
        .allTextContents()
        .then((values) => values.join(" ")),
      /128/,
    );
    await picker
      .getByRole("group", { name: "Filter values", exact: true })
      .getByRole("button", { name: "Favorite api.example.test", exact: true })
      .click();
    await wait();
    await reset();
    await apply();
    await open("host");
    await page.evaluate(() => document.querySelector("app-shell").traffic.loadFacets());
    assert.equal(
      await savedTerm("future.example.test")
        .locator("..")
        .locator(".filter-option-count")
        .textContent(),
      "0",
    );
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await page.waitForFunction(() => {
        const t = document.querySelector("app-shell").traffic,
          box = t.filterPicker.getBoundingClientRect();
        return box.bottom <= innerHeight;
      });
      const alignment = await picker.locator(".filter-choice").evaluateAll((rows) =>
        rows.flatMap((row) => {
          const checkbox = row.querySelector('input[type="checkbox"]'),
            count = row.querySelector(".filter-option-count"),
            star = row.querySelector(".filter-star"),
            name = row.querySelector("label > span:first-of-type");
          if (!checkbox || !count || !star || !name) return [];
          const center = (node) => {
            const box = node.getBoundingClientRect();
            return box.top + box.height / 2;
          };
          return [
            {
              label: name.textContent,
              controls:
                Math.max(center(checkbox), center(count), center(star)) -
                Math.min(center(checkbox), center(count), center(star)),
              text: Math.abs(center(name) - center(checkbox)),
            },
          ];
        }),
      );
      assert.ok(
        alignment.every((row) => row.controls < 1 && row.text < 1),
        "Host text, checkboxes, counts and stars are vertically misaligned: " +
          JSON.stringify(alignment),
      );
      const footer = await picker.locator(".filter-picker-footer").boundingBox();
      assert.ok(footer.y >= 0 && footer.y + footer.height <= height + 1);
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/traffic-saved-${width}.png`),
      });
    }
    await page.evaluate(() =>
      document.querySelector("app-shell").setAttribute("data-theme", "dark"),
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-saved-dark.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ colorScheme: "light" });
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-saved-contrast.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ forcedColors: "none" });
    await checkFavoriteReviewRegressions(page, { picker, open, wait, settle });
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      t.closePicker();
      globalThis.__workspaceFixture.favorites = [];
      await t.clearFilters();
    });
    await open("host");
    await wait();
    assert.equal(await picker.locator(".filter-custom").evaluate((details) => details.open), false);
    assert.equal(await picked("cdn.domain.example").isVisible(), true);
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/traffic-basic-${width}.png`),
        animations: "disabled",
      });
    }
  } finally {
    await page.evaluate(async (sessions) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      t.closePicker();
      s.sessions = sessions;
      s.favorites = [];
      s.favoriteWriteError = false;
      await t.clearFilters();
    }, saved);
    await page.setViewportSize({ width: 1440, height: 900 });
  }
}

async function checkFavoriteReviewRegressions(page, { picker, open, wait, settle }) {
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.closePicker();
    await t.clearFilters();
  });
  await open("host");
  await wait();
  const beginSave = async (text, apply) => {
    await page.evaluate(
      ({ text, apply }) => {
        const t = document.querySelector("app-shell").traffic,
          s = globalThis.__workspaceFixture;
        s.deferFavoriteWrite = true;
        t.filterPaste.value = text;
        t.filterPaste.dispatchEvent(new Event("input"));
        s.favoriteOperation = apply
          ? t.applyFilter(new Event("submit", { cancelable: true }))
          : t.savePaste();
      },
      { text, apply },
    );
    await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
  };
  await beginSave("cancelled-apply.test", true);
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    t.closePicker();
    s.releaseFavoriteWrite();
    await s.favoriteOperation;
  });
  await settle();
  assert.deepEqual(
    await page.evaluate(() => document.querySelector("app-shell").traffic.filterGroups),
    [],
    "A pending save applied its filter after Cancel",
  );
  assert.equal(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.favorites.some(
        (entry) => entry.value?.value === "cancelled-apply.test",
      ),
    ),
    true,
    "Cancel undid an explicitly requested Favorite save",
  );
  await open("host");
  await wait();
  await beginSave("old-field.test", false);
  await picker.getByLabel("Filter field", { exact: true }).selectOption("method");
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.favoriteWriteError = true;
    s.releaseFavoriteWrite();
    await s.favoriteOperation;
    s.favoriteWriteError = false;
  });
  await wait();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.pickerLibraryError),
    "",
    "An old field's Favorite failure replaced the current field's feedback",
  );
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.closePicker();
    t.filters = [
      ...Array.from({ length: 13 }, (_, index) => ({
        column: "path",
        operator: "contains",
        value: String(index),
        label: "",
      })),
      { column: "host", operator: "equals", value: "api.example.test", label: "" },
    ];
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  await open("host");
  await wait();
  await picker.getByLabel("Filter comparison").selectOption("equals");
  await picker.getByLabel("Column filter value").fill("future.example.test");
  await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
  await wait();
  assert.equal(
    await picker
      .getByRole("checkbox", { name: "Require equals “future.example.test”", exact: true })
      .isChecked(),
    true,
    "Replacing Equals at the requirement limit was rejected",
  );
  await picker.getByLabel("Filter comparison").selectOption("contains");
  await picker.getByLabel("Column filter value").fill("cdn");
  await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
  assert.match(
    await picker
      .locator(".filter-picker-error")
      .allTextContents()
      .then((values) => values.join(" ")),
    /14/,
  );
  assert.equal(await picker.getByLabel("Column filter value").inputValue(), "cdn");
  const contains = picker.getByRole("checkbox", { name: "Require contains “cdn”", exact: true });
  await contains.click();
  assert.equal(await contains.isChecked(), false, "A saved requirement exceeded the total limit");
  await picker
    .getByRole("checkbox", { name: "Require equals “future.example.test”", exact: true })
    .uncheck();
  await contains.check();
  await wait();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.pickerEntryError),
    "",
    "Correcting the requirement limit left a stale error",
  );
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.closePicker();
    await t.clearFilters();
  });
  await open("host");
  await wait();
  await picker.getByLabel("Filter comparison").selectOption("contains");
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    s.deferFavoriteWrite = true;
    t.filterInput.value = "api";
    s.favoriteOperation = t.addRequirement();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
  await picker.getByLabel("Column filter value").fill("next requirement");
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.releaseFavoriteWrite();
    await s.favoriteOperation;
  });
  await wait();
  assert.equal(
    await picker.getByLabel("Column filter value").inputValue(),
    "next requirement",
    "A completed requirement save erased newer authoring text",
  );
  assert.equal(
    await picker.getByRole("checkbox", { name: "Require contains “api”", exact: true }).isChecked(),
    true,
  );
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    s.deferFavoriteWrite = true;
    s.favoriteOperation = t.addRequirement();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
  await picker.getByLabel("Filter comparison").selectOption("equals");
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.releaseFavoriteWrite();
    await s.favoriteOperation;
  });
  await wait();
  assert.equal(await picker.getByLabel("Column filter value").inputValue(), "next requirement");
  assert.equal(await picker.getByLabel("Filter comparison").inputValue(), "equals");
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    t.closePicker();
    await t.clearFilters();
    const values = Array.from({ length: 129 }, (_, index) => ({
      kind: "text",
      value: `limit-${index}.test`,
    }));
    s.favorites = values.map((value) => ({ dimension: "host", value, condition: null }));
    t.filterGroups = [
      { dimension: "host", values: [], savedValues: values.slice(0, 128), selectionMode: "either" },
    ];
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  await open("host");
  await wait();
  const overflow = picker.getByRole("checkbox", {
    name: "Use saved term limit-128.test",
    exact: true,
  });
  await overflow.click();
  assert.equal(await overflow.isChecked(), false);
  assert.match(
    await page.evaluate(() => document.querySelector("app-shell").traffic.pickerEntryError),
    /128/,
  );
  await picker
    .getByRole("checkbox", { name: "Use saved term limit-0.test", exact: true })
    .uncheck();
  await overflow.check();
  await wait();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.pickerEntryError),
    "",
    "Correcting a saved-selection limit left a stale error",
  );
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    t.closePicker();
    await t.clearFilters();
  });
}
