import { fillFilterValues } from "./filter-picker-helpers.mjs";
import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficFilterGroups(page, root, { picker, open, wait, preview }) {
  const favorites = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.favorites),
  );
  const presentation = await page.evaluate(() => ({
    theme: document.querySelector("app-shell").getAttribute("data-theme"),
    follow: document.querySelector("app-shell").traffic.followLatest,
  }));
  const custom = picker.locator(".filter-custom");
  const observed = picker.locator(".filter-observed-group");
  const requirements = picker.locator(".filter-requirements");
  const mode = picker.getByLabel("Combine groups", { exact: true });
  const summary = picker.locator("#filter-combine-summary");
  const saved = picker.getByRole("checkbox", {
    name: "Use saved term api.example.test",
    exact: true,
  });
  const picked = picker.getByRole("checkbox", {
    name: "Pick observed host otherdomain.example",
    exact: true,
  });
  try {
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      await t.clearFilters();
      globalThis.__workspaceFixture.favorites = [];
    });
    await open("host");
    await wait();
    assert.equal(
      await picker.locator(".filter-combine").isVisible(),
      false,
      "The default observed-only view shows an irrelevant combination control",
    );
    await page.setViewportSize({ width: 960, height: 720 });
    await page.evaluate(() =>
      document.querySelector("app-shell").setAttribute("data-theme", "light"),
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-groups-default.png"),
      animations: "disabled",
    });
    await custom.locator(":scope > summary").click();
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    await fillFilterValues(picker, "pending-custom.test");
    assert.equal(await picker.locator(".filter-combine").isVisible(), true);
    await fillFilterValues(picker, "");
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    await requirements.locator("summary").click();
    await picker.getByLabel("Column filter value").fill("api");
    await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
    await preview(2);
    assert.equal(
      await picker.locator(".filter-combine").isVisible(),
      false,
      "Additional requirements revealed an unrelated membership combination control",
    );
    await picker.getByRole("checkbox", { name: "Require contains “api”", exact: true }).uncheck();
    await requirements.locator("summary").click();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.favorites = ["api.example.test", "*.domain.example"].map(
        (value) => ({
          dimension: "host",
          value: {
            kind: value.startsWith("*.") ? "host-subdomains" : "text",
            value: value.replace(/^\*\./, ""),
          },
          condition: null,
        }),
      );
    });
    await page.evaluate(() => document.querySelector("app-shell").traffic.loadFavorites());
    await wait();
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    assert.equal(await custom.getAttribute("aria-label"), "Saved and custom hosts");
    assert.match(await summary.innerText(), /No values selected/);
    assert.equal(
      await picker.evaluate((picker) => {
        const groups = [
          ".filter-custom",
          ".filter-combine",
          ".filter-observed-group",
          ".filter-requirements",
        ].map((selector) => picker.querySelector(selector));
        return groups
          .slice(1)
          .every(
            (group, index) =>
              groups[index].compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING,
          );
      }),
      true,
      "The combination control and requirements do not follow their membership groups",
    );
    assert.deepEqual(await mode.locator("option").allTextContents(), [
      "Match either group (OR)",
      "Match both groups (AND)",
      "Use observed hosts only",
    ]);
    await saved.check();
    assert.equal(await picker.locator(".filter-combine").isVisible(), true);
    await preview(2);
    assert.equal(await custom.locator("summary .filter-group-count").innerText(), "1 selected");
    await mode.selectOption("both");
    await preview(2);
    assert.match(
      await summary.innerText(),
      /Saved and custom hosts only; no selections in Observed hosts/,
    );
    await picked.check();
    await preview(0);
    assert.equal(await observed.locator(".filter-group-count").innerText(), "1 selected");
    assert.match(
      await summary.innerText(),
      /Saved and custom hosts AND a selection in Observed hosts/,
    );
    await mode.selectOption("either");
    await preview(3);
    assert.match(await summary.innerText(), /Saved and custom hosts OR Observed hosts/);
    await mode.selectOption("ignore-saved");
    await preview(1);
    assert.equal(
      await custom.getByText("Not used in this mode", { exact: true }).isVisible(),
      true,
    );
    assert.equal(await saved.isChecked(), true);
    assert.match(await summary.innerText(), /Only selections in Observed hosts/);
    await picked.uncheck();
    await preview(6);
    assert.match(await summary.innerText(), /No observed hosts selected/);
    await requirements.locator("summary").click();
    await picker.getByLabel("Column filter value").fill("api");
    await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
    await preview(2);
    assert.match(await summary.innerText(), /This group adds no restriction/);
    await picker.getByRole("checkbox", { name: "Require contains “api”", exact: true }).uncheck();
    await preview(6);
    await requirements.locator("summary").click();
    await picked.check();
    await mode.selectOption("either");
    await preview(3);
    await requirements.locator("summary").focus();
    await page.keyboard.press("Enter");
    assert.equal(await picker.getByLabel("Column filter value").isVisible(), true);
    await requirements.locator("summary").click();
    await page.evaluate(async () => {
      const t = document.querySelector("app-shell").traffic;
      await t.loadFavorites();
      await t.loadFacets();
    });
    assert.equal(await requirements.evaluate((details) => details.open), false);
    assert.equal(await custom.evaluate((details) => details.open), true);
    await picker.getByLabel("Filter field", { exact: true }).selectOption("method");
    await wait();
    await picker.getByLabel("Filter field", { exact: true }).selectOption("host");
    await wait();
    assert.equal(await requirements.evaluate((details) => details.open), false);
    assert.equal(await saved.isChecked(), true);
    assert.equal(await picked.isChecked(), true);
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await page.evaluate(() =>
        document.querySelector("app-shell").setAttribute("data-theme", "light"),
      );
      if (width === 800) await custom.locator(":scope > summary").click();
      await picker.locator(".filter-picker-scroll").evaluate((scroll) => (scroll.scrollTop = 0));
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/traffic-groups-${width}.png`),
        animations: "disabled",
      });
      const footer = await picker.locator(".filter-picker-footer").boundingBox();
      assert.ok(footer.y >= 0 && footer.y + footer.height <= height + 1);
    }
    await mode.selectOption("ignore-saved");
    await page.evaluate(() =>
      document.querySelector("app-shell").setAttribute("data-theme", "dark"),
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-groups-dark.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-groups-contrast.png"),
      animations: "disabled",
    });
    await page.emulateMedia({ forcedColors: "none" });
    await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/traffic-groups-zoom.png"),
      animations: "disabled",
    });
    await page.evaluate(async () => {
      document.documentElement.style.zoom = "";
      const t = document.querySelector("app-shell").traffic;
      await t.clearFilters();
      globalThis.__workspaceFixture.favorites = [];
      t.filterGroups = [
        {
          dimension: "host",
          values: [{ kind: "text", value: "otherdomain.example" }],
          savedValues: [],
          selectionMode: "both",
        },
      ];
      await t.refreshSessions(undefined, true);
    });
    await open("host");
    await wait();
    assert.equal(
      await picker.locator(".filter-combine").isVisible(),
      true,
      "An existing nondefault mode cannot be reset without custom values",
    );
    await mode.focus();
    await mode.selectOption("either");
    assert.equal(await picker.locator(".filter-combine").isVisible(), false);
    assert.equal(
      await picker
        .getByLabel("Search observed values")
        .evaluate((input) => input.getRootNode().activeElement === input),
      true,
      "Hiding the combination control left keyboard focus on a hidden input",
    );
  } finally {
    await page.emulateMedia({ forcedColors: "none" });
    await page.evaluate(async (favorites) => {
      document.documentElement.style.zoom = "";
      globalThis.__workspaceFixture.favorites = favorites;
      await document.querySelector("app-shell").traffic.clearFilters();
    }, favorites);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.evaluate(({ theme, follow }) => {
      const shell = document.querySelector("app-shell");
      if (theme === null) shell.removeAttribute("data-theme");
      else shell.setAttribute("data-theme", theme);
      shell.traffic.followLatest = follow;
      shell.traffic.renderFollowState();
    }, presentation);
  }
}
