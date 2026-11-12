import assert from "node:assert/strict";
import { resolve } from "node:path";

/** Structured URL editing exercised against the production Composer template. */
export async function checkQueryEditor(page, root) {
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  await page.evaluate(async () => document.querySelector("app-shell").activateView("composer"));
  const workspace = page.locator("composer-workspace");
  const editor = workspace.locator("query-editor");
  await editor.waitFor({ state: "visible" });
  const url = workspace.locator('[name="url"]');
  const disclosure = editor.locator("details");
  const summary = editor.locator("summary");
  const name = (row) =>
    editor.getByRole("textbox", { name: `Query parameter name ${row}`, exact: true });
  const value = (row) =>
    editor.getByRole("textbox", { name: `Query parameter value ${row}`, exact: true });
  const equals = (row) =>
    editor.getByRole("checkbox", {
      name: `Include equals sign for query parameter ${row}`,
      exact: true,
    });
  const focused = (field) =>
    field.evaluate((element) => element.getRootNode().activeElement === element);
  await url.fill("https://example.test/?region=us&limit=20");
  await summary.focus();
  await summary.press("Enter");
  assert.equal(await editor.getByRole("checkbox").count(), 0);
  assert.equal(await editor.locator(".query-editor-hint").count(), 0);
  await value(1).focus();
  await value(1).press("Tab");
  assert.equal(
    await focused(editor.getByRole("button", { name: "Remove query parameter 1", exact: true })),
    true,
    "Populated query values should lead directly to the row action",
  );
  await summary.click();
  const original =
    "HTTP://Example.TEST:80/a/../b?na%6De=one+two&repeat=%2f&repeat=%2F&flag&empty=&bad=%E9#keep?x";
  await url.fill(original);
  await editor.evaluate((element) => {
    globalThis.__queryEditorEvents = [];
    element.addEventListener("query-change", (event) =>
      globalThis.__queryEditorEvents.push(event.detail),
    );
    globalThis.__queryEditorNativeEvents = 0;
    element.parentElement.addEventListener("input", (event) => {
      if (element.contains(event.target)) globalThis.__queryEditorNativeEvents++;
    });
    element.parentElement.addEventListener("change", (event) => {
      if (element.contains(event.target)) globalThis.__queryEditorNativeEvents++;
    });
  });
  assert.equal(await disclosure.getAttribute("open"), null);
  await summary.focus();
  await summary.press("Enter");
  assert.equal(await name(1).inputValue(), "name");
  assert.equal(await value(1).inputValue(), "one two");
  assert.equal(await equals(4).isChecked(), false);
  assert.equal(await equals(5).isChecked(), true);
  assert.equal(await editor.getByRole("checkbox").count(), 2);
  assert.equal(await editor.locator(".query-editor-equals-label").first().innerText(), "Include =");
  assert.match(
    await equals(4).evaluate(
      (element) =>
        element.getRootNode().getElementById(element.getAttribute("aria-describedby")).textContent,
    ),
    /flag is name only; flag= is an empty value/,
  );
  await summary.press("Enter");
  await summary.press("Enter");
  assert.equal(await url.inputValue(), original, "Opening query rows normalized the URL");
  assert.equal(await page.evaluate(() => globalThis.__queryEditorEvents.length), 0);

  await name(1).focus();
  await name(1).press("Tab");
  assert.equal(await focused(value(1)), true, "Query fields lost ordinary row tab order");
  await value(1).fill("changed & café");
  assert.equal(
    await url.inputValue(),
    "HTTP://Example.TEST:80/a/../b?na%6De=changed%20%26%20caf%C3%A9&repeat=%2f&repeat=%2F&flag&empty=&bad=%E9#keep?x",
  );
  await equals(4).focus();
  await equals(4).press("Space");
  assert.equal(await focused(equals(4)), true);
  assert.match(await url.inputValue(), /&flag=&empty=/);
  await equals(4).press("Space");
  assert.match(await url.inputValue(), /&flag&empty=/);
  await value(4).evaluate((element) => {
    globalThis.__queryValueInput = element;
  });
  await value(4).focus();
  await value(4).press("y");
  assert.equal(await focused(value(4)), true, "Typing a value moved keyboard focus");
  assert.equal(await equals(4).count(), 0, "A populated value kept the empty-value control");
  assert.equal(await editor.locator(".query-editor-hint").count(), 1);
  assert.equal(
    await value(4).evaluate((element) => element === globalThis.__queryValueInput),
    true,
    "Showing or hiding the empty-value control replaced the value field",
  );
  assert.match(await url.inputValue(), /&flag=y&empty=/);
  await value(4).press("Backspace");
  assert.equal(await focused(value(4)), true, "Clearing a value moved keyboard focus");
  assert.equal(await equals(4).isChecked(), true, "Clearing a value dropped the equals sign");
  assert.equal(await editor.locator(".query-editor-hint").count(), 2);
  await value(4).press("Tab");
  assert.equal(await focused(equals(4)), true, "Empty values should lead to Include =");
  await equals(4).press("Space");
  assert.equal(await focused(equals(4)), true);
  assert.match(await url.inputValue(), /&flag&empty=/);
  await equals(4).press("Tab");
  assert.equal(
    await focused(editor.getByRole("button", { name: "Remove query parameter 4", exact: true })),
    true,
  );
  await name(2).fill("another key");
  assert.match(await url.inputValue(), /&another%20key=%2f&repeat=%2F/);

  await editor.getByRole("button", { name: "Add parameter", exact: true }).click();
  assert.equal(await focused(name(7)), true);
  await name(7).fill("repeat");
  const longValue = "wrapped query value ".repeat(120);
  await value(7).fill(longValue);
  assert.equal(
    await value(7).evaluate((element) => element.scrollWidth <= element.clientWidth + 2),
    true,
  );
  await value(7).press("Enter");
  assert.equal(await value(7).inputValue(), longValue);
  const beforeRemove = await url.inputValue();
  await editor.getByRole("button", { name: "Remove query parameter 2", exact: true }).click();
  assert.equal(await focused(name(2)), true);
  assert.equal(await name(2).inputValue(), "repeat");
  await editor.getByRole("button", { name: "Undo remove", exact: true }).click();
  assert.equal(await focused(name(2)), true);
  assert.equal(await url.inputValue(), beforeRemove, "Query Undo changed escaping or order");
  await editor.evaluate((element) => {
    const snapshot = element.captureDraft();
    element.replaceValue("https://different.test/?other=1");
    element.restoreDraft(snapshot);
  });
  assert.equal(await editor.evaluate((element) => element.value), beforeRemove);
  assert.equal(await value(7).inputValue(), longValue);
  assert.equal(await page.evaluate(() => globalThis.__queryEditorNativeEvents), 0);

  // URL edits replace query rows, including a fragment-only question mark and
  // incomplete input. Merely inspecting either must leave its text untouched.
  for (const untouched of ["https://example.test/?", "https://example.test/#query?no", "draft?"]) {
    await url.fill(untouched);
    assert.equal(await editor.locator(".query-editor-row").count(), 0);
    assert.equal(await editor.evaluate((element) => element.value), untouched);
    await summary.click();
    await summary.click();
    assert.equal(await url.inputValue(), untouched);
  }
  await url.fill("https://example.test/?flag");
  await editor.getByRole("button", { name: "Remove query parameter 1", exact: true }).click();
  assert.equal(
    await focused(editor.getByRole("button", { name: "Add parameter", exact: true })),
    true,
  );
  assert.equal(await url.inputValue(), "https://example.test/");
  await editor.getByRole("button", { name: "Undo remove", exact: true }).click();
  assert.equal(await focused(name(1)), true);
  assert.equal(await url.inputValue(), "https://example.test/?flag");

  await page.setViewportSize({ width: 800, height: 600 });
  const manyRows =
    "https://example.test/?" +
    Array.from({ length: 24 }, (_, index) => `parameter${index + 1}=value${index + 1}`).join("&");
  await url.fill(manyRows);
  await editor.getByRole("button", { name: "Remove query parameter 1", exact: true }).click();
  await editor.getByRole("button", { name: "Undo remove", exact: true }).click();
  assert.equal(await focused(name(1)), true);
  assert.equal(await url.inputValue(), manyRows);
  assert.equal(
    await name(1).evaluate((field) => {
      const box = field.getBoundingClientRect(),
        viewport = field.closest(".composer-request-fields").getBoundingClientRect();
      return box.top >= viewport.top - 1 && box.bottom <= viewport.bottom + 1;
    }),
    true,
    "Undo left the restored query field focused outside the visible draft scroller",
  );

  await url.fill(original);
  await value(1).fill(longValue);
  for (const [width, height, suffix] of [
    [1440, 900, "wide"],
    [960, 720, "compact"],
    [800, 600, "small"],
  ]) {
    await page.setViewportSize({ width, height });
    assert.equal(
      await editor.evaluate((element) => element.scrollWidth <= element.clientWidth + 2),
      true,
      `Query rows overflow at ${width}px`,
    );
    await editor.scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/query-editor-${suffix}.png`),
    });
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/query-editor-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("https://workspace.test/");
}
