import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkHandlerAuthoring(page, scripts, root) {
  const source = () => scripts.evaluate((editor) => editor.sourceEditor.getValue());
  const trigger = scripts.getByRole("button", { name: "Handlers", exact: true });
  const menu = scripts.locator(".handler-menu");
  const waitAnalysis = () => scripts.evaluate((editor) => editor.refreshSourceAnalysis());
  assert.equal(await source(), "");
  const activity = page.locator("automation-workspace .automation-activity");
  assert.equal(
    await activity.isVisible(),
    (await activity.textContent()) !== "No active automation",
    "Only show the workspace summary on Scripts when it has activity to report",
  );
  assert.match(await scripts.locator(".script-state").textContent(), /Paused/);
  assert.match(
    await scripts.locator("draft-status").textContent(),
    /New draft/,
    JSON.stringify(
      await scripts.evaluate((editor) => ({
        dirty: editor.scriptDirty,
        issue: editor.settingsError,
        initial: editor.initialDraft,
        current: editor.scriptDraft(),
        child: editor.querySelector("draft-status").dirty,
      })),
    ),
  );
  assert.equal(
    await scripts.getByRole("button", { name: "Save draft", exact: true }).isDisabled(),
    true,
  );
  await scripts.getByRole("button", { name: "View full sample" }).click();
  assert.match(
    await scripts.locator(".script-help-content").textContent(),
    /Explore the API without changing traffic/,
  );
  await page.keyboard.press("Escape");
  assert.equal(
    await scripts
      .getByRole("button", { name: "View full sample" })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  assert.equal(await source(), "");
  for (const width of [1440, 960, 800]) {
    await page.setViewportSize({ width, height: width === 1440 ? 900 : width === 960 ? 720 : 600 });
    await trigger.click();
    await menu.waitFor({ state: "visible" });
    assert.equal(await menu.getByRole("button").count(), 7);
    assert.equal(
      await menu.evaluate((element) => element.scrollWidth <= element.clientWidth),
      true,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/handler-menu-${width}.png`),
    });
    await page.keyboard.press("Escape");
    assert.equal(await trigger.evaluate((button) => button.matches(":focus")), true);
    const toolbar = await scripts
      .locator(".script-actions > button:visible")
      .evaluateAll((buttons) =>
        buttons.map((button) => ({
          label: button.textContent.trim(),
          top: button.getBoundingClientRect().top,
        })),
      );
    assert.deepEqual(
      toolbar.map((button) => button.label),
      ["Save & enable", "Save draft", "Discard changes"],
    );
    assert.equal(
      new Set(toolbar.map((button) => button.top)).size,
      1,
      "Draft commands wrapped at " + width,
    );
    const heading = await scripts.getByRole("heading", { name: "Traffic scripts" }).boundingBox();
    const headingCenter = heading.y + heading.height / 2;
    for (const name of ["TypeScript source options", "Handlers", "Debug", "Open API docs"]) {
      const box = await scripts.getByRole("button", { name, exact: true }).boundingBox();
      assert.ok(
        box && box.y <= headingCenter && headingCenter <= box.y + box.height,
        name + " wrapped below the heading at " + width,
      );
    }
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/scripts-toolbar-${width}.png`),
    });
  }
  const sourceOptions = scripts.getByRole("button", {
    name: "TypeScript source options",
    exact: true,
  });
  await sourceOptions.focus();
  await page.keyboard.press("Enter");
  const sourceMenu = scripts.locator("#script-source-menu");
  await sourceMenu.getByRole("button", { name: "Validate only" }).waitFor();
  await sourceMenu.locator("button:focus").waitFor();
  await page.keyboard.press("End");
  assert.equal(
    await sourceMenu
      .getByRole("button", { name: "Edit JavaScript" })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/scripts-source-options-800.png"),
  });
  await page.keyboard.press("Escape");
  assert.equal(await sourceOptions.evaluate((button) => button.matches(":focus")), true);
  for (const [opener, action] of [
    [sourceOptions, "Validate only"],
    [scripts.getByRole("button", { name: "Debug", exact: true }), "Test in sandbox"],
    [scripts.getByRole("button", { name: "Debug", exact: true }), "Run sandbox test in debugger"],
    [null, "Save & enable"],
  ]) {
    if (opener) {
      await opener.click();
    }
    await scripts.getByRole("button", { name: action, exact: true }).click();
    await scripts.locator(".handler-trigger:enabled").waitFor();
    assert.match(await scripts.locator("output").textContent(), /Choose a handler from Handlers/);
    assert.equal(
      await trigger.evaluate((button) => button.matches(":focus")),
      true,
      action +
        " left focus on " +
        (await scripts.evaluate((editor) => editor.getRootNode().activeElement?.outerHTML)),
    );
    assert.equal(await scripts.evaluate((editor) => editor.typeScriptIssue), false);
    assert.equal(await source(), "");
  }
  await trigger.click();
  await menu.getByRole("button", { name: "Modify request headers Add", exact: true }).click();
  const inserted = await source();
  assert.match(inserted, /const shouldModifyRequestHeaders = false/);
  assert.match(inserted, /if \(shouldModifyRequestHeaders\)/);
  assert.match(inserted, /setHeader\("User-Agent"/);
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.draftModel.getValueInRange(editor.sourceEditor.getSelection()),
    ),
    "false",
  );
  await page.keyboard.press("Control+z");
  await waitAnalysis();
  assert.equal(await source(), "");
  assert.match(await scripts.locator("draft-status").textContent(), /New draft/);
  await page.keyboard.press("Control+y");
  await waitAnalysis();
  assert.equal(await source(), inserted);
  await trigger.click();
  await menu.getByRole("button", { name: "Modify request headers Go to", exact: true }).click();
  assert.equal(await source(), inserted);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/handler-stub-800.png") });
  await trigger.click();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/handler-menu-existing-800.png"),
  });
  await page.keyboard.press("Escape");
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.draftModel.getValueInRange(editor.sourceEditor.getSelection()),
    ),
    "onRequestHead",
  );
  const surface = scripts.locator("editor-surface").first();
  await surface.getByRole("button", { name: "Expand editor", exact: true }).click();
  const debug = scripts.getByRole("button", { name: "Debug", exact: true });
  await debug.click();
  await scripts
    .locator("#script-debug-menu")
    .getByRole("button", { name: "Run sandbox test in debugger" })
    .waitFor();
  const debugMenu = scripts.locator("#script-debug-menu");
  assert.deepEqual(await debugMenu.getByRole("button").allTextContents(), [
    "Open debugger for live traffic F12",
    "Test in sandbox",
    "Run sandbox test in debugger",
  ]);
  await debugMenu.locator("button:focus").waitFor();
  await page.keyboard.press("ArrowDown");
  assert.equal(
    await scripts
      .getByRole("button", { name: "Test in sandbox", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/scripts-debug-menu-expanded-800.png"),
  });
  await page.keyboard.press("Escape");
  assert.equal(await debug.evaluate((button) => button.matches(":focus")), true);
  assert.equal(await source(), inserted);
  await trigger.click();
  await page.keyboard.press("Escape");
  assert.equal(await surface.evaluate((element) => element.expanded), true);
  await trigger.click();
  await menu.getByRole("button", { name: "Modify request body Add", exact: true }).click();
  await scripts.getByRole("button", { name: "Review settings", exact: true }).click();
  assert.equal(await surface.evaluate((element) => element.expanded), false);
  assert.equal(
    await scripts
      .getByLabel("Read bounded bodies", { exact: true })
      .evaluate((input) => input.matches(":focus")),
    true,
  );
  assert.equal(await scripts.getByLabel("Read bounded bodies", { exact: true }).isChecked(), false);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/handler-settings-800.png"),
  });
  await scripts.getByRole("button", { name: "Discard changes", exact: true }).click();
  assert.equal(await source(), "");
  await waitAnalysis();
  // Syntax and import checks operate on actual parser results in the production worker.
  const cases = [
    [
      '// export function onRequestHead() {}\nconst text="export function onRequestHead() {}";',
      "Add",
    ],
    ["function outer() { function onRequestHead() {} }", "Add"],
    ["function onRequestHead() {}", "Review"],
    ["export const onRequestHead = () => {};", "Review"],
    ["export async function onRequestHead() {}", "Review"],
    ["export function onRequestHead() {", "Review"],
    ["export function onRequestHead() {}\nexport function onRequestHead() {}", "Review"],
    ['export * as onRequestHead from "transmog:api";', "Review"],
    ["// 🌍\r\nexport function onRequestHead() { return {}; }", "Go to"],
  ];
  for (const [text, expected] of cases) {
    await scripts.evaluate((editor, text) => editor.sourceEditor.setValue(text), text);
    await trigger.click();
    assert.equal(
      await menu
        .getByRole("button", { name: "Modify request headers " + expected, exact: true })
        .count(),
      1,
    );
    await page.keyboard.press("Escape");
  }
  await scripts.evaluate((editor) =>
    editor.sourceEditor.setValue(
      'import {pass as next} from "transmog:api";\r\nconst api = 1; // keep\r\n',
    ),
  );
  await trigger.click();
  await menu.getByRole("button", { name: "Modify request headers Add", exact: true }).click();
  assert.match(await source(), /return next\(\)/);
  assert.match(await source(), /const api = 1; \/\/ keep/);
  await scripts.evaluate((editor) => editor.checkTypeScript());
  const delayed = await scripts.evaluate(async (editor) => {
    const inspect = editor.inspectSource.bind(editor);
    let release;
    const gate = new Promise((resolve) => {
      release = resolve;
    });
    editor.inspectSource = async (...args) => {
      await gate;
      return inspect(...args);
    };
    try {
      editor.sourceEditor.setValue("// delayed analysis");
      const opening = editor.toggleHandlers();
      editor.sourceEditor.setValue("// newer source");
      editor.sourceEditor.focus();
      release();
      await opening;
      return {
        source: editor.sourceEditor.getValue(),
        open: editor.handlerMenu.matches(":popover-open"),
        focused: editor.sourceEditor.hasTextFocus(),
      };
    } finally {
      editor.inspectSource = inspect;
    }
  });
  assert.deepEqual(delayed, { source: "// newer source", open: false, focused: true });
  await scripts.evaluate(async (editor) => {
    editor.sourceEditor.setValue("");
    await Promise.all([
      editor.chooseHandler("onRequestHead"),
      editor.chooseHandler("onRequestHead"),
    ]);
  });
  assert.equal((await source()).match(/export function onRequestHead/g).length, 1);
  for (const media of ["dark", "light"]) {
    await page.emulateMedia({
      colorScheme: media,
      forcedColors: media === "light" ? "active" : "none",
    });
    await trigger.click();
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/handler-menu-${media}.png`),
    });
    await page.keyboard.press("Escape");
  }
  await page.emulateMedia({ colorScheme: "light", forcedColors: "none" });
  await page.setViewportSize({ width: 1440, height: 900 });
  const baseline = await scripts.evaluate((editor) => editor.initialDraft);
  const reopen = () =>
    scripts.evaluate(async (editor) => {
      editor.sourceEditor.dispose();
      editor.sourceEditor = null;
      editor.draftModel.dispose();
      editor.draftModel = null;
      editor.scriptReady = false;
      await editor.initializeScriptEditor();
    });
  await page.evaluate((draft) => {
    globalThis.__workspaceFixture.savedScript = {
      ...draft,
      source: "// previous starter",
      sourceEdited: false,
    };
  }, baseline);
  await reopen();
  assert.equal(await source(), "", "Unedited saved starters use the current starter view");
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), false);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.savedScript.source = "// newer unedited starter";
    globalThis.__workspaceFixture.scriptGeneration =
      (globalThis.__workspaceFixture.scriptGeneration ?? 0) + 1;
  });
  if (!(await scripts.getByLabel("Host filter (optional)", { exact: true }).isVisible())) {
    await scripts.locator(".script-settings > summary").click();
  }
  await scripts.getByLabel("Host filter (optional)", { exact: true }).fill("local.test");
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await scripts.getByRole("button", { name: "Compare with saved script", exact: true }).click();
  await scripts.getByRole("button", { name: "Load saved draft", exact: true }).click();
  await waitAnalysis();
  assert.equal(await source(), "", "Conflict recovery uses the current starter view");
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), false);
  const authored = "// deliberately empty authored source";
  await page.evaluate(
    ({ draft, authored }) => {
      globalThis.__workspaceFixture.savedScript = {
        ...draft,
        source: authored,
        sourceEdited: true,
      };
    },
    { draft: baseline, authored },
  );
  await reopen();
  assert.equal(await source(), authored);
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.savedScript;
  });
  await reopen();
  const recoveredJavaScript =
    'import {pass} from "transmog:api"; export function onRequestHead() { return pass(); }';
  await scripts.getByRole("button", { name: "TypeScript source options", exact: true }).click();
  await scripts
    .getByLabel("Source options", { exact: true })
    .getByRole("button", { name: "Edit JavaScript", exact: true })
    .click();
  await scripts.locator(".handler-trigger:enabled").waitFor();
  await scripts.evaluate(
    (editor, source) => editor.sourceEditor.setValue(source),
    recoveredJavaScript,
  );
  await scripts.getByRole("button", { name: "JavaScript source options", exact: true }).click();
  await scripts.getByRole("button", { name: "Return to TypeScript…", exact: true }).click();
  await scripts.evaluate((editor) => {
    editor.sourceEditor.setValue("// temporary TypeScript repair");
    editor.sourceEditor.setValue("");
  });
  await scripts.getByRole("button", { name: "Keep JavaScript", exact: true }).click();
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await scripts.locator(".handler-trigger:enabled").waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.savedScript.sourceEdited),
    true,
    "Recovered authored JavaScript must retain its source provenance",
  );
  await reopen();
  assert.equal(await source(), recoveredJavaScript);
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.savedScript;
  });
  await reopen();
  await scripts.evaluate((editor) =>
    editor.sourceEditor.setValue(
      'import {pass} from "transmog:api"; export function onRequestHead() { return pass(); }',
    ),
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptSave = true;
    globalThis.__workspaceFixture.scriptSavePending = false;
  });
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.scriptSavePending);
  await scripts.evaluate((editor) => editor.sourceEditor.setValue(""));
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptSave = false;
    globalThis.__workspaceFixture.finishScriptSave();
  });
  await scripts.getByRole("button", { name: "Save draft", exact: true, disabled: false }).waitFor();
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await scripts.locator(".handler-trigger:enabled").waitFor();
  await reopen();
  assert.equal(await source(), "");
  assert.equal(
    await scripts.evaluate((editor) => editor.scriptDraft().sourceEdited),
    true,
    "Clearing source during a pending save must preserve an intentionally empty authored draft",
  );
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.savedScript;
  });
  await reopen();
  for (const action of ["activateScript", "testScript", "debugTest"]) {
    const interrupted = await scripts.evaluate(async (editor, action) => {
      const inspect = editor.inspectSource.bind(editor);
      const priorUrl = editor.scriptTestUrl.value;
      let release;
      const pending = new Promise((resolve) => {
        release = resolve;
      });
      editor.inspectSource = async (...args) => {
        await pending;
        return inspect(...args);
      };
      try {
        editor.sourceEditor.setValue(
          'import {pass} from "transmog:api"; export function onRequestHead(){return pass();} // pending ' +
            action,
        );
        const running = editor[action]();
        if (action === "activateScript") {
          editor.scriptForm.elements.namedItem("scriptHost").value = "newer-filter.test";
          editor.markScriptChanged();
        } else {
          editor.scriptTestUrl.value = "https://newer-input.test/";
          editor.markTestChanged();
        }
        release();
        await running;
        return { active: editor.scriptActive, status: editor.scriptText };
      } finally {
        editor.inspectSource = inspect;
        editor.scriptTestUrl.value = priorUrl;
      }
    }, action);
    assert.equal(interrupted.active, false, "Preparation must not apply newer edits");
    assert.match(
      interrupted.status,
      action === "activateScript" ? /draft changed/i : /input changed/i,
    );
    await scripts.getByRole("button", { name: "Discard changes", exact: true }).click();
  }
  // Legacy script tests deliberately exercise the full reference example.
  await scripts.evaluate((editor) => {
    editor.sourceEditor.setValue(
      editor.allHelpSections.find((section) => section.id === "examples").examples[0].code,
    );
    editor.querySelector(".script-settings").open = false;
    editor.helpSearch = "";
    editor.filterScriptHelp();
  });
  await waitAnalysis();
}
