import { fillFilterValues } from "./filter-picker-helpers.mjs";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkTrafficFilterDrafts } from "./check-traffic-filter-drafts.mjs";
import { checkTrafficFilterGroups } from "./check-traffic-filter-groups.mjs";
import { checkTrafficFilterEfficiency } from "./check-traffic-filter-efficiency.mjs";
import { checkTrafficFilterReview } from "./check-traffic-filter-review.mjs";

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
    await picker.getByRole("button", { name: "Clear this field", exact: true }).click();
    await savedTerm("api.example.test").check();
    await savedTerm("*.domain.example").check();
    await picker.getByLabel("Combine groups").selectOption(mode);
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
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    assert.equal(await picker.getByLabel("Is in", { exact: true }).isVisible(), false);
    const customSettings = picker.locator(".filter-custom > summary");
    await customSettings.focus();
    await page.keyboard.press("Enter");
    assert.equal(await picker.getByRole("checkbox", { name: /^Require / }).count(), 0);
    assert.equal(await picker.locator(".filter-list-entry label > span").innerText(), "Is in");
    assert.equal(
      await picker.getByLabel("Is in", { exact: true }).getAttribute("placeholder"),
      "Enter values separated by commas or newlines",
    );
    assert.equal(
      await picker
        .getByText("Example: exact-domain.example, *.include-subdomains.example", { exact: true })
        .isVisible(),
      true,
    );
    assert.deepEqual(
      await picker.getByLabel("Combine groups").locator("option").allTextContents(),
      ["Match either group (OR)", "Match both groups (AND)", "Use observed hosts only"],
    );
    await fillFilterValues(
      picker,
      "API.EXAMPLE.TEST., *.domain.example, future.example.test, api.example.test",
    );
    assert.equal(
      await picker.locator(".filter-list-entry label > span").innerText(),
      "Is in (values separated by commas or newlines)",
    );
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
      5,
    );
    await open("host");
    assert.equal(
      await picker.locator(".filter-custom").evaluate((details) => details.open),
      true,
      "Favorite values did not expand the saved/custom group",
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
    await preview(5);
    await picked("domain.example").check();
    await preview(5);
    await picker.getByRole("button", { name: "Discard changes", exact: true }).click();
    await settle();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      5,
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
      5,
    );
    await open("host");
    assert.equal(await picked("domain.example").isChecked(), true);
    await picked("domain.example").uncheck();
    for (const host of ["domain.example", "cdn.domain.example", "eu.cdn.domain.example"])
      assert.match(
        await picked(host).locator("..").locator("..").textContent(),
        /Included by \*\.domain.example/,
      );
    assert.equal(
      await savedTerm("*.domain.example")
        .locator("..")
        .locator(".filter-option-count")
        .textContent(),
      "3",
    );
    assert.match(
      await picked("otherdomain.example").locator("..").locator("..").textContent(),
      /Outside Host filter/,
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
      ["either", null, 5],
      ["either", "domain.example", 5],
      ["both", "domain.example", 1],
      ["both", "otherdomain.example", 0],
      ["both", "cdn.domain.example", 1],
      ["either", "cdn.domain.example", 5],
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
    await picker.getByLabel("Combine groups").selectOption("either");
    await preview(5);
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
    if (!(await picker.locator(".filter-requirements").evaluate((details) => details.open))) {
      await picker.locator(".filter-requirements summary").click();
    }
    await page.evaluate(() => document.querySelector("app-shell").traffic.loadFacets());
    assert.equal(
      await picker.locator(".filter-requirements").evaluate((details) => details.open),
      true,
      "Counts collapsed the authored requirement editor",
    );
    await openRequirements(picker);
    await picker.getByLabel("Column filter value").fill("cdn");
    await wait();
    assert.match(
      await picker.locator(".filter-preview").innerText(),
      /Finish or correct pending fields/,
    );
    await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
    await preview(2);
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      2,
    );
    await open("host");
    await picker.getByRole("button", { name: "Clear this field", exact: true }).click();
    await fillFilterValues(picker, "ok.test, bad:443");
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
    await fillFilterValues(picker, "retry.example.test");
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
    await saveAndSelect(page, picker);
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
        .getByRole("group", { name: "Other filter values", exact: true })
        .getByRole("checkbox", { name: "Use saved term api.example.test", exact: true })
        .isChecked(),
      true,
    );
    await preview(5);
    await picker.getByRole("button", { name: "Clear all Host Favorites", exact: true }).click();
    await wait();
    await preview(5);
    await fillFilterValues(picker, "intervening.test");
    await saveAndSelect(page, picker);
    await wait();
    await picker.getByRole("button", { name: "Undo", exact: true }).click();
    await wait();
    assert.equal(await savedTerm("intervening.test").isChecked(), true);
    await page.evaluate(() => (globalThis.__workspaceFixture.deferFacets = true));
    await fillFilterValues(picker, "first.test");
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await fillFilterValues(picker, "newer.test");
    await page.evaluate(() => globalThis.__workspaceFixture.releaseFacets());
    await preview(5);
    assert.equal(await picker.getByLabel("Is in", { exact: true }).inputValue(), "newer.test");
    assert.equal(
      await picker
        .getByLabel("Is in", { exact: true })
        .evaluate((input) => input === input.getRootNode().activeElement),
      true,
    );
    await page.keyboard.press("Escape");
    await open("host");
    assert.equal(await picker.getByLabel("Is in", { exact: true }).inputValue(), "newer.test");
    const overLimit = Array.from({ length: 129 }, (_, index) => `host-${index}.test`).join(",");
    await fillFilterValues(picker, overLimit);
    await picker.getByRole("button", { name: "Save and apply", exact: true }).click();
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
    await fillFilterValues(picker, "exact-domain.example, *.include-subdomains.example");
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
    await checkTrafficFilterDrafts(page, root, { traffic, picker, open, settle });
    await checkTrafficFilterGroups(page, root, { picker, open, wait, preview });
    await checkTrafficFilterEfficiency(page, root, { picker, open, wait, settle });
    await checkTrafficFilterReview(page, root, { traffic, picker, open, wait, settle });
    await checkFavoriteReviewRegressions(page, { picker, open, wait, settle });
    await checkPickerPresentation(page, { picker, open, wait, apply });
    await checkRetainedChoiceCounts(page, { picker, open, wait, preview });
    await checkPickerEditingReview(page, { picker, open, wait, apply });
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

async function checkPickerPresentation(page, { picker, open, wait, apply }) {
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    t.closePicker();
    await t.clearFilters();
    s.favorites = [
      ...["future.example.test", "api.example.test"].map((value) => ({
        dimension: "host",
        value: { kind: "text", value },
        condition: null,
      })),
      ...["cdn", "domain"].map((value) => ({
        dimension: "host",
        value: null,
        condition: { column: "host", operator: "contains", value },
      })),
    ];
    t.filterGroups = [
      {
        dimension: "host",
        values: [{ kind: "text", value: "otherdomain.example" }],
        savedValues: [{ kind: "text", value: "api.example.test" }],
      },
    ];
    t.filters = [{ column: "host", operator: "contains", value: "domain", label: "" }];
    t.queryRevision++;
    await t.refreshSessions(undefined, true);
  });
  await open("host");
  await wait();
  const rows = (selector) =>
    picker
      .locator(selector)
      .locator('input[type="checkbox"]')
      .evaluateAll((inputs) => inputs.map((input) => input.id));
  const saved = ".filter-custom > .filter-saved[aria-label='Saved values']";
  const observed = ".filter-options";
  const requirements = ".filter-requirements .filter-saved";
  const before = await Promise.all([saved, observed, requirements].map(rows));
  assert.equal(before[0][0], "saved-option-text:api.example.test");
  assert.equal(before[1][0], "filter-option-text:otherdomain.example");
  assert.equal(before[2][0], "requirement-contains:domain");
  const field = picker.getByLabel("Filter field", { exact: true });
  assert.equal(await field.locator('[value="host"]').textContent(), "● Host");
  assert.equal(await field.locator('[value="host"]').getAttribute("aria-label"), "Host (filtered)");
  assert.equal(await field.locator('[value="method"]').textContent(), "Method");
  assert.equal(
    await field.evaluate(
      (select) =>
        getComputedStyle(select).color ===
        getComputedStyle(select.querySelector('[value="host"]')).color,
    ),
    true,
  );
  const scope = picker.locator(
    '[role="status"][title="Other column filters and searches still apply."]',
  );
  assert.match(await scope.innerText(), /Counts ignore this field’s filters/);
  for (const [name, checked] of [
    ["Use saved term api.example.test", false],
    ["Use saved term future.example.test", true],
    ["Pick observed host otherdomain.example", false],
    ["Pick observed host api.example.test", true],
    ["Require contains “domain”", false],
    ["Require contains “cdn”", true],
  ])
    await picker.getByRole("checkbox", { name, exact: true }).setChecked(checked);
  await wait();
  assert.deepEqual(
    await Promise.all([saved, observed, requirements].map(rows)),
    before,
    "Editing checkboxes moved rows before reopening",
  );
  await apply();
  await open("host");
  await wait();
  assert.equal((await rows(saved))[0], "saved-option-text:future.example.test");
  assert.equal((await rows(observed))[0], "filter-option-text:api.example.test");
  assert.equal((await rows(requirements))[0], "requirement-contains:cdn");
  await field.selectOption("method");
  await wait();
  assert.equal(
    await picker
      .getByText("Example: exact-domain.example, *.include-subdomains.example", { exact: true })
      .count(),
    0,
  );
  await picker.getByRole("checkbox", { name: "POST", exact: true }).check();
  assert.equal(
    await field.locator('[value="method"]').textContent(),
    "Method · Unapplied changes",
    "The field chooser did not identify its unapplied draft",
  );
  await picker.getByRole("button", { name: "Discard changes", exact: true }).click();
  await open("process");
  await picker.locator(".filter-custom > summary").click();
  assert.equal(
    await picker.getByLabel("Is in", { exact: true }).getAttribute("placeholder"),
    "Enter one literal value; commas are kept",
  );
  await fillFilterValues(picker, "app,helper.exe");
  assert.equal(
    await picker.locator(".filter-list-entry label > span").innerText(),
    "Is in (one literal value; commas are kept)",
  );
  await picker.getByRole("button", { name: "Discard changes", exact: true }).click();
  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    await t.clearFilters();
    t.filterGroups = [
      {
        dimension: "host",
        values: [],
        savedValues: [{ kind: "text", value: "api.example.test" }],
        selectionMode: "ignore-saved",
      },
    ];
  });
  await open("status");
  await wait();
  assert.equal(
    await field.locator('[value="host"]').textContent(),
    "Host",
    "Paused saved-only filters were marked active",
  );
  const more = picker.locator(".filter-options details");
  await more.locator("summary").click();
  const zero = more.getByRole("checkbox").first();
  const id = await zero.getAttribute("id");
  const order = await rows(observed);
  await zero.check();
  await wait();
  await field.focus();
  assert.deepEqual(
    await rows(observed),
    order,
    "A checked zero-count row jumped out of More values",
  );
  await apply();
  await open("status");
  await wait();
  assert.equal(
    (await rows(observed))[0],
    id,
    "A reopened zero-count selection was not promoted above More values",
  );
}

async function checkRetainedChoiceCounts(page, { picker, open, wait, preview }) {
  const original = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.sessions),
  );
  try {
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture,
        seed = s.sessions[0];
      t.closePicker();
      await t.clearFilters();
      s.sessions = ["A", "Z"].flatMap((prefix) =>
        Array.from({ length: 128 }, (_, index) => ({
          ...seed,
          id: prefix + index,
          method: prefix + String(index).padStart(3, "0"),
        })),
      );
      s.favorites = [];
      t.filterGroups = [
        {
          dimension: "method",
          values: Array.from({ length: 128 }, (_, index) => ({
            kind: "text",
            value: "a" + String(index).padStart(3, "0"),
          })),
        },
      ];
      t.queryRevision++;
      await t.refreshSessions(undefined, true);
    });
    await open("method");
    await wait();
    await picker.locator(".filter-custom > summary").click();
    await picker.getByRole("button", { name: "Clear this field", exact: true }).click();
    await fillFilterValues(
      picker,
      Array.from({ length: 128 }, (_, index) => "Z" + String(index).padStart(3, "0")).join(","),
    );
    await saveAndSelect(page, picker);
    await preview(128, 256);
    const count = (name) =>
      picker
        .getByRole("checkbox", { name, exact: true })
        .locator("..")
        .locator(".filter-option-count")
        .innerText();
    assert.equal(await count("A000"), "1");
    assert.equal(
      await count("A127"),
      "—",
      "A retained row outside the bounded count request falsely showed zero",
    );
  } finally {
    await page.evaluate(async (sessions) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      t.closePicker();
      s.sessions = sessions;
      s.favorites = [];
      t.trafficRevision++;
      await t.clearFilters();
    }, original);
  }
}

async function checkPickerEditingReview(page, { picker, open, wait, apply }) {
  const original = await page.evaluate(() => ({
    sessions: structuredClone(globalThis.__workspaceFixture.sessions),
    favorites: structuredClone(globalThis.__workspaceFixture.favorites),
  }));
  const findings = [];
  try {
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      t.closePicker();
      await t.clearFilters();
      s.sessions = [
        ...s.sessions,
        { ...s.sessions[0], id: "review-extra-method", method: "UNLISTED" },
      ];
      s.favorites = [];
      t.trafficRevision++;
      await t.refreshSessions(undefined, true);
    });
    await open("method");
    await wait();
    const more = picker.locator(".filter-options details");
    assert.equal(await more.evaluate((details) => details.open), false);
    await picker.getByLabel("Search observed values").fill("PATCH");
    await wait();
    await picker.getByRole("button", { name: "Use exact value “PATCH”", exact: true }).click();
    await wait();
    const patch = picker.getByRole("checkbox", { name: "PATCH", exact: true, includeHidden: true });
    assert.equal(await patch.isChecked(), true);
    if (!(await patch.isVisible()))
      findings.push(
        "Use exact value leaves its selected zero-count choice hidden under collapsed More values",
      );
    await patch.uncheck();
    await picker.getByLabel("Filter field", { exact: true }).selectOption("host");
    await wait();
    await picker.locator(".filter-custom > summary").click();
    await fillFilterValues(picker, "editing-review.test");
    await wait();
    const cdp = await page.context().newCDPSession(page);
    try {
      const ax = await cdp.send("Accessibility.getFullAXTree");
      const input = ax.nodes.find(
        (node) => node.role?.value === "textbox" && node.name?.value === "Is in",
      );
      assert.ok(input, "Is in is missing from the accessibility tree");
      if (input.description?.value !== "Enter values separated by commas or newlines")
        findings.push(
          "Filled Is in input does not expose its value-format guidance as an accessible description",
        );
    } finally {
      await cdp.detach();
    }
    await saveAndSelect(page, picker);
    await wait();
    await picker
      .getByRole("group", { name: "Saved values", exact: true })
      .getByRole("button", { name: "Favorite editing-review.test", exact: true })
      .click();
    await wait();
    const term = picker.getByRole("checkbox", {
      name: "Use saved term editing-review.test",
      exact: true,
    });
    assert.equal(await term.isChecked(), true);
    await term.click();
    await wait();
    if (!(await term.isVisible()))
      findings.push("Unchecking a newly authored unstarred value removes its row before reopening");
    assert.deepEqual(findings, []);
    assert.equal(await term.isChecked(), false);
    assert.equal(
      await term.evaluate((input) => input === input.getRootNode().activeElement),
      true,
      "Unchecking the retained term lost focus",
    );
    await apply();
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      original.sessions.length + 1,
    );
    await open("host");
    await wait();
    assert.equal(await term.count(), 0, "The temporary unchecked term survived reopening");
  } finally {
    await page.evaluate(async ({ sessions, favorites }) => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      t.closePicker();
      s.sessions = sessions;
      s.favorites = favorites;
      t.trafficRevision++;
      await t.clearFilters();
    }, original);
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
  await openRequirements(picker);
  await picker.getByLabel("Filter comparison").selectOption("equals");
  await openRequirements(picker);
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
  await openRequirements(picker);
  await picker.getByLabel("Filter comparison").selectOption("contains");
  await openRequirements(picker);
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
  await openRequirements(picker);
  await picker.getByLabel("Filter comparison").selectOption("contains");
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    s.deferFavoriteWrite = true;
    t.filterInput.value = "api";
    s.favoriteOperation = t.addRequirement();
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
  await openRequirements(picker);
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
  await openRequirements(picker);
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

// Exercise saving independently, then explicitly select the saved rows when a
// scenario needs both actions.
async function saveAndSelect(page, picker) {
  const values = await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic;
    return t.previewSavedValues().map((value) => JSON.stringify(value));
  });
  await picker.getByRole("button", { name: "Save to Favorites", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickerSaving);
  const labels = await page.evaluate((values) => {
    const t = document.querySelector("app-shell").traffic;
    return [...t.pickerSavedOptions, ...t.pickerSelectedOptions]
      .filter((row) => values.includes(JSON.stringify(row.value)))
      .map((row) => row.label);
  }, values);
  assert.equal(labels.length, values.length);
  for (const label of labels) {
    await picker.getByRole("checkbox", { name: "Use saved term " + label, exact: true }).check();
  }
}

async function openRequirements(picker) {
  if (!(await picker.locator(".filter-requirements").evaluate((details) => details.open))) {
    await picker.locator(".filter-requirements > summary").click();
  }
}
