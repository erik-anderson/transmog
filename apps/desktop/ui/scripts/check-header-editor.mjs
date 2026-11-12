import assert from "node:assert/strict";
import { resolve } from "node:path";
import { fillHeaderEditor } from "./header-editor-helpers.mjs";

/** Shared editing behavior exercised against the production Composer template. */
export async function checkHeaderEditor(page, root) {
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  await page.evaluate(async () => document.querySelector("app-shell").activateView("composer"));
  const editor = page.locator("composer-workspace header-editor");
  await editor.waitFor({ state: "visible" });
  const name = (row) => editor.getByRole("combobox", { name: `Header name ${row}`, exact: true });
  const value = (row) => editor.getByRole("textbox", { name: `Header value ${row}`, exact: true });
  const focused = (field) =>
    field.evaluate((element) => element.getRootNode().activeElement === element);
  const fields = () => editor.evaluate((element) => element.getHeaders());
  const seed = "X-Test: one: two\nx-test: second\nX-Empty: ";
  await editor.evaluate((element, text) => {
    element.value = text;
    globalThis.__headerEditorEvents = [];
    element.addEventListener("header-change", (event) =>
      globalThis.__headerEditorEvents.push(event.detail),
    );
    globalThis.__headerEditorNativeEvents = 0;
    element.parentElement.addEventListener("input", () => globalThis.__headerEditorNativeEvents++);
    element.parentElement.addEventListener("change", () => globalThis.__headerEditorNativeEvents++);
  }, seed);
  assert.equal(await name(1).inputValue(), "X-Test");
  assert.equal(await value(1).inputValue(), "one: two");
  assert.deepEqual(await fields(), [
    { name: "X-Test", value: "one: two" },
    { name: "x-test", value: "second" },
    { name: "X-Empty", value: "" },
  ]);

  await editor.getByRole("button", { name: "Add header", exact: true }).click();
  assert.equal(await focused(name(4)), true, "Add header did not focus the new name");
  await editor.getByRole("status").filter({ hasText: "enter a name or remove" }).waitFor();
  await name(4).fill("X-Arbitrary!");
  const longValue = "wrapped-value-".repeat(250);
  await value(4).fill(longValue);
  assert.equal((await fields())[3].value, longValue);
  assert.equal(
    await value(4).evaluate((element) => element.scrollWidth <= element.clientWidth + 2),
    true,
    "Long values scroll horizontally instead of wrapping",
  );
  await value(4).press("End");
  await value(4).press("Enter");
  assert.equal(
    await value(4).inputValue(),
    longValue,
    "Value wrapping or Enter introduced a newline",
  );

  await value(4).fill("first\nsecond");
  assert.equal(await editor.evaluate((element) => element.reportValidity()), false);
  assert.equal(await focused(value(4)), true);
  assert.match(await editor.getByRole("status").textContent(), /newline/);
  await value(4).fill(longValue);
  await editor.getByRole("button", { name: "Remove header 2", exact: true }).click();
  assert.equal(await focused(name(2)), true, "Removing a row lost useful focus");
  assert.equal(await name(2).inputValue(), "X-Empty");
  await editor.getByRole("button", { name: "Undo remove", exact: true }).click();
  assert.equal(await name(2).inputValue(), "x-test");
  assert.equal(await focused(name(2)), true);

  const eventsBeforeMode = await page.evaluate(() => globalThis.__headerEditorEvents.length);
  await editor.getByRole("button", { name: "Edit as text", exact: true }).click();
  const raw = editor.getByRole("textbox", { name: "Request headers as text", exact: true });
  assert.equal(await focused(raw), true);
  assert.equal(
    await page.evaluate(() => globalThis.__headerEditorEvents.length),
    eventsBeforeMode,
    "Mode-only change dirtied the parent",
  );
  const malformed = "X-Good: yes\nmissing colon\nX-End: keep";
  await fillHeaderEditor(editor, malformed);
  await editor.getByRole("button", { name: "Edit as rows", exact: true }).click();
  assert.equal(await raw.inputValue(), malformed, "Rejected conversion lost the raw draft");
  assert.equal(await editor.evaluate((element) => element.value), malformed);
  assert.match(await editor.getByRole("status").textContent(), /Line 2:/);
  assert.equal(await focused(raw), true);
  assert.equal(
    await raw.evaluate((element) =>
      element.value.slice(element.selectionStart, element.selectionEnd),
    ),
    "missing colon",
  );
  await fillHeaderEditor(editor, seed);
  await editor.getByRole("button", { name: "Edit as rows", exact: true }).click();
  assert.equal(await focused(name(1)), true);
  await value(1).fill("  boundary \t");
  const beforeMode = await fields();
  await editor.getByRole("button", { name: "Edit as text", exact: true }).click();
  await editor.getByRole("button", { name: "Edit as rows", exact: true }).click();
  assert.deepEqual(await fields(), beforeMode, "Mode switch changed authored whitespace");
  assert.equal(
    await page.evaluate(() => globalThis.__headerEditorNativeEvents),
    0,
    "Internal native events escaped the typed contract",
  );

  // Draft restoration must preserve invalid row boundaries, rather than turn
  // an accidental colon/newline into different, valid headers.
  await name(1).fill("Bad:Name");
  await value(1).fill("before\nX-Accidental: after");
  await editor.evaluate((element) => {
    const draft = element.captureDraft();
    element.value = "Other: selection";
    element.restoreDraft(draft);
  });
  assert.equal(await name(1).inputValue(), "Bad:Name");
  assert.equal(await value(1).inputValue(), "before\nX-Accidental: after");
  assert.equal(await editor.evaluate((element) => element.reportValidity()), false);
  await editor.getByRole("button", { name: "Edit as text", exact: true }).click();
  assert.equal(await name(1).isVisible(), true, "Invalid row changed into different raw headers");
  assert.equal(await focused(name(1)), true);
  await name(1).fill("X-Restored");
  assert.equal(await editor.evaluate((element) => element.reportValidity()), false);
  await editor.getByRole("button", { name: "Edit as text", exact: true }).click();
  assert.equal(await focused(value(1)), true);
  await value(1).fill("restored");
  assert.equal(await editor.evaluate((element) => element.reportValidity()), true);

  // An authoritative source load can have the same text as a malformed row.
  // Binding echoes retain the row; explicit replacement must load the valid fields.
  await name(1).fill("X: a");
  await value(1).fill("b");
  const replacement = await editor.evaluate((element) => {
    const text = element.value;
    element.value = text;
    const echoed = element.captureDraft().rows[0];
    element.replaceValue(text);
    return { echoed, loaded: element.getHeaders()[0] };
  });
  assert.deepEqual(replacement, {
    echoed: { name: "X: a", value: "b" },
    loaded: { name: "X", value: "a: b" },
  });

  // The loaded read-only case still supports comparison and copying.
  await editor.evaluate((element) => {
    element.readOnly = true;
  });
  assert.equal(await value(1).getAttribute("readonly"), "");
  assert.equal(
    await editor.getByRole("button", { name: "Add header", exact: true }).isVisible(),
    false,
  );
  await editor.evaluate((element) => {
    element.value = "";
  });
  await editor.getByText("No headers.", { exact: true }).waitFor();
  await editor.getByRole("button", { name: "Edit as text", exact: true }).click();
  await editor.getByRole("button", { name: "Edit as rows", exact: true }).click();
  assert.equal(
    await focused(editor.getByRole("button", { name: "Edit as text", exact: true })),
    true,
  );
  await editor.evaluate((element, text) => {
    element.readOnly = false;
    element.value = text;
  }, `${seed}\nX-Long: ${longValue}`);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/header-editor-wide.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  assert.equal(
    await editor.evaluate((element) => element.scrollWidth <= element.clientWidth + 2),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/header-editor-compact.png"),
  });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/header-editor-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("https://workspace.test/");
}
