import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { checkMonacoIcons } from "./check-monaco-icons.mjs";

export async function waitForBodySource(inspector, expected) {
  const source = inspector.locator("body-source-viewer");
  await source.locator(".source-code-host > .monaco-editor").waitFor();
  if (expected !== undefined)
    await source.evaluate((view, text) => {
      if (view.bodySourceText !== text) throw new Error("Unexpected body preview");
    }, expected);
  return source;
}
/** Wait on the owning preview state, including an intentionally hidden message. */
export async function waitForBodyText(inspector, text) {
  await inspector.locator("body-source-viewer").waitFor({ state: "attached" });
  await inspector
    .page()
    .waitForFunction(
      ({ element, text }) =>
        element.querySelector("body-source-viewer")?.bodySourceText.includes(text),
      { element: await inspector.elementHandle(), text },
    );
}
export function bodyText(inspector) {
  return inspector.evaluate(
    (element) => element.querySelector("body-source-viewer")?.bodySourceText ?? element.bodyText,
  );
}

export async function checkBodySource(page, root) {
  const viewport = page.viewportSize();
  await page.goto("https://workspace.test/");
  const original =
    ')]}\'\n{"wide":9007199254740993,"markup":"<script>globalThis.sourceExecuted=true</script>","items":[' +
    Array.from({ length: 300 }, (_, i) => `{"id":${i},"text":"row ${i}"}`).join(",") +
    "]}";
  const formatted = JSON.stringify(JSON.parse(original.slice(5)), null, 2).replace(
    "9007199254740992",
    "9007199254740993",
  );
  await page.evaluate(
    ({ original, formatted }) => {
      const state = globalThis.__workspaceFixture;
      state.bodyOverride = {
        mediaType: "application/json",
        contentCodings: [],
        retainedBytes: original.length,
        observedBytes: original.length,
      };
      const invoke = window.__TAURI_INTERNALS__.invoke;
      state.sourceInvoke = invoke;
      window.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const result = await invoke(command, args);
        if (command !== "inspect_body") return result;
        const pretty = args.request.representation !== "original-text";
        return {
          ...result,
          representation: pretty ? "formatted-json" : "original-text",
          display: pretty ? formatted : original,
          originalText: original,
          displayBytes: original.length,
          warning: pretty
            ? "XSSI prefix omitted from this formatted view; the original body is unchanged."
            : null,
        };
      };
    },
    { original, formatted },
  );
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  try {
    let releaseStyles, requestedStyles;
    const requested = new Promise((resolve) => {
      requestedStyles = resolve;
    });
    const released = new Promise((resolve) => {
      releaseStyles = resolve;
    });
    await page.route("**/monaco.css", async (route) => {
      requestedStyles();
      await released;
      await route.fallback();
    });
    await page.locator('tr[data-session-id="first"]').click();
    await requested;
    const fallback = inspector.locator(".source-fallback");
    await fallback.focus();
    const reading = await fallback.evaluate((text) => {
      text.scrollTop = 140;
      const range = document.createRange();
      range.setStart(text.firstChild, 10);
      range.setEnd(text.firstChild, 25);
      const selection = text.getRootNode().getSelection?.() ?? document.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      return { top: text.scrollTop, text: selection.toString() };
    });
    assert.ok(reading.top > 0, "Plain-text fallback does not scroll in its constrained pane");
    releaseStyles();
    await page.waitForTimeout(100);
    assert.equal(
      await fallback.isVisible(),
      true,
      "Late syntax hydration removed focused fallback",
    );
    assert.deepEqual(
      await fallback.evaluate((text) => ({
        top: text.scrollTop,
        text: (text.getRootNode().getSelection?.() ?? document.getSelection()).toString(),
      })),
      reading,
    );
    await fallback.evaluate((text) =>
      (text.getRootNode().getSelection?.() ?? document.getSelection()).removeAllRanges(),
    );
    await inspector.getByLabel("Body viewer", { exact: true }).focus();
    const source = await waitForBodySource(inspector, formatted);
    await checkMonacoIcons(source.locator(".source-code-host"));
    await page.unroute("**/monaco.css");
    await source.evaluate((v) => v.editor.setScrollTop(0));
    assert.equal(await source.evaluate((v) => v.model.getLanguageId()), "json");
    assert.equal(await source.evaluate((v) => v.editor.getRawOptions().readOnly), true);
    await source.evaluate((v) => {
      v.editor.focus();
      v.editor.setSelection({ startLineNumber: 2, startColumn: 3, endLineNumber: 2, endColumn: 9 });
    });
    const selected = await source.evaluate((v) => v.model.getValueInRange(v.editor.getSelection()));
    // A real right-button mousedown inside the model selection must keep it.
    const point = await source.evaluate((v) => {
      const rect = v.editorHost.getBoundingClientRect(),
        p = v.editor.getScrolledVisiblePosition({ lineNumber: 2, column: 5 });
      return { x: rect.left + p.left + 1, y: rect.top + p.top + p.height / 2 };
    });
    await page.mouse.click(point.x, point.y, { button: "right" });
    assert.equal((await menu.getByRole("menuitem").allTextContents())[0], "Copy");
    await menu.getByRole("menuitem", { name: "Copy", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), selected);
    assert.equal(
      await source.evaluate((v) => v.model.getValueInRange(v.editor.getSelection())),
      selected,
    );
    await page.keyboard.press("Shift+F10");
    await menu.getByRole("menuitem", { name: "Copy original text preview", exact: true }).click();
    assert.equal(
      (await page.evaluate(() => navigator.clipboard.readText())).replaceAll("\r\n", "\n"),
      original,
    );
    await source.evaluate(async (v) => {
      await v.editor.getAction("editor.foldAll").run();
      v.editor.focus();
    });
    await page.keyboard.press("Shift+F10");
    await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
    assert.equal(
      (await page.evaluate(() => navigator.clipboard.readText())).replaceAll("\r\n", "\n"),
      formatted,
      "Folded/offscreen text was omitted",
    );
    await source.evaluate((v) => {
      v.editor.setPosition({ lineNumber: 1, column: 1 });
      v.editor.focus();
    });
    await page.evaluate(() => navigator.clipboard.writeText("empty source copy sentinel"));
    await page.keyboard.press("Control+c");
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      "empty source copy sentinel",
      "Empty selection copied a line",
    );
    await page.keyboard.press("Shift+F10");
    assert.equal(await menu.getByRole("menuitem", { name: "Copy", exact: true }).count(), 0);
    await page.keyboard.press("Escape");
    assert.equal(await source.evaluate((v) => v.editor.hasTextFocus()), true);
    await source.evaluate((v) =>
      v.editor.setSelection({ startLineNumber: 2, startColumn: 3, endLineNumber: 2, endColumn: 9 }),
    );
    await page.keyboard.press("Control+c");
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      selected,
      "Keyboard copy did not use plain model source",
    );
    // The host menu owns its caret anchor, rather than Monaco's hidden textarea.
    await inspector.locator(".body-options > summary").click();
    for (const language of ["html", "xml", "css", "javascript", "plaintext", "auto"]) {
      await inspector.getByLabel("Source language", { exact: true }).selectOption(language);
      await inspector.locator(".body-options > summary").click();
      await waitForBodySource(inspector);
      assert.equal(
        await source.evaluate((v) => v.model.getLanguageId()),
        language === "auto" ? "json" : language,
      );
      await inspector.locator(".body-options > summary").click();
    }
    await inspector.getByLabel("Wrap", { exact: true }).uncheck();
    await inspector.locator(".body-options > summary").click();
    await waitForBodySource(inspector);
    assert.equal(await source.evaluate((v) => v.editor.getRawOptions().wordWrap), "off");
    await inspector.locator(".body-options > summary").click();
    await inspector.getByRole("button", { name: "Find", exact: true }).click();
    await waitForBodySource(inspector);
    await source.locator(".find-widget.visible").waitFor();
    const findInput = source.locator(".find-widget").getByRole("textbox").first();
    await findInput.fill("Find input selection");
    await findInput.press("Control+a");
    await findInput.press("Control+c");
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      "Find input selection",
      "Read-only model adapter intercepted Find's input copying",
    );
    await page.keyboard.press("Escape");
    // An unrelated detail update preserves the actual model and view state.
    const before = await source.evaluate(async (v) => {
      await v.editor.getAction("editor.unfoldAll").run();
      v.editor.setScrollTop(300);
      v.editor.setSelection({
        startLineNumber: 30,
        startColumn: 2,
        endLineNumber: 31,
        endColumn: 3,
      });
      globalThis.__sourceModel = v.model;
      return { top: v.editor.getScrollTop(), selection: v.editor.getSelection().toString() };
    });
    await page.evaluate(() => {
      const w = document.querySelector("app-shell").traffic;
      w.applyDetail(structuredClone(w.selectedDetail));
    });
    assert.deepEqual(
      await source.evaluate((v) => ({
        top: v.editor.getScrollTop(),
        selection: v.editor.getSelection().toString(),
      })),
      before,
    );
    assert.equal(await source.evaluate((v) => v.model === globalThis.__sourceModel), true);
    await inspector.getByRole("button", { name: "Headers", exact: true }).click();
    await source.evaluate((v) => {
      if (v.model !== null || v.editor !== null)
        throw new Error("Hidden viewer retained an editor");
    });
    await inspector.getByRole("button", { name: "Body", exact: true }).click();
    await waitForBodySource(inspector, formatted);
    assert.deepEqual(
      await source.evaluate((v) => ({
        top: v.editor.getScrollTop(),
        selection: v.editor.getSelection().toString(),
      })),
      before,
    );
    assert.equal(await page.evaluate(() => globalThis.sourceExecuted), undefined);
    await source.evaluate(async (v) => {
      v.editor.setPosition({ lineNumber: 1, column: 1 });
      v.editor.setScrollTop(0);
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    });
    const expanded = source.locator(".margin-view-overlays .codicon-folding-expanded").first();
    await expanded.hover();
    await page.waitForFunction(
      (icon) => getComputedStyle(icon).opacity === "1",
      await expanded.elementHandle(),
    );
    const downArrow = await expanded.evaluate((icon) => getComputedStyle(icon, "::before").content);
    await page.screenshot({ path: resolve(root, "../../../target/ui-check/monaco-icons.png") });
    await expanded.click();
    const collapsed = source.locator(".margin-view-overlays .codicon-folding-collapsed").first();
    await collapsed.waitFor();
    assert.notEqual(
      await collapsed.evaluate((icon) => getComputedStyle(icon, "::before").content),
      downArrow,
      "Collapsing a range did not change its arrow",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/monaco-icons-collapsed.png"),
    });
    await collapsed.click();
    await collapsed.waitFor({ state: "detached" });
    await expanded.waitFor();
    await mkdir(resolve(root, "../../../target/body-source"), { recursive: true });
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      const switcher = page.getByRole("button", { name: "Response", exact: true });
      if (await switcher.isVisible()) await switcher.click();
      await waitForBodySource(inspector, formatted);
      assert.equal(await source.locator(".source-code-host").isVisible(), true);
      await source.evaluate((v) => {
        v.editor.setPosition({ lineNumber: 1, column: 1 });
        v.editor.setScrollTop(0);
        v.editor.focus();
      });
      for (let line = 0; line < 5; line++) await page.keyboard.press("ArrowDown");
      const caret = await source.evaluate((v) => {
        const position = v.editor.getScrolledVisiblePosition(v.editor.getPosition()),
          host = v.editorHost.getBoundingClientRect(),
          viewport = v.closest(".body-scroll").getBoundingClientRect();
        return {
          top: host.top + position.top,
          bottom: host.top + position.top + position.height,
          visibleTop: Math.max(host.top, viewport.top),
          visibleBottom: Math.min(host.bottom, viewport.bottom, innerHeight),
          height: host.height,
        };
      });
      assert.ok(
        caret.top >= caret.visibleTop - 1 && caret.bottom <= caret.visibleBottom + 1,
        `Keyboard source caret clipped at ${width}: ${JSON.stringify(caret)}`,
      );
      await page.screenshot({
        path: resolve(root, `../../../target/body-source/browser-${width}.png`),
      });
    }
    for (const [theme, media, className] of [
      ["dark", { colorScheme: "dark" }, "vs-dark"],
      ["forced-colors", { forcedColors: "active" }, "hc-black"],
    ]) {
      await page.emulateMedia(media);
      await source.locator(`.monaco-editor.${className}`).waitFor();
      await checkMonacoIcons(source.locator(".source-code-host"));
      const arrow = source.locator(".margin-view-overlays .codicon-folding-expanded").first();
      await arrow.hover();
      await page.waitForFunction(
        (icon) => getComputedStyle(icon).opacity === "1",
        await arrow.elementHandle(),
      );
      await page.screenshot({
        path: resolve(root, `../../../target/body-source/browser-icons-${theme}.png`),
      });
    }
  } finally {
    await page.emulateMedia({ colorScheme: "light", forcedColors: "none" });
    await page.setViewportSize(viewport);
    await page.goto("https://workspace.test/");
  }
}

async function checkExplicitSourceIntent(page) {
  for (const intent of ["reveal", "find"]) {
    for (const cancel of [false, true]) {
      let releaseStyles, requestedStyles;
      const requested = new Promise((resolve) => {
          requestedStyles = resolve;
        }),
        released = new Promise((resolve) => {
          releaseStyles = resolve;
        });
      await page.route("**/monaco.css", async (route) => {
        requestedStyles();
        await released;
        await route.fallback();
      });
      try {
        await page.goto("https://workspace.test/");
        await page.locator('tr[data-session-id="first"]').click();
        await requested;
        const inspector = page.locator('traffic-workspace message-inspector[side="response"]'),
          source = inspector.locator("body-source-viewer"),
          fallback = source.locator(".source-fallback"),
          options = inspector.locator(".body-options > summary"),
          cancelControl = inspector.getByRole("button", { name: "Source", exact: true }),
          text = '{"first":"reading","last":"target"}';
        await inspector.evaluate((view, text) => {
          view.bodySource = {
            key: "explicit-intent",
            text,
            originalText: text,
            mediaType: "application/json",
            formatted: false,
            truncated: false,
          };
        }, text);
        await fallback.focus();
        await fallback.evaluate((element) => {
          const start = element.textContent.indexOf("reading"),
            range = document.createRange(),
            selection = element.getRootNode().getSelection?.() ?? document.getSelection();
          range.setStart(element.firstChild, start);
          range.setEnd(element.firstChild, start + "reading".length);
          selection.removeAllRanges();
          selection.addRange(range);
        });
        if (intent === "reveal") {
          await inspector.getByRole("button", { name: "Fields", exact: true }).click();
          const fields = source.locator("extension-visualizer");
          await fields.getByRole("button", { name: "Expand JSON", exact: true }).click();
          await fields
            .getByRole("treeitem")
            .filter({ has: page.locator(".visualizer-field", { hasText: /^last$/ }) })
            .click();
          await fields.getByRole("button", { name: "Show in source", exact: true }).click();
        } else {
          await options.click();
          await inspector.getByRole("button", { name: "Find", exact: true }).click();
        }
        assert.equal(
          await source.evaluate((view) =>
            (view.getRootNode().getSelection?.() ?? document.getSelection()).toString(),
          ),
          "reading",
        );
        assert.equal(
          await source.evaluate(
            (view, intent) => (intent === "reveal" ? !!view.pendingReveal : view.pendingFind),
            intent,
          ),
          true,
        );
        if (cancel) {
          // A later real pointer action supersedes the outstanding explicit intent.
          await cancelControl.click();
        }
        releaseStyles();
        if (cancel) {
          await page.waitForFunction(
            (view) => !view.loading && view.deferred,
            await source.elementHandle(),
          );
          assert.equal(await fallback.isVisible(), true);
          assert.deepEqual(
            await source.evaluate((view) => ({
              editor: !!view.editor,
              pendingFind: view.pendingFind,
              pendingReveal: view.pendingReveal,
              selection: (
                view.getRootNode().getSelection?.() ?? document.getSelection()
              ).toString(),
            })),
            { editor: false, pendingFind: false, pendingReveal: null, selection: "reading" },
          );
          assert.equal(
            await cancelControl.evaluate(
              (element) => element.getRootNode().activeElement === element,
            ),
            true,
          );
          await source.evaluate((view) =>
            (view.getRootNode().getSelection?.() ?? document.getSelection()).removeAllRanges(),
          );
          await waitForBodySource(inspector, text);
          assert.equal(await source.evaluate((view) => view.editor.getSelection().isEmpty()), true);
          assert.equal(await source.locator(".find-widget.visible").count(), 0);
          assert.equal(
            await cancelControl.evaluate(
              (element) => element.getRootNode().activeElement === element,
            ),
            true,
          );
        } else {
          await waitForBodySource(inspector, text);
          if (intent === "reveal") {
            assert.equal(await source.evaluate((view) => view.editor.hasTextFocus()), true);
            assert.equal(
              await source.evaluate((view) =>
                view.model.getValueInRange(view.editor.getSelection()),
              ),
              '"target"',
            );
          } else {
            await source.locator(".find-widget.visible").waitFor();
            const input = source.locator(".find-widget").getByRole("textbox").first();
            assert.equal(
              await input.evaluate((element) => element.getRootNode().activeElement === element),
              true,
            );
          }
          assert.deepEqual(
            await source.evaluate((view) => ({
              find: view.pendingFind,
              reveal: view.pendingReveal,
              deferred: view.deferred,
            })),
            { find: false, reveal: null, deferred: false },
          );
          // Returning from Fields must not revive the superseded fallback reading.
          await inspector.getByRole("button", { name: "Fields", exact: true }).click();
          await inspector.getByRole("button", { name: "Source", exact: true }).click();
          await waitForBodySource(inspector, text);
        }
      } finally {
        releaseStyles();
        await page.unroute("**/monaco.css");
      }
    }
  }
}

async function checkFailedSourceReveal(page) {
  // Keep the harness's error reporting intact except for this deliberate CSS failure.
  const consoleListeners = page.listeners("console"),
    forwardConsole = (message) => {
      if (
        message.type() === "error" &&
        message.location().url === "https://workspace.test/monaco.css" &&
        /^Failed to load resource: net::ERR_FAILED$/.test(message.text())
      )
        return;
      for (const listener of consoleListeners) listener(message);
    };
  for (const listener of consoleListeners) page.off("console", listener);
  page.on("console", forwardConsole);
  let requestedStyles,
    releaseStyles,
    holdStyles = false;
  await page.route("**/monaco.css", async (route) => {
    if (holdStyles) {
      requestedStyles();
      await new Promise((resolve) => {
        releaseStyles = resolve;
      });
    }
    await route.abort("failed");
  });
  try {
    await page.goto("https://workspace.test/");
    await page.locator('tr[data-session-id="first"]').click();
    const inspector = page.locator('traffic-workspace message-inspector[side="response"]'),
      source = inspector.locator("body-source-viewer"),
      fallback = source.locator(".source-fallback"),
      fields = source.locator("extension-visualizer"),
      menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
    await source
      .locator(".source-status")
      .filter({ hasText: "Syntax preview unavailable" })
      .waitFor();
    const text =
      '{"fixture":true,"":""," \\t":" \\t","\\\"\\\" (empty name)":"\\\"\\\" (empty string)"}';
    await inspector.evaluate((view, text) => {
      view.bodySource = {
        key: "failed-json",
        text,
        originalText: text,
        mediaType: "application/json",
        formatted: false,
        truncated: false,
      };
    }, text);
    await page.waitForFunction(
      (view) => !view.loading && view.sourceStatus.includes("Syntax preview unavailable"),
      await source.elementHandle(),
    );
    await inspector.getByRole("button", { name: "Fields", exact: true }).click();
    await fields.getByRole("button", { name: "Expand JSON", exact: true }).click();
    const empty = fields.getByRole("treeitem", {
      name: '"" (empty name) "" (empty string)',
      exact: true,
    });
    await empty.waitFor();
    assert.equal(await empty.locator(".visualizer-field-value").isVisible(), true);
    const copy = async (row, label) => {
      await row.click({ button: "right" });
      await menu.getByRole("menuitem", { name: label, exact: true }).click();
      return page.evaluate(() => navigator.clipboard.readText());
    };
    await page.evaluate(() => navigator.clipboard.writeText("empty JSON copy sentinel"));
    assert.equal(await copy(empty, "Copy name"), "");
    await page.evaluate(() => navigator.clipboard.writeText("empty JSON copy sentinel"));
    assert.equal(await copy(empty, "Copy value"), "");
    assert.equal(await copy(empty, "Copy name and value"), ": ");
    assert.equal(await copy(empty, "Copy as JSON"), '""');
    assert.equal(await copy(empty, "Copy JSON Pointer"), "/");
    const whitespace = fields
      .getByRole("treeitem")
      .filter({ has: page.locator(".visualizer-field", { hasText: "whitespace name" }) });
    assert.equal(await copy(whitespace, "Copy name and value"), " \\t:  \\t");
    const marker = fields
      .getByRole("treeitem")
      .filter({ has: page.locator(".visualizer-field", { hasText: /^"\\"\\" \(empty name\)"$/ }) });
    assert.equal(await copy(marker, "Copy value"), '"" (empty string)');
    await empty.click();
    assert.equal(
      await fields.locator(".visualizer-node-heading strong").textContent(),
      '"" (empty name)',
    );
    assert.equal(await fields.locator(".visualizer-value pre").textContent(), '"" (empty string)');
    await fields.getByLabel("Structure view", { exact: true }).selectOption("table");
    await fields
      .getByRole("treeitem")
      .filter({ has: page.locator(".visualizer-field", { hasText: /^JSON$/ }) })
      .click();
    await fields.getByLabel("Structure view", { exact: true }).selectOption("table");
    const tableEmpty = fields
      .locator("tbody tr")
      .filter({ has: page.locator("th button", { hasText: /^"" \(empty name\)$/ }) });
    assert.equal(await tableEmpty.locator("td").first().textContent(), '"" (empty string)');
    assert.equal(await copy(tableEmpty.locator("td").first(), "Copy as JSON"), '""');
    const fixture = fields
      .getByRole("treeitem")
      .filter({ has: page.locator(".visualizer-field", { hasText: /^fixture$/ }) });
    await fixture.click();
    await fields.getByRole("button", { name: "Show in source", exact: true }).click();
    await page.waitForFunction(
      (view) => {
        const root = view.getRootNode(),
          selection = root.getSelection?.() ?? document.getSelection();
        return (
          !view.loading &&
          view.pendingReveal === null &&
          root.activeElement === view.fallback &&
          selection.toString() === "true"
        );
      },
      await source.elementHandle(),
    );
    assert.equal(await fallback.isVisible(), true);
    assert.equal(await source.evaluate((view) => view.model), null);
    await source.evaluate((view) =>
      (view.getRootNode().getSelection?.() ?? document.getSelection()).removeAllRanges(),
    );
    await inspector.getByRole("button", { name: "Fields", exact: true }).click();
    await fields.getByRole("button", { name: "Expand JSON", exact: true }).click();
    await empty.click();
    await fields.getByRole("button", { name: "Show in source", exact: true }).click();
    await page.waitForFunction(
      (view) => {
        const root = view.getRootNode(),
          selection = root.getSelection?.() ?? document.getSelection();
        return (
          !view.loading &&
          view.pendingReveal === null &&
          root.activeElement === view.fallback &&
          selection.toString() === '""'
        );
      },
      await source.elementHandle(),
    );
    await page.keyboard.press("Shift+F10");
    await menu.getByRole("menuitem", { name: "Copy JSON Pointer", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "/");
    await source.evaluate((view) =>
      (view.getRootNode().getSelection?.() ?? document.getSelection()).removeAllRanges(),
    );
    await inspector.getByRole("button", { name: "Fields", exact: true }).click();
    await fields.getByRole("button", { name: "Expand JSON", exact: true }).click();
    await fields
      .getByRole("treeitem")
      .filter({ has: page.locator(".visualizer-field", { hasText: /^fixture$/ }) })
      .click();
    holdStyles = true;
    const requested = new Promise((resolve) => {
      requestedStyles = resolve;
    });
    await fields.getByRole("button", { name: "Show in source", exact: true }).click();
    await requested;
    // A later user interaction cancels reveal before the failed load settles.
    const options = inspector.locator(".body-options > summary");
    await options.click();
    releaseStyles();
    await page.waitForFunction((view) => !view.loading, await source.elementHandle());
    assert.equal(await source.evaluate((view) => view.pendingReveal), null);
    assert.equal(
      await options.evaluate((element) => element.getRootNode().activeElement === element),
      true,
    );
    assert.equal(
      await source.evaluate((view) =>
        (view.getRootNode().getSelection?.() ?? document.getSelection()).toString(),
      ),
      "",
    );
    // The failed link was removed, so explicit Find can retry with a fresh link.
    await page.unroute("**/monaco.css");
    await inspector.getByRole("button", { name: "Find", exact: true }).click();
    await waitForBodySource(inspector, text);
    await source.locator(".find-widget.visible").waitFor();
    assert.equal(await source.evaluate((view) => view.sourceStatus), "");
    await page.keyboard.press("Escape");
  } finally {
    releaseStyles?.();
    await page.unroute("**/monaco.css");
    page.off("console", forwardConsole);
    for (const listener of consoleListeners) page.on("console", listener);
  }
}

export async function checkJsonFields(page) {
  await checkExplicitSourceIntent(page);
  await checkFailedSourceReveal(page);
  await page.goto("https://workspace.test/");
  const original =
    ')]}\'\n{"wide":9007199254740993,"rate":1e+02,"dup":1,"dup":2,"a/b~":{"text":"line\\nvalue","nil":null,"list":[true,{},[]]}}';
  await page.evaluate((original) => {
    const state = globalThis.__workspaceFixture,
      invoke = window.__TAURI_INTERNALS__.invoke;
    state.bodyOverride = {
      mediaType: "application/json",
      contentCodings: [],
      retainedBytes: original.length,
      observedBytes: original.length,
    };
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command !== "inspect_body") return result;
      const formatted = args.request.representation !== "original-text";
      return {
        ...result,
        representation: formatted ? "formatted-json" : "original-text",
        display: formatted ? JSON.stringify(JSON.parse(original.slice(5)), null, 2) : original,
        originalText: formatted ? original : null,
        displayBytes: original.length,
      };
    };
  }, original);
  await page.locator('tr[data-session-id="first"]').click();
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const source = await waitForBodySource(inspector);
  const displayed = await source.evaluate((v) => v.bodySourceText);
  assert.equal(
    await inspector
      .getByRole("button", { name: "Source", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  assert.match(displayed, /"wide": 9007199254740993/);
  assert.match(displayed, /"rate": 1e\+02/);
  assert.match(displayed, /"dup": 1,\n  "dup": 2/);
  await inspector.getByRole("button", { name: "Fields", exact: true }).click();
  assert.equal(
    await inspector
      .getByRole("button", { name: "Fields", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await inspector
      .getByRole("button", { name: "Fields", exact: true })
      .evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  const fields = source.locator("extension-visualizer");
  const row = (name) =>
    fields.getByRole("treeitem").filter({
      has: page.locator(".visualizer-field", {
        hasText: new RegExp("^" + name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "$"),
      }),
    });
  await row("JSON").getByRole("button", { name: "Expand JSON", exact: true }).click();
  const menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  const copy = async (target, label) => {
    await target.click({ button: "right" });
    await menu.getByRole("menuitem", { name: label, exact: true }).click();
    return (await page.evaluate(() => navigator.clipboard.readText())).replaceAll("\r\n", "\n");
  };
  await row("rate").click();
  const selection = await fields.getByRole("tree").getAttribute("aria-activedescendant");
  assert.equal(await copy(row("wide"), "Copy value"), "9007199254740993");
  assert.equal(await fields.getByRole("tree").getAttribute("aria-activedescendant"), selection);
  assert.equal(await copy(row("rate"), "Copy as JSON"), "1e+02");
  await row("dup").nth(1).click({ button: "right" });
  assert.equal(
    await menu.getByRole("menuitem", { name: "Copy JSON Pointer", exact: true }).count(),
    0,
  );
  await page.keyboard.press("Escape");
  assert.equal(await copy(row("dup").nth(1), "Copy value"), "2");
  assert.match(
    await copy(row("a/b~"), "Copy subtree"),
    /^a\/b~\n  text: line\\nvalue\n  nil: null\n  list\n    \[0\]: true/,
  );
  assert.equal(
    await copy(row("a/b~"), "Copy as JSON"),
    '{"text":"line\\nvalue","nil":null,"list":[true,{},[]]}',
  );
  await row("a/b~").getByRole("button", { name: "Expand a/b~", exact: true }).click();
  assert.equal(await copy(row("text"), "Copy JSON Pointer"), "/a~1b~0/text");
  assert.equal(await copy(row("text"), "Copy name and value"), "text: line\\nvalue");
  await row("wide").click();
  await fields.getByRole("button", { name: "Show in source", exact: true }).click();
  await waitForBodySource(inspector);
  assert.equal(
    await source.evaluate((v) => v.model.getValueInRange(v.editor.getSelection())),
    "9007199254740993",
  );
  assert.equal(await source.evaluate((v) => v.sourceSnapshot), null);
  assert.equal(await source.evaluate((v) => v.editor.hasTextFocus()), true);
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy as JSON", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "9007199254740993");
  await inspector.getByLabel("Body viewer", { exact: true }).selectOption("original-text");
  await waitForBodySource(inspector, original);
  await source.evaluate((v) => {
    const node = [...v.displayDocument.nodes.values()].find((node) => node.name === "rate");
    v.editor.setPosition(v.model.getPositionAt(node.start));
    v.editor.focus();
  });
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "1e+02");
  // An unmapped mouse target must not borrow the keyboard caret's JSON field.
  const pointerJson = '{"a":1,"b":2}';
  await inspector.evaluate((view, text) => {
    view.bodySource = {
      key: "unmapped-pointer",
      mediaType: "application/json",
      text,
      originalText: text,
      formatted: false,
      truncated: false,
    };
  }, pointerJson);
  await waitForBodySource(inspector, pointerJson);
  const unmapped = await source.evaluate((view) => {
    view.editor.setPosition({ lineNumber: 1, column: 6 });
    view.editor.focus();
    const ruler = view.editorHost
      .querySelector(".decorationsOverviewRuler")
      .getBoundingClientRect();
    const point = { x: ruler.right - 2, y: ruler.top + Math.min(20, ruler.height / 2) };
    const target = view.editor.getTargetAtClientPoint(point.x, point.y);
    return {
      ...point,
      position: target?.position ?? null,
      caret: view.editor.getPosition(),
      selection: view.editor.getSelection().toString(),
    };
  });
  assert.equal(
    unmapped.position,
    null,
    "Overview-ruler fixture unexpectedly mapped to source text",
  );
  await page.mouse.click(unmapped.x, unmapped.y, { button: "right" });
  await menu.waitFor({ state: "visible" });
  assert.deepEqual(
    await menu.getByRole("menuitem").allTextContents(),
    ["Copy body preview"],
    "Unmapped pointer offered semantic actions for the unrelated caret",
  );
  assert.deepEqual(
    await source.evaluate((view) => ({
      caret: view.editor.getPosition(),
      selection: view.editor.getSelection().toString(),
    })),
    { caret: unmapped.caret, selection: unmapped.selection },
    "Overview-ruler context changed the source caret or selection",
  );
  await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), pointerJson);
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "1");
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy JSON Pointer", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "/a");
  const validPointer = await source.evaluate((view) => {
    const host = view.editorHost.getBoundingClientRect(),
      position = view.editor.getScrolledVisiblePosition({ lineNumber: 1, column: 12 });
    return { x: host.left + position.left + 1, y: host.top + position.top + position.height / 2 };
  });
  await page.mouse.click(validPointer.x, validPointer.y, { button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    "2",
    "Mapped pointer lost its own field semantics",
  );
  // Choosing JSON for a plain-text body maps the already-mounted model immediately.
  const plain = '{\r\n "a/b~": {"wide": 9007199254740993}\r\n}';
  await inspector.evaluate((i, plain) => {
    i.bodySourceLanguage = "plaintext";
    i.bodySource = {
      ...i.bodySource,
      key: "plain",
      mediaType: "text/plain",
      text: plain,
      originalText: plain,
      formatted: false,
    };
  }, plain);
  await waitForBodySource(inspector, plain);
  assert.equal(await source.evaluate((v) => v.displayDocument), null);
  await inspector.locator(".body-options > summary").click();
  await inspector.getByLabel("Source language", { exact: true }).selectOption("json");
  await inspector.locator(".body-options > summary").click();
  await waitForBodySource(inspector);
  await source.evaluate((v) => {
    const node = [...v.displayDocument.nodes.values()].find((node) => node.name === "wide");
    v.editor.setPosition(v.model.getPositionAt(node.start));
    v.editor.focus();
  });
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy JSON Pointer", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "/a~1b~0/wide");
  for (const prefix of [")]}'\n", ")]}',\n"]) {
    const text = prefix + '{\r\n  "wide": 9007199254740993,\r\n  "next": 2\r\n}';
    await inspector.evaluate((i, text) => {
      i.bodySource = {
        ...i.bodySource,
        key: text,
        mediaType: "application/json",
        originalText: text,
        text,
        formatted: false,
      };
    }, text);
    await waitForBodySource(inspector, text);
    await source.evaluate((v) => {
      const node = [...v.displayDocument.nodes.values()].find((node) => node.name === "wide");
      v.editor.setPosition(v.model.getPositionAt(node.start));
      v.editor.focus();
    });
    await page.keyboard.press("Shift+F10");
    await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
    assert.equal(
      await page.evaluate(() => navigator.clipboard.readText()),
      "9007199254740993",
      "CRLF model normalization lost an exact recognized XSSI prefix mapping",
    );
  }
  const lone = '{"\\ud800":{"bad":"a\\udc00b","good":"😀"}}';
  await inspector.evaluate((i, text) => {
    i.bodySource = { ...i.bodySource, key: text, originalText: text, text, formatted: false };
  }, lone);
  await waitForBodySource(inspector, lone);
  await source.evaluate((v) => {
    const node = [...v.displayDocument.nodes.values()].find((node) => node.name === "bad");
    v.editor.setPosition(v.model.getPositionAt(node.start));
    v.editor.focus();
  });
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "a\\udc00b");
  await source.evaluate((v) => {
    const node = v.displayDocument.root.children[0];
    v.editor.setPosition(v.model.getPositionAt(node.start));
    v.editor.focus();
  });
  await page.keyboard.press("Shift+F10");
  assert.equal(
    await menu.getByRole("menuitem", { name: "Copy JSON Pointer", exact: true }).count(),
    0,
  );
  await page.keyboard.press("Escape");
  // Budget/incomplete states keep original text and never silently reuse lossy native formatting.
  for (const [text, truncated, expected] of [
    ["[".repeat(34) + "0" + "]".repeat(34), false, /structured preview limit/],
    ['{"unfinished":', true, /incomplete preview/],
  ]) {
    await inspector.evaluate(
      (i, { text, truncated }) => {
        i.bodySource = {
          ...i.bodySource,
          key: text,
          originalText: text,
          text: "{}",
          formatted: true,
          truncated,
        };
      },
      { text, truncated },
    );
    await waitForBodySource(inspector, text);
    assert.match(await source.locator(".source-status").textContent(), expected);
    assert.equal(await inspector.getByRole("button", { name: "Fields", exact: true }).count(), 0);
    await source.evaluate((v) => v.editor.focus());
    await page.keyboard.press("Shift+F10");
    await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), text);
  }
  await inspector.evaluate((i) => i.inspectBody());
  await waitForBodySource(inspector);
  await inspector.getByRole("button", { name: "Fields", exact: true }).waitFor();
  for (const representation of ["bytes", "metadata"]) {
    await inspector.getByLabel("Body viewer", { exact: true }).selectOption(representation);
    assert.equal(
      await inspector.getByRole("group", { name: "JSON body view", exact: true }).count(),
      0,
    );
    await inspector.getByLabel("Body viewer", { exact: true }).selectOption("original-text");
    await waitForBodySource(inspector);
  }
  await inspector.evaluate((i) => {
    i.detail = null;
  });
  assert.equal(
    await inspector.getByRole("group", { name: "JSON body view", exact: true }).count(),
    0,
  );
  await page.goto("https://workspace.test/");
}

export async function checkBodySourceBounds(page) {
  await page.goto("https://workspace.test/");
  await page.locator('tr[data-session-id="first"]').click();
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const source = await waitForBodySource(inspector);
  const baseline = await source.evaluate((v) => v.monaco.editor.getModels().length);
  const measurements = [];
  for (const bytes of [256 * 1024, 1024 * 1024, 16 * 1024 * 1024]) {
    const start = Date.now();
    await inspector.evaluate((i, bytes) => {
      i.bodySourceWrap = false;
      i.bodySource = {
        key: String(bytes),
        mediaType: "text/plain",
        text: "x".repeat(bytes),
        originalText: null,
        formatted: false,
        truncated: false,
      };
    }, bytes);
    if (bytes <= 1024 * 1024) await waitForBodySource(inspector);
    else await source.locator(".source-fallback").waitFor({ state: "visible" });
    const sample = await source.evaluate((v) => ({
      characters: v.model?.getValueLength() ?? v.bodySourceText.length,
      models: v.monaco.editor.getModels().length,
      limited: v.sourceEditorLimited,
    }));
    assert.equal(sample.characters, bytes);
    assert.equal(sample.models, bytes <= 1024 * 1024 ? baseline : baseline - 1);
    assert.equal(sample.limited, bytes > 1024 * 1024);
    measurements.push({ bytes, milliseconds: Date.now() - start, models: sample.models });
  }
  const fallback = source.locator(".source-fallback");
  assert.match(await source.locator(".source-status").textContent(), /larger preview.*plain text/);
  await inspector.locator(".body-options > summary").click();
  assert.equal(
    await inspector.getByRole("button", { name: "Find", exact: true }).isDisabled(),
    true,
  );
  await inspector.locator(".body-options > summary").click();
  await fallback.focus();
  await fallback.evaluate((text) => {
    const range = document.createRange();
    range.setStart(text.firstChild, 2);
    range.setEnd(text.firstChild, 22);
    const selection = text.getRootNode().getSelection?.() ?? document.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  });
  await page.keyboard.press("Control+c");
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "x".repeat(20));
  await page.keyboard.press("Shift+F10");
  const copyMenu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  await copyMenu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
  assert.equal(
    await page.evaluate(
      async () => (await navigator.clipboard.readText()) === "x".repeat(16 * 1024 * 1024),
    ),
    true,
  );
  const largeJson =
    '{"wide":9007199254740993,' +
    Array.from({ length: 50 }, (_, index) => `"field${index}":${index}`).join(",") +
    ',"padding":"' +
    "x".repeat(1024 * 1024) +
    '"}';
  await inspector.evaluate((i, text) => {
    i.bodySource = {
      key: "large-json",
      mediaType: "application/json",
      text,
      originalText: text,
      formatted: false,
      truncated: false,
    };
  }, largeJson);
  await fallback.waitFor({ state: "visible" });
  await inspector.locator(".body-options > summary").click();
  await inspector.getByLabel("Wrap", { exact: true }).check();
  await inspector.locator(".body-options > summary").click();
  assert.equal(
    await fallback.evaluate((element) => getComputedStyle(element).whiteSpace),
    "pre-wrap",
  );
  await inspector.locator(".body-options > summary").click();
  await inspector.getByLabel("Wrap", { exact: true }).uncheck();
  await inspector.locator(".body-options > summary").click();
  assert.equal(await fallback.evaluate((element) => getComputedStyle(element).whiteSpace), "pre");
  await fallback.focus();
  await page.keyboard.press("Control+a");
  const selectedPoint = await fallback.evaluate((text) => {
    const selection = text.getRootNode().getSelection?.() ?? document.getSelection(),
      rect = selection.getRangeAt(0).getClientRects()[0],
      bounds = text.getBoundingClientRect();
    return {
      x: Math.max(rect.left + 8, bounds.left + 8),
      y: rect.top + rect.height / 2,
      length: selection.toString().length,
    };
  });
  assert.ok(selectedPoint.length > 1024 * 1024);
  await page.mouse.click(selectedPoint.x, selectedPoint.y, { button: "right" });
  assert.equal(
    await copyMenu.getByRole("menuitem", { name: "Copy as JSON", exact: true }).count(),
    0,
    "Fallback pointer menu guessed a field from an unrelated selection boundary",
  );
  assert.equal((await copyMenu.getByRole("menuitem").allTextContents())[0], "Copy");
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 800, height: 600 });
  const response = page.getByRole("button", { name: "Response", exact: true });
  if (await response.isVisible()) await response.click();
  const coverage =
    "Only part of the body was retained because a capture limit was reached.\nXSSI prefix omitted from this formatted view; the original body is unchanged.\nPreview limited to the displayed portion.";
  await inspector.evaluate((i, coverage) => {
    i.bodyNotice = Array(8).fill(coverage).join("\n");
  }, coverage);
  const notice = inspector.locator(".body-notice"),
    status = source.locator(".source-status");
  const originalStatus = await source.evaluate((view) => view.sourceStatus);
  await source.evaluate((view, value) => {
    view.sourceStatus = Array(8).fill(value).join("\n");
  }, originalStatus);
  // Earlier workspace layouts may leave enough height for short explanations to fit.
  // Exercise actual overflow rather than requiring a fitted paragraph to scroll.
  try {
    for (const explanation of [notice, status]) {
      assert.equal(
        await explanation.evaluate((element) => element.scrollHeight > element.clientHeight),
        true,
      );
      await explanation.focus();
      await explanation.press("End");
      await page.waitForFunction(
        (element) => element.scrollTop > 0,
        await explanation.elementHandle(),
      );
      await explanation.press("Shift+F10");
      await copyMenu.getByRole("menuitem", { name: "Copy explanation", exact: true }).click();
      if (explanation === notice)
        assert.equal(
          (await page.evaluate(() => navigator.clipboard.readText())).replaceAll("\r\n", "\n"),
          Array(8).fill(coverage).join("\n"),
        );
      else assert.match(await page.evaluate(() => navigator.clipboard.readText()), /1 MiB/);
    }
  } finally {
    await source.evaluate((view, value) => {
      view.sourceStatus = value;
    }, originalStatus);
    await inspector.evaluate((view, value) => {
      view.bodyNotice = value;
    }, coverage);
  }
  await inspector.getByRole("button", { name: "Fields", exact: true }).click();
  const fields = source.locator("extension-visualizer");
  const treeBounds = await fields.getByRole("tree").evaluate((tree) => {
    const rect = tree.getBoundingClientRect();
    return { height: tree.clientHeight, bottom: rect.bottom };
  });
  assert.ok(
    treeBounds.height >= 26 && treeBounds.bottom <= 601,
    "Compact Fields lost its evidence viewport behind coverage notices: " +
      JSON.stringify(treeBounds),
  );
  assert.equal(await status.isVisible(), false);
  await fields.getByRole("button", { name: "Expand JSON", exact: true }).click();
  const tree = fields.getByRole("tree");
  await tree.focus();
  await tree.press("End");
  await tree.press("ArrowUp");
  await tree.press("Enter");
  assert.equal(await fields.evaluate((view) => view.visualizerSelectedName), "field49");
  await fields.getByRole("button", { name: "Show in source", exact: true }).click();
  await fallback.waitFor({ state: "visible" });
  assert.equal(
    await fallback.evaluate((text) =>
      (text.getRootNode().getSelection?.() ?? document.getSelection()).toString(),
    ),
    "49",
  );
  const revealed = await fallback.evaluate((text) => {
    const selection = text.getRootNode().getSelection?.() ?? document.getSelection(),
      selected = selection.getRangeAt(0).getBoundingClientRect(),
      viewport = text.getBoundingClientRect(),
      outer = text.closest(".body-scroll").getBoundingClientRect();
    return {
      focused: text.getRootNode().activeElement === text,
      scrollLeft: text.scrollLeft,
      left: selected.left,
      right: selected.right,
      top: selected.top,
      bottom: selected.bottom,
      visibleLeft: Math.max(viewport.left, outer.left),
      visibleRight: Math.min(viewport.right, outer.right, innerWidth),
      visibleTop: Math.max(viewport.top, outer.top),
      visibleBottom: Math.min(viewport.bottom, outer.bottom, innerHeight),
    };
  });
  assert.equal(revealed.focused, true, "Show in source did not focus its revealed fallback");
  assert.ok(
    revealed.scrollLeft > 0 &&
      revealed.left >= revealed.visibleLeft - 1 &&
      revealed.right <= revealed.visibleRight + 1 &&
      revealed.top >= revealed.visibleTop - 1 &&
      revealed.bottom <= revealed.visibleBottom + 1,
    "Show in source left its selected token outside the actual fallback viewport: " +
      JSON.stringify(revealed),
  );
  assert.equal(await source.evaluate((v) => v.model), null);
  await inspector.getByLabel("Body viewer", { exact: true }).selectOption("original-text");
  await waitForBodySource(inspector);
  assert.equal(await inspector.evaluate((view) => view.bodySourceLimited), false);
  assert.equal(
    await inspector.getByRole("group", { name: "JSON body view", exact: true }).count(),
    0,
  );
  await inspector.locator(".body-options > summary").click();
  assert.equal(
    await inspector.getByRole("button", { name: "Find", exact: true }).isEnabled(),
    true,
  );
  await inspector.locator(".body-options > summary").click();
  await page.setViewportSize({ width: 1280, height: 800 });
  await inspector.evaluate(async (i) => {
    for (let index = 0; index < 24; index++) {
      i.bodySource = {
        key: "switch-" + index,
        mediaType: "text/plain",
        text: "new source " + index,
        originalText: null,
        formatted: false,
        truncated: false,
      };
      await Promise.resolve();
    }
  });
  await waitForBodySource(inspector, "new source 23");
  assert.equal(await source.evaluate((v) => v.monaco.editor.getModels().length), baseline);
  await source.evaluate((v) => v.editor.focus());
  await page.keyboard.press("Control+f");
  await source.locator(".find-widget.visible").waitFor();
  await page.keyboard.press("Escape");
  await page.evaluate(() => navigator.clipboard.writeText("stale source copy sentinel"));
  await page.keyboard.press("Shift+F10");
  const menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  await menu.evaluate((element) => {
    const host = element.closest("inspection-copy-menu") ?? element.getRootNode().host;
    const action = host.request.actions.find((action) => action.id === "preview");
    const read = action.read;
    action.read = async () => {
      globalThis.__sourceCopyPending = true;
      await new Promise((resolve) => {
        globalThis.__releaseSourceCopy = resolve;
      });
      return read(() => true);
    };
  });
  await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
  await page.waitForFunction(() => globalThis.__sourceCopyPending);
  await inspector.evaluate((i) => {
    i.bodySource = { ...i.bodySource, key: "new-evidence", text: "new evidence" };
  });
  await page.evaluate(() => globalThis.__releaseSourceCopy());
  await menu.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() => navigator.clipboard.readText()),
    "stale source copy sentinel",
  );
  await waitForBodySource(inspector, "new evidence");
  await inspector.getByRole("button", { name: "Headers", exact: true }).click();
  assert.equal(await source.evaluate((v) => v.monaco.editor.getModels().length), baseline - 1);
  assert.equal(await source.evaluate((v) => v.jsonDocument), null);
  assert.equal(await source.evaluate((v) => v.sourceSnapshot), null);
  process.stdout.write(
    "Bounded body-source allocation: " +
      JSON.stringify(measurements) +
      "; rapid-switch and hidden model counts returned to baseline.\n",
  );
  await page.goto("https://workspace.test/");
}
