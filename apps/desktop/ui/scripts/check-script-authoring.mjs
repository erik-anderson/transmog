import assert from "node:assert/strict";
import { resolve } from "node:path";

// Shared by browser fixtures and the isolated native WebView2 harness.
export async function checkScriptApiReference(page, scripts, evidenceRoot) {
  const help = scripts.locator(".script-help-dialog");
  const source = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  if (!(await help.evaluate((dialog) => dialog.open)))
    await scripts.getByRole("button", { name: "Open API docs", exact: true }).click();
  const search = help.getByLabel("Search Script API", { exact: true });
  await search.fill("utf8Decode");
  await page.waitForFunction(() =>
    Boolean(
      document.querySelector("app-shell").shadowRoot.querySelector("script-editor")
        .apiReferenceEditor,
    ),
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.apiReferenceEditor.getValue()),
    await scripts.evaluate(
      (editor) =>
        editor.allHelpSections.find((section) => section.id === "api-reference").examples[0].code,
    ),
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.apiReferenceModel.getLanguageId()),
    "typescript",
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.apiReferenceEditor.getRawOptions().readOnly),
    true,
  );
  await scripts.evaluate((editor) => editor.checkTypeScript());
  const namespace = help.locator("details").filter({
    has: page.getByText("Full transmog:api namespace", { exact: true }),
  });
  await namespace.locator(":scope > summary").click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("script-editor")
        .apiReferenceEditor,
  );
  await search.fill("ResponseBodyEvent");
  assert.equal(
    await namespace.evaluate((details) => details.open),
    true,
    "A new API search left the matching namespace collapsed",
  );
  await page.waitForFunction(() =>
    Boolean(
      document.querySelector("app-shell").shadowRoot.querySelector("script-editor")
        .apiReferenceEditor,
    ),
  );
  await scripts.evaluate((editor) => editor.apiReferenceEditor.focus());
  await page.keyboard.press("Control+f");
  await help.locator(".find-widget.visible").waitFor({ state: "visible" });
  await page.keyboard.press("Escape");
  assert.equal(
    await help.evaluate((dialog) => dialog.open),
    true,
    "Closing Monaco Find closed the API docs dialog",
  );
  for (const term of ["utf8Decode", "ResponseBodyEvent"]) {
    await search.fill(term);
    const found = await scripts.evaluate((editor) => {
      const selection = editor.apiReferenceEditor.getSelection();
      return {
        text: editor.apiReferenceModel.getValueInRange(selection),
        visible: editor.apiReferenceEditor
          .getVisibleRanges()
          .some(
            (range) =>
              range.startLineNumber <= selection.startLineNumber &&
              range.endLineNumber >= selection.startLineNumber,
          ),
      };
    });
    assert.ok(found.text.includes(term), "API search did not select the declaration");
    assert.equal(found.visible, true, "API search left the declaration offscreen");
  }
  await scripts.evaluate((editor) => editor.apiReferenceEditor.focus());
  await page.keyboard.type("This must remain read only");
  assert.equal(
    await scripts.evaluate((editor) => editor.apiReferenceModel.getValue()),
    await scripts.evaluate(
      (editor) =>
        editor.allHelpSections.find((section) => section.id === "api-reference").examples[0].code,
    ),
  );
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 800, height: 600 },
  ]) {
    await page.setViewportSize(viewport);
    await search.fill("utf8Decode");
    assert.equal(await help.evaluate((dialog) => dialog.scrollWidth <= dialog.clientWidth), true);
    await page.screenshot({
      path: resolve(evidenceRoot, `script-api-namespace-${viewport.width}.png`),
    });
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({ path: resolve(evidenceRoot, "script-api-namespace-forced-colors.png") });
  await page.emulateMedia({ forcedColors: "none" });
  const readingPosition = await scripts.evaluate(async (editor) => {
    const declaration = editor.apiReferenceModel.findMatches(
      "export type Request =",
      false,
      false,
      false,
      null,
      true,
      1,
    )[0].range;
    editor.apiReferenceEditor.setPosition({ lineNumber: declaration.startLineNumber, column: 3 });
    editor.apiReferenceEditor.revealLineInCenter(
      declaration.startLineNumber,
      editor.monaco.editor.ScrollType.Immediate,
    );
    editor.apiReferenceEditor.focus();
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return {
      position: editor.apiReferenceEditor.getPosition(),
      firstVisibleLine: editor.apiReferenceEditor.getVisibleRanges()[0].startLineNumber,
    };
  });
  await page.keyboard.press("Escape");
  assert.equal(
    await scripts.evaluate((editor) => editor.apiReferenceEditor),
    null,
    "Closed API reference retained an editor",
  );
  assert.equal(
    await scripts.evaluate((editor) => editor.sourceEditor.getValue()),
    source,
    "API reference changed the script draft",
  );
  await scripts.getByRole("button", { name: "Open API docs", exact: true }).click();
  assert.deepEqual(
    await scripts.evaluate((editor) => editor.apiReferenceEditor.getPosition()),
    readingPosition.position,
    "Reopening the API reference lost its reading position",
  );
  await page
    .waitForFunction(
      (line) =>
        document
          .querySelector("app-shell")
          .shadowRoot.querySelector("script-editor")
          .apiReferenceEditor.getVisibleRanges()[0]?.startLineNumber === line,
      readingPosition.firstVisibleLine,
      { timeout: 5000 },
    )
    .catch(async (error) => {
      const actual = await scripts.evaluate((editor) => ({
        position: editor.apiReferenceEditor.getPosition(),
        firstVisibleLine: editor.apiReferenceEditor.getVisibleRanges()[0]?.startLineNumber,
      }));
      throw new Error(
        `API reference reading position was not restored: ${JSON.stringify({ before: readingPosition, actual })}`,
        { cause: error },
      );
    });
  await search.fill("utf8Decode");
  assert.equal(
    await scripts.evaluate((editor) => Boolean(editor.apiReferenceEditor)),
    true,
    "API reference did not reopen",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
}

export async function checkScriptAuthoringLayout(page, scripts, surface, evidenceRoot) {
  const original = await scripts.evaluate((editor) => editor.sourceEditor.getValue());
  const valid =
    'import { pass } from "transmog:api";\nexport function onRequestHead() { return pass(); }\n';
  const waitForStatus = (text) =>
    page.waitForFunction((text) => {
      const editor = document.querySelector("app-shell").shadowRoot.querySelector("script-editor");
      return editor.authoringStatus.includes(text);
    }, text);
  const bounds = () => scripts.locator('[aria-label="Traffic script source editor"]').boundingBox();
  try {
    for (const viewport of [
      { width: 1440, height: 900 },
      { width: 960, height: 720 },
      { width: 800, height: 600 },
    ]) {
      await page.setViewportSize(viewport);
      for (const expanded of [false, true]) {
        await scripts.evaluate((editor, source) => editor.sourceEditor.setValue(source), valid);
        await waitForStatus("checks passed");
        if (expanded)
          await surface.getByRole("button", { name: "Expand editor", exact: true }).click();
        await scripts.evaluate((editor) => {
          editor.sourceEditor.focus();
          editor.sourceEditor.setPosition({ lineNumber: 3, column: 1 });
        });
        const before = await bounds();
        // Use keyboard input, including partial syntax followed by type errors.
        await page.keyboard.type('const sampleNumber: number = "temporary error";');
        await waitForStatus("TypeScript has");
        const invalid = await bounds();
        assert.ok(Math.abs(invalid.y - before.y) < 1, "Type errors moved the typing surface");
        assert.equal(invalid.height, before.height, "Type errors resized the typing surface");
        assert.equal(await scripts.evaluate((editor) => editor.sourceEditor.hasTextFocus()), true);
        await page.screenshot({
          path: resolve(
            evidenceRoot,
            `script-typing-${viewport.width}-${expanded ? "expanded" : "panel"}.png`,
          ),
        });
        await page.keyboard.press("Control+a");
        await page.keyboard.insertText(valid);
        await waitForStatus("checks passed");
        assert.ok(
          Math.abs((await bounds()).y - before.y) < 1,
          "Fixing an error moved the typing surface",
        );
        if (expanded) await page.keyboard.press("Escape");
      }
    }
  } finally {
    await scripts.evaluate((editor, source) => editor.sourceEditor.setValue(source), original);
    await waitForStatus("checks passed");
    await page.setViewportSize({ width: 1440, height: 900 });
  }
}
