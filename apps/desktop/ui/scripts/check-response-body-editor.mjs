import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkMonacoIcons } from "./check-monaco-icons.mjs";

export async function checkResponseBodyEditor(page, root) {
  const form = page.locator(".auto-response-editor"),
    field = (name) => form.locator(`[name="${name}"]`);
  const dialog = page.locator(".response-body-dialog");
  const closed = async () => {
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("response-body-editor")
          .editor === null,
    );
  };
  const open = async () => {
    await form.getByRole("button", { name: "Edit in dialog…", exact: true }).click();
    await dialog.locator(".body-code-host > .monaco-editor").waitFor();
  };
  const focus = () =>
    page.evaluate(() =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor")
        .editor.focus(),
    );
  const type = async (value) => {
    await focus();
    await page.keyboard.press("Control+a");
    await page.keyboard.type(value);
  };
  const value = () =>
    page.evaluate(() =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor")
        .model.getValue(),
    );
  const original = await field("body").inputValue();
  let releaseStyles, requestedStyles;
  const stylesRequested = new Promise((resolve) => (requestedStyles = resolve)),
    stylesReleased = new Promise((resolve) => (releaseStyles = resolve));
  const delayStyles = async (route) => {
    requestedStyles();
    await stylesReleased;
    await route.fallback();
  };
  await page.route("**/monaco.css", delayStyles);
  await form.getByRole("button", { name: "Edit in dialog…", exact: true }).click();
  await stylesRequested;
  await dialog.locator(".body-dialog-content > label textarea").fill("plain text fallback");
  await page.evaluate(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("response-body-editor")
      .bodyEditorStyles.dispatchEvent(new Event("error")),
  );
  await dialog
    .getByText("Syntax editor could not load. You can still edit plain text.", { exact: true })
    .waitFor();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  releaseStyles();
  await page.unroute("**/monaco.css", delayStyles);
  assert.equal(
    await field("body").inputValue(),
    original,
    "Failed syntax loading changed the inline draft",
  );

  // Start saving the plain-text fallback before Monaco's stylesheet finishes loading.
  // Hydration must inherit the pending save lock, including real keyboard input.
  let releaseLateStyles, requestedLateStyles;
  const lateRequested = new Promise((resolve) => (requestedLateStyles = resolve)),
    lateReleased = new Promise((resolve) => (releaseLateStyles = resolve));
  const delayLateStyles = async (route) => {
    requestedLateStyles();
    await lateReleased;
    await route.fallback();
  };
  await page.route("**/monaco.css", delayLateStyles);
  await form.getByRole("button", { name: "Edit in dialog…", exact: true }).click();
  await lateRequested;
  await dialog
    .locator(".body-dialog-content > label textarea")
    .fill("saved before syntax hydration");
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.deferAssetEdit = true;
    s.assetEditPending = false;
  });
  await dialog.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.assetEditPending);
  releaseLateStyles();
  await dialog.locator(".body-code-host > .monaco-editor").waitFor();
  await page.unroute("**/monaco.css", delayLateStyles);
  await focus();
  await page.keyboard.press("Control+End");
  await page.keyboard.type(" input during save");
  assert.equal(
    await value(),
    "saved before syntax hydration",
    "Late syntax hydration allowed edits that the pending save would lose",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferAssetEdit = false;
    globalThis.__workspaceFixture.finishAssetEdit();
  });
  await closed();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .draftDirty,
  );
  assert.equal(await field("body").inputValue(), "saved before syntax hydration");
  await field("body").fill(original);
  await form.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .draftDirty,
  );

  const html =
    "<html><body><script>globalThis.autoBodyExecuted=true;</script><h1>Offline</h1></body></html>";
  await field("body").fill(html);
  await open();
  assert.equal(await dialog.getByLabel("Body language", { exact: true }).inputValue(), "html");
  assert.equal(
    await dialog.getByRole("button", { name: "Save rule", exact: true }).isVisible(),
    true,
  );
  await type("cancelled body");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  assert.equal(await field("body").inputValue(), html);
  assert.equal(
    await form
      .getByRole("button", { name: "Edit in dialog…", exact: true })
      .evaluate((el) => el.getRootNode().activeElement === el),
    true,
  );
  assert.equal(
    await page.evaluate(() => globalThis.autoBodyExecuted),
    undefined,
    "Body markup executed in the workbench",
  );

  for (const [language, text] of [
    ["html", html],
    ["css", "body{color:red;background:white;}"],
    ["javascript", "function add(a,b){return a+b;}"],
    ["json", '{"value":[1,2],"active":true}'],
  ]) {
    await open();
    await page.evaluate((text) => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor");
      w.editor.executeEdits("fixture-paste", [{ range: w.model.getFullModelRange(), text }]);
    }, text);
    await dialog.getByLabel("Body language", { exact: true }).selectOption(language);
    await dialog.getByRole("button", { name: "Format", exact: true }).click();
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("response-body-editor")
          .bodyFormatting,
    );
    assert.notEqual(
      await value(),
      text,
      `${language} formatting did not change unformatted content`,
    );
    if (language === "json") await checkMonacoIcons(dialog.locator(".body-code-host"));
    await focus();
    await page.keyboard.press("Control+z");
    await page.waitForFunction(
      (text) =>
        document
          .querySelector("app-shell")
          .shadowRoot.querySelector("response-body-editor")
          .model.getValue() === text,
      text,
      { timeout: 3000 },
    );
    assert.equal(await value(), text, `${language} formatting cannot be undone`);
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await closed();
  }
  await open();
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-body-dialog.png"),
  });
  const apply = await dialog
    .getByRole("button", { name: "Apply to draft", exact: true })
    .boundingBox();
  assert.ok(apply.y + apply.height < 600);
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-body-dialog-contrast.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await type("applied body");
  await dialog.getByRole("button", { name: "Apply to draft", exact: true }).click();
  await closed();
  assert.equal(await field("body").inputValue(), "applied body");
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
          .draftDirty,
    ),
    true,
  );

  const name = await field("name").inputValue();
  await field("name").fill("Another pending property");
  await open();
  assert.equal(
    await dialog.getByRole("button", { name: "Save rule", exact: true }).count(),
    1,
    "Dialog should explicitly save all pending rule changes",
  );
  await dialog.getByText(/Save rule saves all pending rule changes/).waitFor();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  await field("name").fill(name);
  await open();
  assert.equal(
    await dialog.getByRole("button", { name: "Save rule", exact: true }).count(),
    1,
    "Restored properties still hide body-only saving",
  );
  await type("save failure draft");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.assetError = true;
    globalThis.__workspaceFixture.deferAssetEdit = true;
  });
  await dialog.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.assetEditPending);
  assert.equal(
    await page.evaluate(() => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor");
      return w.editor.getOption(w.monaco.editor.EditorOption.readOnly);
    }),
    true,
    "Pending save permits later body edits",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferAssetEdit = false;
    globalThis.__workspaceFixture.finishAssetEdit();
  });
  await dialog.getByText(/Fixture asset write failed/).waitFor();
  assert.equal(await value(), "save failure draft");
  await type("revised after failed save");
  assert.equal(
    await value(),
    "revised after failed save",
    "Failed Save left the body editor locked",
  );
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  assert.equal(
    await field("body").inputValue(),
    "applied body",
    "Cancel after a failed save lost the earlier draft",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.assetError = false));
  await open();
  await type("saved from dialog");
  await dialog.getByRole("button", { name: "Save rule", exact: true }).click();
  await closed();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .draftDirty,
  );
  assert.equal(await field("body").inputValue(), "saved from dialog");
  assert.equal(
    await page.evaluate(() => {
      const w = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor");
      return w.editor === null && w.model === null;
    }),
    true,
    "Closing retained editor/model resources",
  );
  // Preserve the fixture's prior body for the remaining workspace checks.
  await field("body").fill(original);
  await form.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("automation-workspace")
        .draftDirty,
  );

  await checkSvgAndHexDialog(page, root, { form, field, dialog, open, closed, focus, value });
}

async function checkSvgAndHexDialog(
  page,
  root,
  { form, field, dialog, open, closed, focus, value },
) {
  const original = await field("body").inputValue(),
    mediaType = await field("mediaType").inputValue();
  const svg =
    '<?xml version="1.0"?>\n<!-- captured markup stays inert -->\n<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><style>.shape { fill: #336699; }</style><linearGradient id="paint"><stop stop-color="#ff0000" /></linearGradient><path class="shape" d="M0 0 L40 40"/><script>globalThis.svgBodyExecuted=true;</script></svg>';
  await field("body").fill(svg);
  await open();
  assert.equal(
    await dialog.getByLabel("Body language", { exact: true }).inputValue(),
    "svg",
    "SVG with an XML declaration was not detected",
  );
  assert.equal(
    await dialog.getByRole("button", { name: "Format", exact: true }).isDisabled(),
    true,
    "Unsafe HTML formatting is offered for SVG",
  );
  assert.equal(
    await value(),
    svg,
    "Opening SVG changed namespaces, attribute case, or self-closing markup",
  );
  assert.equal(
    await page.evaluate(() => globalThis.svgBodyExecuted),
    undefined,
    "Captured SVG executed",
  );
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();

  await field("mediaType").fill("image/svg+xml; charset=utf-8");
  await field("body").fill("<!-- SVG is still being written -->");
  await open();
  assert.equal(
    await dialog.getByLabel("Body language", { exact: true }).inputValue(),
    "svg",
    "SVG MIME type was not detected",
  );
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  await field("mediaType").fill(mediaType);

  await field("body").fill("<svg><style>.shape { fill: #336699; }</style></svg>");
  await open();
  await page.setViewportSize({ width: 800, height: 600 });
  const picker = dialog.locator(".colorpicker-widget");
  const assertPickerUsable = async () => {
    // Monaco replaces the hover while async color information settles. Read
    // visibility, geometry and hit targets together, instead of racing a
    // detached widget between separate waitFor/boundingBox/evaluate calls.
    const measured = await page.waitForFunction(() => {
      const w = document
          .querySelector("app-shell")
          .shadowRoot.querySelector("response-body-editor"),
        el = w.overflowHost.querySelector(".colorpicker-widget");
      if (!el) return false;
      const box = el.getBoundingClientRect(),
        root = el.getRootNode();
      if (
        !box.width ||
        !box.height ||
        getComputedStyle(el).visibility === "hidden" ||
        Number(getComputedStyle(el.closest(".monaco-hover")).opacity) < 1
      )
        return false;
      return {
        box: box.toJSON(),
        hit: [box.top + 5, box.bottom - 5].every((y) => {
          const hit = root.elementFromPoint(box.left + box.width / 2, y);
          return hit !== null && el.contains(hit);
        }),
        inDialog: el.closest(".body-editor-overflow")?.closest("dialog")?.open === true,
      };
    });
    const { box, hit, inDialog } = await measured.jsonValue();
    await measured.dispose();
    assert.ok(
      box.y >= 0 && box.x >= 0 && box.y + box.height <= 600 && box.x + box.width <= 800,
      "SVG color picker is outside the compact viewport: " + JSON.stringify(box),
    );
    assert.equal(
      hit,
      true,
      "The top or bottom of the color picker is clipped or cannot receive pointer events",
    );
    assert.equal(inDialog, true, "Color picker escaped the modal top layer");
  };
  await dialog.locator(".colorpicker-color-decoration").first().click();
  await assertPickerUsable();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-svg-picker-top.png"),
  });
  await page.keyboard.press("Escape");
  await picker.waitFor({ state: "hidden" });
  assert.equal(await dialog.isVisible(), true, "Escape from the picker closed the body editor");
  // Move the color to the last visible line and verify Monaco can place it above.
  await page.evaluate(() => {
    const w = document.querySelector("app-shell").shadowRoot.querySelector("response-body-editor");
    w.model.setValue(
      "<svg>\n" +
        Array(40).fill("<!-- shape -->").join("\n") +
        "\n<style>.shape { fill: #336699; }</style>\n</svg>",
    );
    w.editor.setPosition({ lineNumber: 42, column: 25 });
    w.editor.revealLine(42);
  });
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  await dialog.locator(".colorpicker-color-decoration").first().click();
  await assertPickerUsable();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-svg-picker-bottom.png"),
  });
  await page.keyboard.press("Escape");
  await picker.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("response-body-editor")
        .editor.hasTextFocus(),
    ),
    true,
    "Escape from the picker did not restore editor focus",
  );
  await page.setViewportSize({ width: 400, height: 300 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-svg-dialog-200percent.png"),
  });
  assert.ok(
    (await dialog.locator(".body-code-host").boundingBox()).height >= 60,
    "SVG help text leaves no usable editor at 200% zoom",
  );
  for (const name of ["Apply to draft", "Save rule", "Cancel"]) {
    const box = await dialog.getByRole("button", { name, exact: true }).boundingBox();
    assert.ok(
      box && box.x >= 0 && box.y >= 0 && box.x + box.width <= 400 && box.y + box.height <= 300,
      `SVG dialog ${name} is unreachable at 200% zoom`,
    );
  }
  await page.setViewportSize({ width: 800, height: 600 });
  await page.keyboard.press("Escape");
  await closed();
  assert.equal(
    await form
      .getByRole("button", { name: "Edit in dialog…", exact: true })
      .evaluate((el) => el.getRootNode().activeElement === el),
    true,
    "Closing the SVG editor did not restore focus",
  );

  await field("body").fill("A\nB");
  await open();
  await dialog.getByLabel("Body view", { exact: true }).selectOption("hex");
  assert.match(await value(), /^41 0a 42$/);
  await dialog.getByLabel("Body view", { exact: true }).selectOption("text");
  assert.equal(await value(), "A\nB");
  await dialog.getByLabel("Body view", { exact: true }).selectOption("hex");
  await focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("00 ff 80");
  await dialog.getByLabel("Body view", { exact: true }).selectOption("text");
  assert.equal(
    await dialog.getByLabel("Body view", { exact: true }).inputValue(),
    "hex",
    "Binary bytes switched into lossy text",
  );
  assert.equal(await value(), "00 ff 80");
  await dialog.getByText(/cannot be shown as lossless Unicode text/).waitFor();
  await focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("a");
  await dialog.getByRole("button", { name: "Apply to draft", exact: true }).click();
  assert.equal(await dialog.isVisible(), true, "An incomplete hex byte closed the editor");
  await dialog.getByText(/Each hexadecimal byte needs two digits/).waitFor();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  // The view baseline must retain bytes normalized by textarea and Monaco.
  await page.evaluate(() => {
    const w = document.querySelector("app-shell").shadowRoot.querySelector("response-body-editor"),
      opener = document
        .querySelector("app-shell")
        .shadowRoot.querySelector(".auto-response-editor button");
    w.open("A\r\nB\nC", "plaintext", false, opener, {
      bytes: [65, 13, 10, 66, 10, 67],
      textEncoding: "utf-8",
    });
  });
  await dialog.locator(".body-code-host .monaco-editor").waitFor();
  await dialog.getByLabel("Body view", { exact: true }).selectOption("hex");
  assert.equal(
    await value(),
    "41 0d 0a 42 0a 43",
    "Opening or switching views normalized original bytes",
  );
  // 200% browser zoom in an 800×600 window leaves a 400×300 CSS viewport.
  await page.setViewportSize({ width: 400, height: 300 });
  assert.ok(
    (await dialog.locator(".body-code-host").boundingBox()).height >= 60,
    "Body editor has no usable editing area at 200% zoom",
  );
  for (const name of ["Apply to draft", "Cancel"]) {
    const box = await dialog.getByRole("button", { name, exact: true }).boundingBox();
    assert.ok(
      box && box.x >= 0 && box.y >= 0 && box.x + box.width <= 400 && box.y + box.height <= 300,
      `Dialog ${name} is unreachable at 200% zoom`,
    );
  }
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/auto-response-body-dialog-200percent.png"),
  });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await closed();
  await page.setViewportSize({ width: 1280, height: 800 });
  await field("body").fill(original);
  await field("mediaType").fill(mediaType);
}
