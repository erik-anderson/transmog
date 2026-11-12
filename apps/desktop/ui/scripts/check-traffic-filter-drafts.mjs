import { fillFilterValues } from "./filter-picker-helpers.mjs";
import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTrafficFilterDrafts(page, root, { traffic, picker, open, settle }) {
  const wait = () =>
    page.waitForFunction(() => {
      const t = document.querySelector("app-shell").traffic;
      return !t.pickerLoading && !t.pickerSaving && !t.pickerLibraryLoading;
    });
  const pending = picker.locator(".filter-pending");
  const button = traffic.getByRole("button", { name: /^Filters/ });
  const field = picker.getByLabel("Filter field", { exact: true });
  const paste = picker.getByLabel("Is in", { exact: true });
  const discard = () =>
    picker.getByRole("button", { name: "Discard changes", exact: true }).click();
  const applied = () =>
    page.evaluate(() => document.querySelector("app-shell").traffic.trafficQuery());
  await page.evaluate(async () => document.querySelector("app-shell").traffic.clearFilters());
  await open("host");
  await wait();
  if (!(await paste.isVisible())) await picker.locator(".filter-custom > summary").click();
  const before = await applied();
  await fillFilterValues(picker, "api.example.test, *.domain.example");
  assert.equal(await pending.isVisible(), true);
  await page.keyboard.press("Escape");
  assert.match(await button.innerText(), /Unapplied changes/);
  assert.deepEqual(await applied(), before, "Escape applied the filter draft");
  await open("host");
  assert.equal(await paste.inputValue(), "api.example.test, *.domain.example");
  await field.selectOption("method");
  await wait();
  const post = picker.getByRole("checkbox", { name: "POST", exact: true });
  await post.check();
  await field.selectOption("host");
  await wait();
  assert.equal(await paste.inputValue(), "api.example.test, *.domain.example");
  await discard();
  assert.doesNotMatch(
    await button.innerText(),
    /Unapplied changes/,
    "Discard did not reset the whole filter draft",
  );
  await open("method");
  assert.equal(await post.isChecked(), false);
  assert.equal(await pending.count(), 0, "Reverting a selection left a dirty state");
  assert.doesNotMatch(await button.innerText(), /Unapplied changes/);
  await post.check();
  await traffic.getByRole("heading", { name: "Traffic", exact: true }).click();
  assert.equal(await picker.isVisible(), false);
  assert.deepEqual(await applied(), before, "Outside dismissal applied a draft");
  await open("method");
  assert.equal(await post.isChecked(), true);
  await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
  await settle();
  assert.doesNotMatch(await button.innerText(), /Unapplied changes/);
  assert.equal((await applied()).filterGroups[0].dimension, "method");

  await open("host");
  await wait();
  await fillFilterValues(picker, "favorites-only.test");
  await picker.getByRole("button", { name: "Save to Favorites", exact: true }).click();
  await wait();
  const favorite = picker.getByRole("checkbox", {
    name: "Use saved term favorites-only.test",
    exact: true,
  });
  assert.equal(await favorite.isChecked(), false, "Saving a Favorite checked its filter box");
  assert.equal(await pending.count(), 0);
  assert.match(
    await picker.locator(".filter-favorite-status").innerText(),
    /Saved 1 value to Favorites/,
  );
  await favorite.check();
  await picker.getByRole("button", { name: "Close filter", exact: true }).click();
  await open("host");
  assert.equal(await favorite.isChecked(), true);
  await discard();
  await open("host");
  await picker.locator(".filter-custom > summary").click();
  assert.equal(
    await favorite.isChecked(),
    false,
    "Discard removed the saved Favorite or retained its draft selection",
  );
  await fillFilterValues(picker, "otherdomain.example");
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await wait();
    const footer = await picker.locator(".filter-picker-footer").boundingBox();
    assert.ok(footer.y >= 0 && footer.y + footer.height <= height + 1);
    assert.equal(await pending.isVisible(), true);
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/traffic-draft-${width}.png`),
    });
  }
  await picker.getByRole("button", { name: "Save and apply", exact: true }).click();
  await settle();
  assert.doesNotMatch(await button.innerText(), /Unapplied changes/);
  await open("host");
  assert.equal(await pending.count(), 0);
  assert.equal(await paste.inputValue(), "");
  assert.equal(
    await picker
      .getByRole("checkbox", { name: "Use saved term otherdomain.example", exact: true })
      .isChecked(),
    true,
  );

  await field.selectOption("duration");
  await wait();
  await picker.getByLabel("Range from inclusive").fill("10");
  await picker.getByLabel("Range to inclusive").fill("2");
  await page.keyboard.press("Escape");
  await open("duration");
  assert.equal(await picker.getByLabel("Range from inclusive").inputValue(), "10");
  assert.equal(await picker.getByLabel("Range to inclusive").inputValue(), "2");
  await discard();
  await open("host");
  await wait();
  await openRequirements(picker);
  await picker.getByLabel("Filter comparison").selectOption("equals");
  await openRequirements(picker);
  await picker.getByLabel("Column filter value").fill("draft-comparison.test");
  await page.keyboard.press("Escape");
  await open("host");
  assert.equal(
    await picker.getByLabel("Column filter value").inputValue(),
    "draft-comparison.test",
  );
  assert.equal(await picker.getByLabel("Filter comparison").inputValue(), "equals");
  await traffic.locator("#header-method").getByRole("button").click();
  await page.keyboard.press("Escape");
  await open("host");
  assert.equal(
    await picker.getByLabel("Column filter value").inputValue(),
    "draft-comparison.test",
    "Opening a column menu overwrote the dismissed comparison draft",
  );
  assert.equal(await picker.getByLabel("Filter comparison").inputValue(), "equals");
  const beforeComparison = await applied();
  await picker.locator(".filter-requirements > summary").click();
  await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
  assert.equal(await picker.isVisible(), true, "Apply silently dropped unfinished comparison text");
  assert.equal(
    await picker.getByLabel("Column filter value").isVisible(),
    true,
    "Apply left its comparison validation hidden",
  );
  assert.deepEqual(await applied(), beforeComparison);
  await picker.getByRole("button", { name: "Add requirement", exact: true }).click();
  await wait();
  assert.equal(await picker.getByLabel("Column filter value").inputValue(), "");
  assert.equal(
    await picker.getByLabel("Column filter value").evaluate((input) => input.checkValidity()),
    true,
    "Adding the comparison left its cleared authoring input invalid",
  );
  await picker.getByRole("button", { name: "Apply filter", exact: true }).click();
  await settle();
  assert.equal(
    await picker.isVisible(),
    false,
    "Comparison validation prevented the corrected filter from applying",
  );

  await page.evaluate(async () => document.querySelector("app-shell").traffic.clearFilters());
  await open("host");
  await wait();
  const beforeLateEdit = await applied();
  await fillFilterValues(picker, "late-apply.test");
  await page.evaluate(() => {
    const t = document.querySelector("app-shell").traffic,
      s = globalThis.__workspaceFixture;
    s.deferFavoriteWrite = true;
    s.favoriteOperation = t.applyFilter(new Event("submit", { cancelable: true }));
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.favoriteWritePending);
  await openRequirements(picker);
  await picker.getByLabel("Column filter value").fill("newer comparison");
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.releaseFavoriteWrite();
    await s.favoriteOperation;
  });
  await wait();
  assert.deepEqual(
    await applied(),
    beforeLateEdit,
    "A pending save applied after comparison text was edited",
  );
  assert.equal(await picker.isVisible(), true);
  assert.equal(await picker.getByLabel("Column filter value").inputValue(), "newer comparison");
  await discard();

  await page.evaluate(async () => {
    const t = document.querySelector("app-shell").traffic;
    await t.clearFilters();
    t.filterGroups = Array.from({ length: 32 }, (_, index) => ({
      dimension: `extension:review:${index}`,
      values: [{ kind: "text", value: "test" }],
    }));
  });
  await open("host");
  await wait();
  await fillFilterValues(picker, "favorite-at-field-limit.test");
  await picker.getByRole("button", { name: "Save to Favorites", exact: true }).click();
  await wait();
  assert.equal(
    await paste.inputValue(),
    "",
    "The applied filter field limit prevented an independent Favorite save",
  );
  assert.equal(
    await picker
      .getByRole("checkbox", { name: "Use saved term favorite-at-field-limit.test", exact: true })
      .isChecked(),
    false,
  );
  await page.evaluate(async () => document.querySelector("app-shell").traffic.clearFilters());
}

async function openRequirements(picker) {
  if (!(await picker.locator(".filter-requirements").evaluate((details) => details.open))) {
    await picker.locator(".filter-requirements > summary").click();
  }
}
