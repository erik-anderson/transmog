import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkFormatConverter(page, root) {
  await page.goto("https://workspace.test/");
  const host = page.locator("app-shell converter-host"),
    panel = host.getByRole("dialog", { name: "Format converter", exact: true });
  const trigger = host.getByRole("button", { name: "Format converter", exact: true });
  const filter = page.getByRole("searchbox", { name: "Filter or search traffic", exact: true });
  const input = host.getByRole("textbox", { name: "Converter input", exact: true }),
    output = host.getByRole("textbox", { name: "Converter output", exact: true });
  await trigger.click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").authConversions
        .length === 8,
  );
  assert.deepEqual(
    await host
      .locator('optgroup[label="Authentication"] option')
      .evaluateAll((options) => options.map((option) => option.value)),
    [
      "auth-decode",
      "jwt-decode",
      "jws-decode",
      "jwe-inspect",
      "basic-decode",
      "digest-parse",
      "ntlm-decode",
      "negotiate-decode",
    ],
  );
  const basicInput = "Authorization: Basic dXNlcjpwYXNz";
  const basicOutput = JSON.stringify(
    { format: "Basic", status: "decoded", username: "user", password: "pass" },
    null,
    2,
  );
  await page.evaluate(
    ({ basicInput, basicOutput }) => {
      globalThis.__workspaceFixture.authConversionOutputs = {
        "auth-decode": { [basicInput]: basicOutput },
        "basic-decode": { [basicInput]: basicOutput },
      };
    },
    { basicInput, basicOutput },
  );
  await input.fill(basicInput);
  await host.getByLabel("Conversion", { exact: true }).selectOption("auth-decode");
  await page.waitForFunction(
    (expected) =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterOutput === expected,
    basicOutput,
  );
  await host.getByLabel("Conversion", { exact: true }).selectOption("basic-decode");
  await input.fill("invalid Basic");
  await host
    .getByText("Authentication structure is malformed or unsupported.", { exact: true })
    .waitFor();
  assert.equal(await host.getByLabel("Conversion", { exact: true }).inputValue(), "basic-decode");
  assert.equal(
    await host.getByRole("button", { name: "Copy output", exact: true }).isEnabled(),
    false,
  );
  await input.fill("héllo");
  await host.getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterOutput === "aMOpbGxv",
  );
  await host.getByRole("button", { name: "Copy output", exact: true }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "aMOpbGxv");
  const jwt = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.";
  const decodedJwt = JSON.stringify(
    {
      header: { alg: "none" },
      payload: { sub: "user" },
      signature: "",
      verification: "Not checked",
    },
    null,
    2,
  );
  await page.evaluate(
    ({ jwt, decodedJwt }) => {
      globalThis.__workspaceFixture.jwtConversions = { [jwt]: decodedJwt };
    },
    { jwt, decodedJwt },
  );
  await input.fill(jwt);
  await host.getByLabel("Conversion", { exact: true }).selectOption("jwt-decode");
  await page.waitForFunction(
    (expected) =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterOutput === expected,
    decodedJwt,
  );
  await host
    .getByText("Decodes the header and claims locally. Signatures are not verified.", {
      exact: true,
    })
    .waitFor();
  await host.getByRole("button", { name: "Copy output", exact: true }).click();
  assert.equal(
    (await page.evaluate(() => navigator.clipboard.readText())).replace(/\r\n/g, "\n"),
    decodedJwt,
  );
  await host.getByRole("button", { name: "Save output…", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.converterSaved.operation),
    "jwt-decode",
  );
  await host.getByRole("button", { name: "Use output as input", exact: true }).click();
  assert.equal(await input.inputValue(), decodedJwt);
  await input.press("Control+z");
  assert.equal(await input.inputValue(), jwt);
  await input.fill("incomplete.jwt");
  await host.getByText("Expected a complete compact JWT.", { exact: true }).waitFor();
  assert.equal(await input.inputValue(), "incomplete.jwt");
  for (const name of ["Copy output", "Use output as input", "Save output…"]) {
    assert.equal(await host.getByRole("button", { name, exact: true }).isEnabled(), false);
  }
  await input.fill("héllo");
  await host.getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").converterReady,
  );
  await page.keyboard.press("Escape");
  assert.equal(await panel.isVisible(), false);
  assert.equal(await trigger.evaluate((node) => node.getRootNode().activeElement === node), true);
  await trigger.click();
  assert.equal(await input.inputValue(), "héllo");
  await host.getByRole("button", { name: "Use output as input", exact: true }).click();
  assert.equal(await input.inputValue(), "aMOpbGxv");
  await input.press("Control+z");
  assert.equal(await input.inputValue(), "héllo");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferConversion = true;
  });
  await input.fill("old");
  await page.waitForFunction(() => globalThis.__workspaceFixture.conversionPending);
  await input.fill("new");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterOutput === "bmV3",
  );
  await page.evaluate(() => globalThis.__workspaceFixture.finishConversion());
  assert.equal(await output.inputValue(), "bmV3");
  await input.fill("invalid");
  await host.getByText("Invalid conversion input.", { exact: true }).waitFor();
  assert.equal(
    await host.getByRole("button", { name: "Copy output", exact: true }).isEnabled(),
    false,
  );
  await input.fill("/wA=");
  await host.getByLabel("Conversion", { exact: true }).selectOption("base64-decode");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterOutput === "ff00",
  );
  await host.getByRole("button", { name: "Use output as input", exact: true }).click();
  assert.equal(await host.getByLabel("Input representation", { exact: true }).inputValue(), "hex");
  await input.press("Control+z");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterWindowError = true;
  });
  await host.getByRole("button", { name: "Open in new window", exact: true }).click();
  await host.getByText("Converter window could not be opened.", { exact: true }).waitFor();
  assert.equal(await input.inputValue(), "/wA=");
  assert.equal(await panel.isVisible(), true);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterWindowError = false;
    globalThis.__workspaceFixture.holdCommand = "open_converter_window";
    delete globalThis.__workspaceFixture.finishHeldCommand;
  });
  await host.getByRole("button", { name: "Open in new window", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await filter.focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterHandoff,
  );
  assert.equal(
    await filter.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "A late handoff must not change the user’s newer focus",
  );
  await panel.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.converterDraft.input),
    "/wA=",
  );
  await page.locator('tr[data-session-id="first"]').click();
  const copyMenu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  const inspector = page.locator('message-inspector[side="request"]');
  const row = inspector.locator(".headers-table tbody tr").first();
  await row.click({ button: "right" });
  await copyMenu.getByRole("menuitem", { name: "Open in Format converter", exact: true }).click();
  assert.equal(await input.inputValue(), "*/*");
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "converter_header_input";
  });
  await row.click({ button: "right" });
  await copyMenu.getByRole("menuitem", { name: "Open in Format converter", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForTimeout(30);
  assert.equal(await copyMenu.isVisible(), false, "Escape must dismiss a pending header menu");
  await row.click({ button: "right" });
  await copyMenu.getByRole("menuitem", { name: "Open in Format converter", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterWindowError = true;
    globalThis.__workspaceFixture.holdCommand = "open_converter_window";
    delete globalThis.__workspaceFixture.finishHeldCommand;
  });
  await host
    .getByRole("dialog", { name: "Keep the current conversion?", exact: true })
    .getByRole("button", { name: "Open in new window", exact: true })
    .click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishHeldCommand);
  await filter.focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("converter-host")
        .converterHandoff,
  );
  assert.equal(
    await panel.isVisible(),
    false,
    "A delayed window failure must not reopen a dismissed converter",
  );
  assert.equal(await filter.evaluate((node) => node.getRootNode().activeElement === node), true);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterWindowError = false;
  });
  await inspector.locator(".headers-table tbody tr").nth(1).click({ button: "right" });
  await copyMenu.getByText("The captured value was redacted.", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await trigger.click();
  await input.fill("selected text");
  await input.evaluate((node) => {
    node.focus();
    node.setSelectionRange(0, 8);
  });
  await input.press("Shift+F10");
  await host.getByRole("button", { name: "Open in Format converter", exact: true }).click();
  await host.getByRole("dialog", { name: "Keep the current conversion?", exact: true }).waitFor();
  await host.getByRole("button", { name: "Replace input", exact: true }).click();
  assert.equal(await input.inputValue(), "selected");
  await input.press("Control+z");
  assert.equal(await input.inputValue(), "selected text");
  await page.keyboard.press("Escape");
  assert.equal(
    await trigger.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Closing a converter opened from its own input must focus the toolbar",
  );
  await trigger.click();
  await page.evaluate(() => {
    const write = navigator.clipboard.writeText.bind(navigator.clipboard);
    navigator.clipboard.writeText = () =>
      new Promise((resolve) => {
        globalThis.finishConverterClipboard = () => {
          navigator.clipboard.writeText = write;
          resolve();
        };
      });
  });
  await input.evaluate((node) => {
    node.focus();
    node.setSelectionRange(0, 8);
  });
  await input.press("Shift+F10");
  await host.getByRole("button", { name: "Cut", exact: true }).click();
  await trigger.click();
  await input.fill("newer edit");
  await page.evaluate(() => globalThis.finishConverterClipboard());
  await page.waitForTimeout(30);
  assert.equal(
    await input.inputValue(),
    "newer edit",
    "A delayed cut must not change a newer edit",
  );
  await page.evaluate(() => {
    const read = navigator.clipboard.readText.bind(navigator.clipboard);
    navigator.clipboard.readText = () =>
      new Promise((resolve) => {
        globalThis.finishConverterClipboard = () => {
          navigator.clipboard.readText = read;
          resolve("replacement");
        };
      });
  });
  await input.evaluate((node) => {
    node.focus();
    node.setSelectionRange(0, 5);
  });
  await input.press("Shift+F10");
  await host.getByRole("button", { name: "Paste", exact: true }).click();
  await trigger.click();
  await input.fill("another selection");
  await input.evaluate((node) => {
    node.focus();
    node.setSelectionRange(0, 7);
  });
  await input.press("Shift+F10");
  await page.evaluate(() => globalThis.finishConverterClipboard());
  await page.waitForTimeout(30);
  assert.equal(
    await host.locator('textarea[aria-label="Converter input"]').inputValue(),
    "another selection",
    "A delayed paste must not edit a newer selection",
  );
  assert.equal(
    await host.getByRole("button", { name: "Open in Format converter", exact: true }).isVisible(),
    true,
    "A delayed clipboard action must not close a newer menu",
  );
  await page.keyboard.press("Escape");
  await input.evaluate((node) => {
    node.focus();
    node.setSelectionRange(0, 7);
  });
  await input.press("Shift+F10");
  await host.getByRole("button", { name: "Cut", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").inputEditor
        .value === " selection",
  );
  await input.press("Control+z");
  assert.equal(
    await input.inputValue(),
    "another selection",
    "Context-menu edits must remain undoable",
  );
  await input.fill(jwt);
  await host.getByLabel("Conversion", { exact: true }).selectOption("jwt-decode");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").converterReady,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterSaveError = true;
  });
  await host.getByRole("button", { name: "Save output…", exact: true }).click();
  await host.getByRole("dialog", { name: "Output could not be saved", exact: true }).waitFor();
  await page.keyboard.press("Escape");
  assert.equal(
    await output.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Dismissing a save failure must restore the output editor",
  );
  assert.equal(await panel.isVisible(), true);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.converterSaveError = false;
  });
  await input.fill("4100");
  await host.getByLabel("Input representation", { exact: true }).selectOption("hex");
  await host.locator("details > summary").click();
  await host.getByLabel("Text encoding", { exact: true }).selectOption("utf16le");
  await host.getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
  const openToken = async () =>
    page.evaluate((jwt) => {
      const shell = document.querySelector("app-shell");
      shell.dispatchEvent(
        new CustomEvent("format-converter-request", {
          bubbles: true,
          composed: true,
          detail: { text: jwt, operation: "jwt-decode", source: shell },
        }),
      );
    }, jwt);
  await openToken();
  await host
    .getByRole("dialog", { name: "Keep the current conversion?", exact: true })
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  assert.equal(await host.evaluate((element) => element.converterInput), "4100");
  assert.equal(await host.getByLabel("Conversion", { exact: true }).inputValue(), "base64-encode");
  await openToken();
  await host.getByRole("button", { name: "Replace input", exact: true }).click();
  assert.equal(await input.inputValue(), jwt);
  assert.equal(await host.getByLabel("Conversion", { exact: true }).inputValue(), "jwt-decode");
  assert.equal(await host.getByLabel("Input representation", { exact: true }).inputValue(), "text");
  assert.equal(await host.getByLabel("Text encoding", { exact: true }).inputValue(), "utf8");
  await input.press("Control+z");
  assert.equal(await input.inputValue(), "4100");
  assert.equal(await host.getByLabel("Conversion", { exact: true }).inputValue(), "base64-encode");
  assert.equal(await host.getByLabel("Input representation", { exact: true }).inputValue(), "hex");
  assert.equal(await host.getByLabel("Text encoding", { exact: true }).inputValue(), "utf16le");
  await input.press("Control+y");
  assert.equal(await input.inputValue(), jwt);
  assert.equal(await host.getByLabel("Conversion", { exact: true }).inputValue(), "jwt-decode");
  await host.locator("details > summary").click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").converterReady,
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
    [400, 300],
  ]) {
    await page.setViewportSize({ width, height });
    const bounds = await panel.boundingBox();
    assert.ok(
      bounds.x >= 0 &&
        bounds.y >= 0 &&
        bounds.x + bounds.width <= width + 1 &&
        bounds.y + bounds.height <= height + 1,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/converter-${width}.png`),
    });
  }
  await host.getByRole("button", { name: "Copy output", exact: true }).click();
  await input.scrollIntoViewIfNeeded();
  await page.setViewportSize({ width: 800, height: 600 });
  await page.evaluate(() => (document.querySelector("app-shell").theme = "dark"));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/converter-dark.png") });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/converter-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("https://workspace.test/");
}
