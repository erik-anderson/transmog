import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkMonacoIcons } from "./check-monaco-icons.mjs";

export async function checkScripts(page, root) {
  const scripts = page.locator("script-editor");
  const sourceSurface = scripts.locator("editor-surface").first();
  const state = () => scripts.locator(".script-state");
  const waitIdle = () =>
    page.waitForFunction(() => {
      const editor = document.querySelector("app-shell").shadowRoot.querySelector("script-editor");
      return editor.scriptReady && !editor.scriptBusy;
    });
  const append = async (text) =>
    scripts.evaluate(
      (editor, text) => editor.sourceEditor.setValue(editor.sourceEditor.getValue() + text),
      text,
    );
  const fixture = async (values) =>
    page.evaluate((values) => Object.assign(globalThis.__workspaceFixture, values), values);
  await waitIdle();
  assert.match(await scripts.locator(".script-scope").textContent(), /example.test/);
  const helpButton = scripts.getByRole("button", { name: "Script API", exact: true });
  const help = scripts.locator(".script-help-dialog");
  await helpButton.click();
  assert.equal(await help.evaluate((dialog) => dialog.matches(":modal")), true);
  assert.match(await help.textContent(), /revision 2/);
  await help.getByLabel("Search Script API", { exact: true }).fill("bypass");
  assert.match(
    await help.locator(".script-help-content").textContent(),
    /fresh host with empty exchange state/,
  );
  await page.keyboard.press("Escape");
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("script-editor")
      .scriptHelpButton.matches(":focus"),
  );
  await helpButton.click();
  await help.getByLabel("Search Script API", { exact: true }).fill("Upstream");
  assert.match(await help.locator(".script-help-content").textContent(), /Failure categories/);
  await help.getByLabel("Search Script API", { exact: true }).fill("no-such-help-section");
  assert.match(await help.locator(".script-help-content").textContent(), /No matching help/);
  await help.getByLabel("Search Script API", { exact: true }).fill("Complete examples");
  const helpContent = help.locator(".script-help-content");
  await helpContent.evaluate((content) => (content.scrollTop = content.scrollHeight));
  assert.ok(await helpContent.evaluate((content) => content.scrollTop > 0));
  await help.getByLabel("Search Script API", { exact: true }).fill("onFailed");
  assert.equal(
    await helpContent.evaluate((content) => content.scrollTop),
    0,
    "A new help search retained the previous result's scroll position",
  );
  for (const query of ["request body", "body limits"]) {
    await help.getByLabel("Search Script API", { exact: true }).fill(query);
    assert.match(await helpContent.textContent(), /Filters, buffering and limits/);
  }
  await help.getByLabel("Search Script API", { exact: true }).fill("Complete examples");
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 800, height: 600 },
  ]) {
    await page.setViewportSize(viewport);
    assert.equal(await help.getByRole("button", { name: "Close", exact: true }).isVisible(), true);
    assert.equal(await help.evaluate((dialog) => dialog.scrollWidth <= dialog.clientWidth), true);
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-help-${viewport.width}.png`),
    });
    await help.getByLabel("Search Script API", { exact: true }).fill("onFailed");
    assert.equal(
      await help
        .locator(".script-help-table")
        .evaluate((table) => table.scrollWidth <= table.clientWidth),
      true,
      "The phase contract requires horizontal scrolling",
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-help-events-${viewport.width}.png`),
    });
    await help.getByLabel("Search Script API", { exact: true }).fill("Complete examples");
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/script-help-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await help.getByRole("button", { name: "Close", exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 900 });
  assert.equal(
    await scripts.getByLabel("Failure category (onFailed)", { exact: true }).inputValue(),
    "Upstream",
  );
  await scripts.evaluate((editor) => editor.validateScript());
  assert.match(await scripts.locator("output").textContent(), /source and capabilities are valid/);
  await checkMonacoIcons(scripts.locator(".monaco-editor-host").first());
  assert.deepEqual(
    await scripts.evaluate(async (editor) => {
      const { getTypeScriptWorker } = await import("/monaco.js");
      const worker = await (await getTypeScriptWorker())(editor.draftModel.uri);
      return (await worker.getSemanticDiagnostics(editor.draftModel.uri.toString())).map(
        (item) => item.messageText,
      );
    }),
    [],
    "Default script API import has TypeScript errors",
  );
  assert.deepEqual(
    await scripts.evaluate(async (editor) => {
      const { getTypeScriptWorker } = await import("/monaco.js");
      const examples = editor.allHelpSections.find((section) => section.id === "examples").examples;
      const issues = [];
      for (const [index, example] of examples.entries()) {
        const model = editor.monaco.editor.createModel(
          example.code,
          "typescript",
          editor.monaco.Uri.parse(`file:///script-help/example-${index}.ts`),
        );
        try {
          const worker = await (await getTypeScriptWorker())(model.uri);
          issues.push(
            ...(await worker.getSemanticDiagnostics(model.uri.toString())).map(
              (item) => item.messageText,
            ),
          );
        } finally {
          model.dispose();
        }
      }
      return issues;
    }),
    [],
    "Bundled help examples have TypeScript errors",
  );
  assert.ok(
    (await scripts.locator(".monaco-editor-host").first().boundingBox()).height >= 192,
    "Source editor has insufficient space",
  );
  await sourceSurface.getByRole("button", { name: "Expand editor", exact: true }).click();
  assert.equal(
    await sourceSurface.locator("dialog").evaluate((dialog) => dialog.matches(":modal")),
    true,
  );
  assert.equal(
    await sourceSurface.getByRole("button", { name: "Save & enable", exact: true }).isVisible(),
    true,
  );
  await helpButton.click();
  await page.keyboard.press("Escape");
  assert.equal(await sourceSurface.locator("dialog").evaluate((dialog) => dialog.open), true);
  assert.equal(await helpButton.evaluate((button) => button.matches(":focus")), true);
  await page.keyboard.press("Escape");
  assert.equal(
    await sourceSurface.locator("dialog").evaluate((dialog) => dialog.matches(":modal")),
    false,
  );

  // One action validates, saves, and enables; reopening selects the same source and settings.
  await scripts.getByRole("button", { name: "Save & enable", exact: true }).click();
  await waitIdle();
  assert.equal(await scripts.getByRole("button", { name: "Pause script", exact: true }).count(), 1);
  assert.match(await state().textContent(), /Saved/);
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), false);
  const activated = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.savedScript),
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].source),
    activated.source,
  );
  assert.equal(
    await scripts.getByRole("button", { name: "Save draft", exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await scripts.getByRole("button", { name: "Save & apply", exact: true }).isDisabled(),
    true,
  );
  await append("\n// discard this edit");
  await scripts.locator(".script-settings > summary").click();
  await scripts.getByLabel("Host filter (optional)", { exact: true }).fill("discard.example.test");
  await scripts.getByLabel("Method filter (optional)", { exact: true }).fill("DELETE");
  await scripts.getByRole("button", { name: "Discard changes", exact: true }).click();
  assert.equal(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    activated.source,
  );
  assert.equal(
    await scripts.getByLabel("Host filter (optional)", { exact: true }).inputValue(),
    activated.prefilter.host ?? "",
  );
  assert.equal(
    await scripts.getByLabel("Method filter (optional)", { exact: true }).inputValue(),
    activated.prefilter.method ?? "",
  );
  assert.equal(
    await scripts.getByRole("button", { name: "Save draft", exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].source),
    activated.source,
    "Discard changed the live script",
  );
  await scripts.locator(".script-settings > summary").click();
  await append("\n// edit applied with one click");
  assert.match(await state().textContent(), /Unapplied changes/);
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.match(await scripts.locator("output").textContent(), /Saved and enabled/);
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), false);
  await scripts.locator(".script-settings > summary").click();
  await scripts.getByLabel("Host filter (optional)", { exact: true }).fill("sandbox.example.test");
  await scripts.getByLabel("Method filter (optional)", { exact: true }).fill("PATCH");
  await scripts.getByLabel("Writable headers", { exact: true }).fill("User-Agent, user-agent");
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.activeScripts[0].manifest.prefilter.host,
    ),
    "sandbox.example.test",
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.activeScripts[0].manifest.prefilter.method,
    ),
    "PATCH",
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.scriptDirty || editor.scriptUnapplied),
    false,
    "Applied capability settings differ from the saved baseline",
  );
  assert.deepEqual(
    await page.evaluate(
      () => globalThis.__workspaceFixture.activeScripts[0].manifest.capabilities.writeHeaders,
    ),
    ["user-agent"],
    "Writable headers were not normalized as a set",
  );
  for (const width of [1440, 800]) {
    await page.setViewportSize({ width, height: width === 1440 ? 900 : 600 });
    await scripts.getByLabel("Method filter (optional)", { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-filters-${width}.png`),
    });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await scripts.locator(".script-settings > summary").click();

  assert.match(await scripts.locator(".script-scope").textContent(), /sandbox.example.test/);
  assert.match(await scripts.locator(".script-scope").textContent(), /PATCH/);
  // Optional sandbox automatically validates and describes its actual input and action.
  await scripts.locator(".script-test summary").first().click();
  await scripts
    .getByLabel("Test URL", { exact: true })
    .fill("https://sandbox.example.test:8443/users/42?ready=yes");
  await scripts.getByLabel("Method", { exact: true }).fill("PATCH");
  await scripts.getByLabel("Request headers", { exact: true }).fill("X-Test: yes");
  await scripts.getByRole("button", { name: "Test in sandbox", exact: true }).click();
  await waitIdle();
  assert.match(
    await scripts.locator(".script-test-result").textContent(),
    /Callback completed.*onRequestHead.*Action: edit.*Filters bypassed/s,
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const input = globalThis.__workspaceFixture.scriptTestInput.request;
      return {
        method: input.method,
        host: input.host,
        port: input.port,
        path: input.path,
        query: input.query,
      };
    }),
    {
      method: "PATCH",
      host: "sandbox.example.test",
      port: 8443,
      path: "/users/42",
      query: "ready=yes",
    },
  );
  await scripts.getByText("Test input and returned changes", { exact: true }).click();
  assert.match(await scripts.locator(".script-test-result pre").textContent(), /User-Agent/);
  const requestSource = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  await append(`
export function onCompleted() { return { decision: { action: "continue" } }; }
export function onFailed() { return { decision: { action: "continue" } }; }
`);
  await scripts.getByRole("combobox", { name: "Handler", exact: true }).selectOption("onCompleted");
  await scripts.getByLabel("Response status", { exact: true }).fill("202");
  await scripts.getByLabel("Response headers", { exact: true }).fill("X-Fixture: completed");
  await scripts.getByLabel("Local response", { exact: true }).check();
  await scripts.getByRole("button", { name: "Test in sandbox", exact: true }).click();
  await waitIdle();
  assert.match(
    await scripts.locator(".script-test-result pre").textContent(),
    /Handler: onCompleted.*Local response: yes.*Response fixture: 202\nX-Fixture: completed/s,
  );
  await scripts.getByRole("combobox", { name: "Handler", exact: true }).selectOption("onFailed");
  await scripts.getByLabel("Failure category (onFailed)", { exact: true }).selectOption("Route");
  await scripts.getByLabel("Local response", { exact: true }).uncheck();
  await scripts.getByRole("button", { name: "Test in sandbox", exact: true }).click();
  await waitIdle();
  const failureEvidence = await scripts.locator(".script-test-result pre").textContent();
  assert.match(failureEvidence, /Handler: onFailed.*Local response: no.*Failure category: Route/s);
  assert.doesNotMatch(failureEvidence, /Response fixture|X-Fixture/);
  await scripts.evaluate((editor, source) => editor.sourceEditor.setValue(source), requestSource);
  await scripts
    .getByRole("combobox", { name: "Handler", exact: true })
    .selectOption("onRequestHead");
  await scripts.getByLabel("Method", { exact: true }).fill("GET");
  assert.match(await scripts.locator(".script-test-result").textContent(), /Outdated/);
  await fixture({ deferScriptTest: true });
  await scripts.getByRole("button", { name: "Test in sandbox", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.scriptTestPending);
  await scripts.getByLabel("Method", { exact: true }).fill("POST");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptTest = false;
    globalThis.__workspaceFixture.finishScriptTest();
  });
  await waitIdle();
  assert.equal(
    await scripts.evaluate((editor) => editor.testOutdated),
    true,
    "Delayed test input was treated as current",
  );
  assert.match(await scripts.locator(".script-test-result pre").textContent(), /GET https:/);

  // Source/capability edits during validation must not proceed to activation.
  await fixture({ deferScriptValidation: true });
  const beforeRevision = await page.evaluate(
    () => globalThis.__workspaceFixture.activeScripts[0].manifest.revision,
  );
  await append("\n// before delayed validation");
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.scriptValidationPending);
  await append("\n// changed while validating");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptValidation = false;
    globalThis.__workspaceFixture.finishScriptValidation();
  });
  await waitIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].manifest.revision),
    beforeRevision,
  );
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), true);

  // The generation read while preparing a draft also guards validation start.
  await fixture({ changeScriptBeforeValidation: true });
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.match(await scripts.locator("output").textContent(), /workspace changed/);
  assert.match(
    await page.evaluate(() => globalThis.__workspaceFixture.savedScript.source),
    /saved concurrently in another window/,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].manifest.revision),
    beforeRevision,
  );

  // A later retry must compare against the loaded saved baseline, not silently adopt the latest generation.
  await scripts.evaluate((editor) => editor.activateScript());
  assert.equal(await scripts.evaluate((editor) => editor.conflictVisible), true);
  await scripts.getByRole("button", { name: "Compare with saved script", exact: true }).click();
  await waitIdle();
  assert.match(
    await scripts.evaluate((editor) => editor.diffModels[0].getValue()),
    /saved concurrently/,
  );
  await scripts.getByRole("button", { name: "Keep my draft", exact: true }).click();

  // A failed activation keeps both the previous live version and the edited draft.
  await fixture({ scriptActivationError: true });
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.match(await scripts.locator("output").textContent(), /Activation failed/);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].manifest.revision),
    beforeRevision,
  );
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /changed while validating/,
  );
  await fixture({ scriptActivationError: false, deferScriptActivation: true });
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.scriptActivationPending);
  await append("\n// newer than activation snapshot");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptActivation = false;
    globalThis.__workspaceFixture.finishScriptActivation();
  });
  await waitIdle();
  assert.equal(
    await scripts.evaluate((editor) => editor.scriptDirty && editor.scriptUnapplied),
    true,
    "Delayed activation marked newer edits saved",
  );
  assert.doesNotMatch(
    await page.evaluate(() => globalThis.__workspaceFixture.savedScript.source),
    /newer than activation snapshot/,
  );

  // Save draft remains separate, including Ctrl+S and edits made while the save is pending.
  await fixture({ deferScriptSave: true });
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.scriptSavePending);
  await append("\n// newer than saved snapshot");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferScriptSave = false;
    globalThis.__workspaceFixture.finishScriptSave();
  });
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), true);
  await scripts.evaluate((editor) => editor.sourceEditor.focus());
  await page.keyboard.press("Control+s");
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), false);
  assert.equal(await scripts.evaluate((editor) => editor.scriptUnapplied), true);
  assert.doesNotMatch(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].source),
    /newer than saved snapshot/,
  );

  // Comparison includes settings and stale comparisons are labelled.
  await scripts.getByRole("button", { name: "More", exact: true }).click();
  await scripts.getByRole("button", { name: "Compare with active", exact: true }).click();
  await waitIdle();
  await checkMonacoIcons(scripts.locator(".diff-editor"));
  assert.match(await scripts.locator(".script-settings-diff").textContent(), /writeHeaders/);
  await scripts.locator(".script-settings-diff > summary").click();
  await scripts.locator(".script-settings-diff pre").first().waitFor({ state: "visible" });
  await scripts.locator(".script-settings-diff > summary").click();
  await append("\n// comparison is now old");
  assert.equal(await scripts.evaluate((editor) => editor.diffStale), true);
  await scripts.getByRole("button", { name: "More", exact: true }).click();
  await scripts.getByRole("button", { name: "Compare with active", exact: true }).click();
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.diffStale), false);
  await scripts.getByRole("button", { name: "Pause script", exact: true }).click();
  await waitIdle();
  assert.equal(
    await scripts.evaluate((editor) => editor.diffStale),
    true,
    "Pausing left a comparison labelled current",
  );
  await scripts.getByRole("button", { name: "Close comparison", exact: true }).click();
  assert.equal(await scripts.evaluate((editor) => editor.diffModels.length), 0);
  assert.match(await state().textContent(), /Paused/);
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /comparison is now old/,
  );
  await scripts.getByRole("button", { name: "Save & enable", exact: true }).click();
  await waitIdle();

  // Simulate reopening a legacy workspace: active beats older/equal saved revisions;
  // a truly newer saved draft stays available without replacing the running source.
  const reopen = async () =>
    scripts.evaluate(async (editor) => {
      editor.sourceEditor.dispose();
      editor.sourceEditor = null;
      editor.draftModel.dispose();
      editor.draftModel = null;
      editor.declarationsDisposable.dispose();
      editor.declarationsDisposable = null;
      editor.scriptReady = false;
      await editor.initializeScriptEditor();
    });
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.savedScript = { ...state.savedScript, source: "// stale separately saved source" };
  });
  await reopen();
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /comparison is now old/,
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.scriptDirty || editor.scriptUnapplied),
    false,
  );
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    const active = state.activeScripts[0];
    const { source, ...unused } = active;
    state.savedScript = {
      ...active.manifest,
      source: active.source + "\n// newest saved draft",
      revision: active.manifest.revision + 1,
    };
    delete state.savedScript.sourceHash;
    delete state.savedScript.apiRevision;
  });
  await reopen();
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /newest saved draft/,
  );
  assert.equal(
    await scripts.evaluate((editor) => !editor.scriptDirty && editor.scriptUnapplied),
    true,
  );
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();

  // Type errors block publishing, but saving and editing last good JavaScript remain available.
  const lastGood = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  await append('\nconst invalid: number = "keep this TypeScript draft";');
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.match(await scripts.locator("output").textContent(), /TypeScript has/);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].source),
    lastGood,
  );
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await waitIdle();
  await scripts.getByRole("button", { name: "More", exact: true }).click();
  await scripts
    .locator("#script-actions-menu")
    .getByRole("button", { name: "Edit JavaScript", exact: true })
    .click();
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.language), "javascript");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/scripts-javascript.png") });
  assert.doesNotMatch(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /const invalid/,
  );
  assert.match(await scripts.evaluate((editor) => editor.retainedSource), /const invalid/);
  await append("\n// JavaScript-only edits must survive the round trip");
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].language),
    "javascript",
  );
  await reopen();
  assert.equal(await scripts.evaluate((editor) => editor.language), "javascript");
  await sourceSurface.getByRole("button", { name: "Expand editor", exact: true }).click();
  await scripts.getByRole("button", { name: "More", exact: true }).click();
  await scripts.getByRole("button", { name: /Return to TypeScript/ }).click();
  assert.match(await scripts.evaluate((editor) => editor.sourceEditor.getValue()), /const invalid/);
  await scripts.getByRole("button", { name: "Compare generated JavaScript", exact: true }).click();
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.returnReady), false);
  await scripts.evaluate((editor) =>
    editor.sourceEditor.setValue(
      editor.sourceEditor
        .getValue()
        .replace('const invalid: number = "keep this TypeScript draft";', "// repaired TypeScript"),
    ),
  );
  await scripts.getByRole("button", { name: "Compare generated JavaScript", exact: true }).click();
  await waitIdle();
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.diffModels.every((model) => model.getLanguageId() === "javascript"),
    ),
    true,
  );
  assert.match(
    await scripts.evaluate((editor) => editor.diffModels[0].getValue()),
    /JavaScript-only edits/,
  );
  assert.equal(
    await scripts
      .locator("editor-surface")
      .nth(1)
      .locator("dialog")
      .evaluate((dialog) => dialog.matches(":modal")),
    true,
  );
  await scripts
    .getByRole("button", { name: "Use TypeScript output", exact: true })
    .click({ trial: true });
  await scripts.getByRole("button", { name: "Close comparison", exact: true }).click();
  await append("\n// invalidate reviewed comparison");
  assert.equal(await scripts.evaluate((editor) => editor.returnReady), false);
  await scripts.getByRole("button", { name: "Compare generated JavaScript", exact: true }).click();
  await waitIdle();
  const canceledDiff = await scripts.evaluate(async (editor) => {
    const pending = editor.showJavascriptComparison(
      "const x = 1;",
      "const x = 2;",
      "Canceled comparison",
    );
    editor.closeComparison();
    return { ready: await pending, models: editor.diffModels.length, visible: editor.diffVisible };
  });
  assert.deepEqual(canceledDiff, { ready: false, models: 0, visible: false });
  await scripts.getByRole("button", { name: "Compare generated JavaScript", exact: true }).click();
  await waitIdle();
  await scripts.locator(".diff-editor").scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/scripts-return-comparison.png"),
  });
  await scripts.getByRole("button", { name: "Use TypeScript output", exact: true }).click();
  assert.equal(await scripts.evaluate((editor) => editor.language), "typescript");
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.sourceEditor.getOption(editor.monaco.editor.EditorOption.ariaLabel),
    ),
    "Traffic script TypeScript source",
  );
  await sourceSurface.getByRole("button", { name: "Return to panel", exact: true }).click();
  assert.match(await scripts.evaluate((editor) => editor.retainedSource), /JavaScript-only edits/);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].language),
    "javascript",
    "Selecting TypeScript must not publish it",
  );
  await scripts.getByRole("button", { name: "Save & apply", exact: true }).click();
  await waitIdle();
  await scripts.getByRole("button", { name: "Pause script", exact: true }).click();
  await waitIdle();
  const appliedBeforeResume = await page.evaluate(
    () => globalThis.__workspaceFixture.pausedScripts[0].javascript,
  );
  const pausedDraftSource = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  await append('\nconst blocked: number = "unsaved and not applied";');
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await waitIdle();
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__workspaceFixture.savedScript.revision >
        globalThis.__workspaceFixture.pausedScripts[0].manifest.revision,
    ),
    true,
  );
  await scripts.getByRole("button", { name: "Resume applied script", exact: true }).click();
  await waitIdle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].javascript),
    appliedBeforeResume,
  );
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /unsaved and not applied/,
  );
  await scripts.evaluate(
    (editor, source) => editor.sourceEditor.setValue(source),
    pausedDraftSource,
  );
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await waitIdle();

  // Previous JavaScript edits remain recoverable through another switch, publish and reopen.
  await scripts.evaluate((editor) => editor.editJavaScript());
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.historyEntries.some((item) => item.source.includes("JavaScript-only edits")),
    ),
    true,
  );
  await scripts.evaluate((editor) => editor.activateScript());
  await reopen();
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.sourceEditor.getOption(editor.monaco.editor.EditorOption.ariaLabel),
    ),
    "Traffic script JavaScript source",
  );
  const previousJsLabel = await scripts.evaluate(
    (editor) =>
      editor.historyEntries.find((item) => item.source.includes("JavaScript-only edits")).label,
  );
  await scripts.locator(".script-history > summary").click();
  await scripts.locator(".script-history").getByText(previousJsLabel, { exact: true }).click();
  await scripts.getByRole("button", { name: `Restore ${previousJsLabel}`, exact: true }).click();
  assert.equal(await scripts.locator(".script-history").evaluate((details) => details.open), false);
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /JavaScript-only edits/,
  );
  await scripts.evaluate((editor) => editor.activateScript());

  // Runtime availability is independent of activation and retry never applies the draft.
  const appliedJs = await page.evaluate(
    () => globalThis.__workspaceFixture.activeScripts[0].javascript,
  );
  await fixture({ scriptRuntimeError: "Fixture sandbox is unavailable" });
  await scripts.evaluate((editor) => editor.refreshRuntime());
  assert.match(await state().textContent(), /Unavailable/);
  await fixture({ retryScriptError: true });
  await scripts.getByRole("button", { name: "Retry applied script", exact: true }).click();
  await waitIdle();
  assert.match(await scripts.locator("output").textContent(), /Retry failed/);
  await fixture({ retryScriptError: false });
  await scripts.getByRole("button", { name: "Retry applied script", exact: true }).click();
  await waitIdle();
  assert.equal(await scripts.evaluate((editor) => editor.runtimeIssue), "");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.activeScripts[0].javascript),
    appliedJs,
  );

  // Conflicting saved edits remain intact until an explicit reviewed choice.
  await append("\n// MY_LOCAL_WINDOW_WORK");
  await page.evaluate(() => {
    const f = globalThis.__workspaceFixture;
    f.savedScript = {
      ...structuredClone(f.savedScript),
      revision: f.savedScript.revision + 1,
      source: f.savedScript.source + "\n// OTHER_WINDOW_WORK",
      retainedSource: (f.savedScript.retainedSource ?? "") + "\n// OTHER_WINDOW_TYPESCRIPT",
    };
    f.scriptGeneration++;
  });
  await scripts.evaluate((editor) => editor.saveScript());
  assert.match(
    await page.evaluate(() => globalThis.__workspaceFixture.savedScript.source),
    /OTHER_WINDOW_WORK/,
  );
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /MY_LOCAL_WINDOW_WORK/,
  );
  await scripts.evaluate((editor) => editor.scriptEditor.closest("editor-surface").toggle());
  await scripts.getByRole("button", { name: "Compare with saved script", exact: true }).click();
  await waitIdle();
  await scripts.locator(".script-settings-diff > summary").click();
  await scripts.locator(".script-settings-diff pre").first().waitFor({ state: "visible" });
  assert.match(
    await scripts.locator(".script-settings-diff").textContent(),
    /OTHER_WINDOW_TYPESCRIPT/,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/scripts-conflict-comparison.png"),
  });
  await scripts.getByRole("button", { name: "Load saved draft", exact: true }).click();
  await scripts.evaluate((editor) => editor.scriptEditor.closest("editor-surface").toggle());
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /OTHER_WINDOW_WORK/,
  );
  await scripts.getByRole("button", { name: "Restore my previous draft", exact: true }).click();
  assert.match(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    /MY_LOCAL_WINDOW_WORK/,
  );
  await scripts.evaluate((editor) => editor.saveScript());

  // Repairing retained TypeScript changes only authoring state, with useful diagnostics.
  await scripts.evaluate((editor) => editor.reviewTypeScript());
  await append('\nconst retainedIssue: number = "wrong";');
  await scripts.evaluate((editor) => editor.compareTypeScriptOutput());
  assert.match(await scripts.locator("output").textContent(), /Keep JavaScript/);
  assert.doesNotMatch(await scripts.evaluate((editor) => editor.sourceAlignment), /matches/);
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.typeScriptIssues.some((item) => item.message.includes("not assignable")),
    ),
    true,
  );
  await scripts.evaluate((editor) => editor.keepJavaScript());
  // JS source includes an unapplied local edit here; compare directly with the saved baseline after activation below.
  await scripts.evaluate((editor) => editor.activateScript());
  await scripts.evaluate((editor) => editor.reviewTypeScript());
  await append("\n// another retained TypeScript repair");
  assert.equal(await scripts.evaluate((editor) => editor.scriptUnapplied), false);
  assert.equal(await scripts.evaluate((editor) => editor.scriptDirty), true);
  await scripts.evaluate((editor) =>
    editor.sourceEditor.setValue(
      'export function onRequestHead() { setTimeout(() => {}, 1); void fetch("https://example.test"); return {action:"continue"}; }',
    ),
  );
  await scripts.evaluate((editor) => editor.compareTypeScriptOutput());
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.typeScriptIssues.some((item) => item.message.includes("setTimeout")),
    ),
    true,
  );
  assert.equal(
    await scripts.evaluate((editor) =>
      editor.typeScriptIssues.some((item) => item.message.includes("fetch")),
    ),
    true,
  );
  await scripts.evaluate((editor) => editor.keepJavaScript());
  await scripts.evaluate((editor) => editor.saveScript());

  // Typed parameter and body-admission settings survive saving and discarding.
  await scripts.locator(".script-settings > summary").evaluate((summary) => {
    summary.parentElement.open = true;
  });
  await scripts
    .getByLabel("Response body media types", { exact: true })
    .fill("application/json, text/plain");
  await scripts.getByLabel("Response body statuses", { exact: true }).fill("200, 202");
  await scripts.getByLabel("Change response status", { exact: true }).check();
  await scripts.getByRole("button", { name: "Add parameter", exact: true }).click();
  const parameter = scripts.locator(".script-parameter").last();
  await parameter.getByLabel("Name", { exact: true }).fill("limit");
  await parameter.getByLabel("Type", { exact: true }).selectOption("integer");
  await parameter.getByLabel("Default", { exact: true }).fill("1.5");
  assert.match(await scripts.locator(".script-scope").textContent(), /Unavailable/);
  await parameter.getByLabel("Default", { exact: true }).fill("5");
  assert.doesNotMatch(await scripts.locator(".script-scope").textContent(), /Unavailable/);
  await parameter.getByLabel("Use default", { exact: true }).uncheck();
  await parameter.getByLabel("Override", { exact: true }).fill("7");
  await scripts.getByRole("button", { name: "Save draft", exact: true }).click();
  await waitIdle();
  const settings = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.savedScript),
  );
  assert.deepEqual(settings.parameters.limit, {
    type: "integer",
    label: "limit",
    default: 5,
    value: 7,
    min: 0,
    max: 100,
  });
  assert.deepEqual(settings.responseBodyFilter, {
    contentTypes: ["application/json", "text/plain"],
    statuses: [200, 202],
    includeLocal: false,
  });
  assert.equal(settings.capabilities.writeStatus, true);
  await parameter.getByLabel("Override", { exact: true }).fill("9");
  await scripts.getByRole("button", { name: "Discard changes", exact: true }).click();
  assert.equal(
    await scripts
      .locator(".script-parameter")
      .last()
      .getByLabel("Override", { exact: true })
      .inputValue(),
    "7",
  );
  await page.setViewportSize({ width: 800, height: 600 });
  await scripts.locator(".script-parameter").last().scrollIntoViewIfNeeded();
  assert.equal(
    await scripts
      .locator(".script-parameters")
      .evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
    true,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/script-parameters.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await scripts.locator(".script-settings > summary").click();

  // More remains anchored inside an expanded editor and Escape restores focus.
  await sourceSurface.getByRole("button", { name: "Expand editor", exact: true }).click();
  await scripts.getByRole("button", { name: "More", exact: true }).click();
  const button = await scripts.getByRole("button", { name: "More", exact: true }).boundingBox();
  const menu = await scripts.locator("#script-actions-menu").boundingBox();
  assert.ok(
    menu.x < button.x + button.width &&
      menu.x + menu.width > button.x &&
      Math.abs(menu.y - button.y - button.height) < 12,
    "Script menu is detached from More",
  );
  await scripts.getByRole("button", { name: "Validate only", exact: true }).focus();
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => {
    const editor = document.querySelector("app-shell").shadowRoot.querySelector("script-editor");
    return editor.scriptMore === editor.getRootNode().activeElement;
  });
  assert.equal(
    await scripts
      .getByRole("button", { name: "More", exact: true })
      .evaluate(
        (button) =>
          button === document.activeElement || button === button.getRootNode().activeElement,
      ),
    true,
  );
  await sourceSurface.getByRole("button", { name: "Return to panel", exact: true }).click();
  await scripts.locator(".script-test summary").first().click();
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await scripts.locator(".script-state").scrollIntoViewIfNeeded();
    const bounds = await scripts
      .getByRole("button", { name: "Pause script", exact: true })
      .boundingBox();
    assert.ok(
      bounds.x >= 0 && bounds.x + bounds.width <= width && bounds.y + bounds.height < height,
      "Primary script action is clipped",
    );
    await page.screenshot({ path: resolve(root, `../../../target/ui-check/scripts-${name}.png`) });
  }
  await page.emulateMedia({ colorScheme: "dark" });
  await scripts.locator(".monaco-editor.vs-dark").first().waitFor({ state: "visible" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/scripts-dark.png") });
  await page.emulateMedia({ forcedColors: "active" });
  await scripts.locator(".monaco-editor.hc-black").first().waitFor({ state: "visible" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/scripts-forced-colors.png"),
  });
  await page.emulateMedia({ colorScheme: "light", forcedColors: "none" });
  await page.setViewportSize({ width: 1280, height: 800 });
}
