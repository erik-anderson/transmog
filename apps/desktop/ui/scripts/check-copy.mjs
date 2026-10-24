import assert from "node:assert/strict";
import { resolve } from "node:path";

async function checkTrafficMenuFocus(traffic) {
  const result = await traffic.evaluate(async (element) => {
    const menu = element.trafficMenu;
    const opened = new Promise((resolve) =>
      menu.addEventListener("toggle", resolve, { once: true }),
    );
    element.querySelector('tr[data-session-id="second"]').dispatchEvent(
      new MouseEvent("contextmenu", {
        button: 2,
        clientX: 300,
        clientY: 240,
        bubbles: true,
        cancelable: true,
      }),
    );
    const root = menu.getRootNode();
    const focusedWhenOpened = menu.contains(root.activeElement);
    root.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }),
    );
    const firstUp = root.activeElement.textContent.trim();
    root.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }),
    );
    const firstDown = root.activeElement.textContent.trim();
    root.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }),
    );
    const endItem = root.activeElement;
    await opened;
    const navigationPreserved = endItem === root.activeElement && menu.contains(endItem);
    for (const key of ["Home", "Escape"])
      root.activeElement.dispatchEvent(
        new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
      );
    return {
      focusedWhenOpened,
      firstUp,
      firstDown,
      navigationPreserved,
      closed: !menu.matches(":popover-open"),
      selected: [...element.trafficSelection.ids],
    };
  });
  assert.equal(result.focusedWhenOpened, true, "Traffic menu opened without keyboard focus");
  assert.equal(result.firstUp, "Clear selection", "Immediate ArrowUp missed the last menu action");
  assert.equal(result.firstDown, "Edit and replay", "ArrowDown did not wrap to the first action");
  assert.equal(result.navigationPreserved, true, "Queued menu toggle stole keyboard navigation");
  assert.equal(result.closed, true, "Immediate Escape did not dismiss the Traffic menu");
  assert.deepEqual(result.selected, ["second"], "Immediate menu keys changed Traffic selection");
  await traffic.locator('tr[data-session-id="first"]').click();
  await traffic.locator('tr[data-session-id="second"]').click({ modifiers: ["Control"] });
  const more = traffic.locator('.traffic-selection-bar button[popovertarget="traffic-entry-menu"]');
  await more.waitFor();
  const multiple = await more.evaluate((button) => {
    const element = button.closest("traffic-workspace");
    button.click();
    const menu = element.trafficMenu;
    const root = menu.getRootNode();
    const focusedWhenOpened = menu.contains(root.activeElement);
    for (const key of ["End", "Home", "Escape"])
      root.activeElement.dispatchEvent(
        new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
      );
    return {
      focusedWhenOpened,
      closed: !menu.matches(":popover-open"),
      selected: [...element.trafficSelection.ids].sort(),
    };
  });
  assert.equal(multiple.focusedWhenOpened, true, "Traffic More menu opened without keyboard focus");
  assert.equal(multiple.closed, true, "Immediate Escape did not dismiss the Traffic More menu");
  assert.deepEqual(multiple.selected, ["first", "second"], "Menu keys changed multiselection");
  await traffic.locator('tr[data-session-id="first"]').click();
}

export async function checkCopy(page, root) {
  await checkTextMenus(page, root);
  await page.goto("https://workspace.test/");
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic;
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.__copied = text;
        },
      },
    });
    t.preferences = { ...t.preferences, layout: "stacked", listSplit: 25 };
    state.visualizerChoices = {
      body: [
        { extensionId: "example.protocol", visualizerId: "structure", name: "Copy structure" },
      ],
      headers: [],
    };
    const node = (id, name, value, children = [], childrenToken = null) => ({
      id,
      name,
      value,
      children,
      childrenToken,
      spans: [],
    });
    state.visualizerDocument = {
      title: "Copy fixture",
      status: "complete",
      presentation: "tree",
      summary: null,
      sources: [],
      diagnostics: [],
      roots: [
        node("first", "First", { kind: "integer", value: "9007199254740993" }),
        node("branch", "Branch", { kind: "object" }, [], "deferred"),
        {
          ...node("bytes", "Bytes", { kind: "bytes" }),
          spans: [{ sourceId: "derived", offset: "1", length: "3" }],
        },
        node("long", "Long", { kind: "string", value: "full-".repeat(200) }),
      ],
    };
    state.visualizerDocument.sources = [
      {
        id: "derived",
        name: "Binary",
        bytesBase64: btoa(String.fromCharCode(1, 0, 10, 255)),
        parents: [],
        transform: "fixture",
        exactMapping: false,
      },
    ];
    state.expandedVisualizerDocument = structuredClone(state.visualizerDocument);
    state.expandedVisualizerDocument.roots[1].childrenToken = null;
    state.expandedVisualizerDocument.roots[1].children = [
      node("child", "Child", { kind: "string", value: "hello\nworld" + "x".repeat(600) }),
    ];
    state.nextExpandedDocument = structuredClone(state.expandedVisualizerDocument);
    state.nextExpandedDocument.roots[1].childrenToken = "deferred";
    state.expandedVisualizerDocument.roots[1].children.push(
      node("continued", "Continued", { kind: "string", value: "second page" }),
    );
    state.emitNative("extensions-changed");
    await t.refreshSessions(undefined, true);
  });
  await page.locator('tr[data-session-id="first"]').click();
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  await inspector.getByRole("button", { name: "Body", exact: true }).click();
  await inspector
    .getByLabel("Body viewer", { exact: true })
    .selectOption({ label: "Copy structure" });
  const visualizer = inspector.locator('extension-visualizer[slot="traffic-response-body"]');
  const row = (name) =>
    visualizer.getByRole("treeitem").filter({
      has: page.locator(".visualizer-field", { hasText: new RegExp("^" + name + "$") }),
    });
  const menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  await row("First").click();
  const selected = await visualizer.getByRole("tree").getAttribute("aria-activedescendant");
  await row("Bytes").click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied === "00 0A FF");
  assert.equal(await visualizer.getByRole("tree").getAttribute("aria-activedescendant"), selected);
  await row("Long").click({ button: "right" });
  assert.equal(
    await menu.getByRole("menuitem", { name: "Copy subtree", exact: true }).count(),
    0,
    "Leaf offered a subtree action",
  );
  await page.keyboard.press("Escape");
  await row("Long").click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied === "full-".repeat(200));
  await row("Branch").click({ button: "right" });
  assert.deepEqual(
    await menu.getByRole("menuitem").allTextContents(),
    ["Copy name", "Copy subtree"],
    "Grouping node offered scalar actions",
  );
  await menu.getByRole("menuitem", { name: "Copy subtree", exact: true }).click();
  await page.waitForFunction(
    () =>
      globalThis.__copied ===
      "Branch\n  Child: hello\\nworld" + "x".repeat(600) + "\n  Continued: second page",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.expand_visualizer),
    2,
    "Copying rejected progressive children under a stable node/token",
  );
  assert.equal(await row("Branch").getAttribute("aria-expanded"), "false");
  assert.equal(await visualizer.getByRole("tree").getAttribute("aria-activedescendant"), selected);
  await visualizer.getByRole("tree").focus();
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy name", exact: true }).waitFor();
  await page.keyboard.press("Escape");
  assert.equal(
    await visualizer
      .getByRole("tree")
      .evaluate((element) => element === element.getRootNode().activeElement),
    true,
  );
  await page.keyboard.press("Home");
  await visualizer.getByLabel("Structure view", { exact: true }).selectOption("table");
  const longCell = visualizer
    .locator(".visualizer-table tbody tr")
    .filter({ has: page.getByRole("button", { name: "Branch", exact: true }) })
    .locator("td")
    .filter({ hasText: /^hello/ });
  await longCell.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied === "hello\\nworld" + "x".repeat(600));
  await inspector.getByRole("button", { name: "Headers", exact: true }).click();
  const header = inspector.locator(".headers-table tbody tr").first();
  await header.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy name and value", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied === "Accept: */*");
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.authValues = { public: "full public", secret: "private-value" };
    state.authInspection = {
      findings: [
        {
          id: "f0",
          format: "Basic",
          role: "Credential",
          status: "decoded",
          recognition: "recognized",
          verification: "Not checked",
          source: { label: "Header", transforms: [] },
          diagnostics: [],
          children: [],
          fields: [
            {
              id: "public",
              name: "User",
              value: "full public",
              available: true,
              sensitive: false,
              note: "",
            },
            {
              id: "secret",
              name: "Password",
              value: "••••••••",
              available: true,
              sensitive: true,
              note: "",
            },
          ],
        },
      ],
      coverage: [],
      diagnostics: [],
    };
  });
  await inspector.evaluate((element) => {
    element.detail = {
      ...element.detail,
      authRevisions: { ...element.detail.authRevisions, [element.boundary]: "copy-auth-fixture" },
    };
  });
  await inspector.getByRole("button", { name: /^Auth/ }).click();
  const auth = inspector.locator("auth-inspector"),
    summary = auth.locator(".auth-finding > summary").first();
  await summary.click({ button: "right" });
  assert.deepEqual(
    await menu.getByRole("menuitem").allTextContents(),
    ["Copy name", "Copy subtree", "Copy subtree including secrets"],
    "Auth heading offered scalar actions",
  );
  await menu.getByRole("menuitem", { name: "Copy subtree", exact: true }).click();
  await page.waitForFunction(
    () => globalThis.__copied === "Basic · Credential\n  User: full public\n  Password: [masked]",
  );
  await summary.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy subtree including secrets", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied.includes("Password: private-value"));
  assert.equal(await auth.getByText("private-value", { exact: true }).count(), 0);
  const password = auth
    .locator(".auth-field")
    .filter({ has: page.locator("dt", { hasText: "Password" }) });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferAuthValue = true;
    globalThis.__copied = "keep clipboard";
  });
  await password.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishAuthValue === "function",
  );
  await page.keyboard.press("Escape");
  await page.evaluate(() => globalThis.__workspaceFixture.finishAuthValue());
  await page.waitForTimeout(40);
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    "keep clipboard",
    "Dismissed copy overwrote the clipboard",
  );
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async () => {
          throw new Error("private-native-error");
        },
      },
    }),
  );
  await password.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await menu
    .getByText("Copy failed. The clipboard or retained value is unavailable.", { exact: true })
    .waitFor();
  assert.equal(await menu.getByText("private-native-error", { exact: true }).count(), 0);
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.authInspection.findings[0].fields.pop();
  });
  await auth.locator(".auth-options > summary").click();
  await auth.getByRole("button", { name: "Refresh inspection", exact: true }).click();
  await auth.getByText("full public", { exact: true }).waitFor();
  await summary.click({ button: "right" });
  assert.equal(
    await menu
      .getByRole("menuitem", { name: "Copy subtree including secrets", exact: true })
      .count(),
    0,
    "Public-only finding offered a secret-inclusive copy",
  );
  await page.keyboard.press("Escape");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/copy-auth.png") });
  await page.goto("https://workspace.test/");
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.__copied = text;
        },
      },
    }),
  );
  await page.locator('tr[data-session-id="first"]').click();
  const traffic = page.locator("traffic-workspace");
  await checkTrafficMenuFocus(traffic);
  await traffic
    .locator('tr[data-session-id="second"] td[data-column-id="path"]')
    .click({ button: "right" });
  const requestMenu = traffic.locator("#traffic-entry-menu");
  await requestMenu.getByRole("button", { name: "Edit and replay", exact: true }).waitFor();
  assert.equal(
    await menu.isVisible(),
    false,
    "Traffic context menu was replaced by an inspection copy menu",
  );
  assert.equal(
    await traffic.locator('tr[data-session-id="second"]').getAttribute("aria-selected"),
    "true",
    "Traffic actions did not target the row under the pointer",
  );
  for (const [width, height, zoom] of [
    [1440, 900, 1],
    [800, 600, 1.25],
  ]) {
    await page.keyboard.press("Escape");
    await page.setViewportSize({ width, height });
    await page.evaluate((zoom) => (document.documentElement.style.zoom = String(zoom)), zoom);
    const cell = traffic.locator('tr[data-session-id="second"] td[data-column-id="path"]');
    await cell.scrollIntoViewIfNeeded();
    const bounds = await cell.boundingBox(),
      point = { x: Math.min(width - 12, bounds.x + bounds.width - 12), y: bounds.y + 12 };
    await page.mouse.click(point.x, point.y, { button: "right" });
    await requestMenu.waitFor();
    const box = await requestMenu.boundingBox();
    assert.ok(
      Math.abs(box.x - Math.max(8, Math.min(point.x, width - box.width - 8))) < 2 &&
        Math.abs(box.y - Math.max(8, Math.min(point.y, height - box.height - 8))) < 2,
      "Traffic menu did not use measured cursor placement",
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/menu-traffic-${width}.png`),
    });
    await page.keyboard.press("End");
    await page.keyboard.press("Home");
    await page.keyboard.press("Escape");
    assert.equal(
      await traffic.locator('tr[data-session-id="second"]').getAttribute("aria-selected"),
      "true",
      "Dismissing Traffic menu cleared selection",
    );
    await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).press("Shift+F10");
    await requestMenu.waitFor();
    await page.keyboard.press("Escape");
  }
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.setViewportSize({ width: 1440, height: 900 });
  await traffic.locator('tr[data-session-id="second"]').click({ button: "right" });
  await requestMenu
    .locator('action-submenu[label="Copy"]')
    .getByRole("button", { name: "Copy", exact: true })
    .click();
  await requestMenu.getByRole("button", { name: "Copy URL", exact: true }).click();
  await page.waitForFunction(() => globalThis.__copied === "http://example.test/second");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.savedTiming = {
      ClientBeginRequest: "2026-10-20T00:00:00Z",
      ClientDoneResponse: "2026-10-20T00:00:01Z",
    };
  });
  await traffic.getByRole("button", { name: "Details…", exact: true }).click();
  const dialog = traffic.locator("dialog[open]");
  const detail = dialog.locator("dd:visible").first();
  await detail.waitFor();
  const expected = await detail.textContent();
  await detail.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    expected,
    "Read-only modal value did not copy",
  );
  await detail.focus();
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy section", exact: true }).waitFor();
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/copy-details-800.png") });
  await page.keyboard.press("Escape");
  assert.equal(
    await detail.evaluate((element) => element === element.getRootNode().activeElement),
    true,
    "Modal copy menu did not restore field focus",
  );
  await detail.evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    const selection = element.getRootNode().getSelection?.() ?? document.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    element.focus();
  });
  await page.keyboard.press("Shift+F10");
  assert.deepEqual(
    await menu.getByRole("menuitem").allTextContents(),
    ["Copy"],
    "Selected modal evidence used an inaccessible outer menu",
  );
  await menu.getByRole("menuitem", { name: "Copy", exact: true }).click();
  assert.equal(await page.evaluate(() => globalThis.__copied), expected);
  await dialog
    .locator("inspection-copy-menu .copy-feedback")
    .getByText("Copied.", { exact: true })
    .waitFor();
  const report = dialog.getByLabel("Timing report", { exact: true });
  await dialog.getByText("Report text", { exact: true }).click();
  await report.evaluate((element) => {
    element.value = "Original report";
    element.focus();
    element.setSelectionRange(0, 8);
  });
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy", exact: true }).waitFor();
  await report.evaluate((element) => {
    element.value = "Updated report";
  });
  await page.evaluate(() => (globalThis.__copied = "keep clipboard"));
  await menu.getByRole("menuitem", { name: "Copy", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    "keep clipboard",
    "Changed modal text copied an obsolete selection",
  );
  await page.goto("https://workspace.test/");
  const converter = page.locator("converter-host");
  await converter.getByRole("button", { name: "Format converter", exact: true }).click();
  await converter.getByLabel("Converter input", { exact: true }).fill("copy text");
  await converter.getByLabel("Conversion", { exact: true }).selectOption("base64-encode");
  await converter.getByRole("button", { name: "Convert", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("converter-host").converterReady,
  );
  const output = converter.getByLabel("Converter output", { exact: true });
  const text = await output.inputValue();
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.__copied = text;
        },
      },
    }),
  );
  await output.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy output", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    text,
    "Read-only converter output was lost when its copy menu opened",
  );
  const input = converter.getByLabel("Converter input", { exact: true });
  for (const [width, height, zoom] of [
    [1440, 900, 1],
    [800, 600, 1.25],
  ]) {
    await page.setViewportSize({ width, height });
    await page.evaluate((zoom) => (document.documentElement.style.zoom = String(zoom)), zoom);
    await input.evaluate((element) => {
      element.focus();
      element.setSelectionRange(0, 4);
    });
    const anchor = await input.boundingBox();
    await page.keyboard.press("Shift+F10");
    const selectionMenu = converter.locator(".selection-menu");
    await selectionMenu.waitFor();
    assert.equal(await selectionMenu.getByRole("button").first().textContent(), "Copy");
    const box = await selectionMenu.boundingBox();
    assert.ok(
      box.x >= 0 &&
        box.y >= 0 &&
        box.x + box.width <= width + 1 &&
        box.y + box.height <= height + 1,
      "Text menu overflowed the viewport",
    );
    assert.ok(
      Math.abs(box.x - Math.max(8, Math.min(anchor.x, width - box.width - 8))) < 2 &&
        Math.abs(box.y - Math.max(8, Math.min(anchor.y + anchor.height, height - box.height - 8))) <
          2,
      "Keyboard menu lost the source position when its parent popover closed",
    );
    await page.keyboard.press("End");
    await page.keyboard.press("Home");
    assert.equal(
      await selectionMenu
        .getByRole("button", { name: "Copy", exact: true })
        .evaluate((element) => element === element.getRootNode().activeElement),
      true,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/menu-text-${width}.png`),
    });
    await page.keyboard.press("Escape");
    assert.equal(
      await input.evaluate((element) => element === element.getRootNode().activeElement),
      true,
    );
  }
  await page.evaluate(() => (document.documentElement.style.zoom = ""));

  await page.goto("https://workspace.test/");
  await page.setViewportSize({ width: 1280, height: 800 });
}

async function checkTextMenus(page, root) {
  await page.goto("https://workspace.test/");
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text) => {
          globalThis.__copied = text;
        },
      },
    }),
  );
  await page.locator('tr[data-session-id="first"]').click();
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const menu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  await inspector.getByRole("button", { name: "Body", exact: true }).click();
  await inspector.getByLabel("Body viewer", { exact: true }).selectOption("original-text");
  const preview = inspector.locator("pre.body-preview");
  await preview.getByText("body for first", { exact: true }).waitFor();
  for (const [width, height, zoom] of [
    [1440, 900, 1],
    [960, 720, 1],
    [800, 600, 1],
    [1440, 900, 1.25],
  ]) {
    await page.setViewportSize({ width, height });
    await page.evaluate((zoom) => (document.documentElement.style.zoom = String(zoom)), zoom);
    const switcher = page.getByRole("button", { name: "Response", exact: true });
    if ((await switcher.count()) && (await switcher.first().isVisible()))
      await switcher.first().click();
    await preview.scrollIntoViewIfNeeded();
    const rect = await preview.boundingBox();
    for (const point of [
      { x: rect.x + 20, y: rect.y + 20 },
      {
        x: Math.min(width - 12, rect.x + rect.width - 12),
        y: Math.min(height - 12, rect.y + rect.height - 12),
      },
    ]) {
      await page.mouse.click(point.x, point.y, { button: "right" });
      await menu.waitFor();
      assert.deepEqual(
        await menu.getByRole("menuitem").allTextContents(),
        ["Copy body preview"],
        "A body is a text block, not a name/value field",
      );
      const box = await menu.boundingBox();
      assert.ok(
        box.x >= 0 && box.y >= 0 && box.x + box.width <= width && box.y + box.height <= height,
        "Copy menu is clipped",
      );
      // A menu should touch the click unless the actual menu size hits an edge.
      assert.ok(
        Math.abs(box.x - Math.max(8, Math.min(point.x, width - box.width - 8))) < 2,
        "Copy menu is detached horizontally",
      );
      assert.ok(
        Math.abs(box.y - Math.max(8, Math.min(point.y, height - box.height - 8))) < 2,
        "Copy menu is detached vertically",
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/copy-body-${width}-${zoom}.png`),
      });
      await page.keyboard.press("Escape");
      assert.equal(
        await preview.evaluate((element) => element === element.getRootNode().activeElement),
        true,
        "Body menu lost return focus",
      );
    }
  }
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.setViewportSize({ width: 1440, height: 900 });
  await preview.focus();
  await page.keyboard.press("Shift+F10");
  await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
  assert.equal(await page.evaluate(() => globalThis.__copied), "body for first");
  await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    globalThis.__bodyOriginalInvoke = invoke;
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const value = await invoke(command, args);
      return command === "inspect_body"
        ? {
            ...value,
            display: "retained text only",
            truncated: true,
            warning: "Encoding warning from fixture",
          }
        : value;
    };
    await document.querySelector("app-shell").traffic.responseMessage.inspectBody();
  });
  await inspector
    .locator(".body-notice")
    .getByText(/Encoding warning/)
    .waitFor();
  await preview.click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Copy body preview", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    "retained text only",
    "Body copy mixed status prose with evidence",
  );
  await inspector.locator(".body-notice").click({ button: "right" });
  assert.deepEqual(await menu.getByRole("menuitem").allTextContents(), ["Copy explanation"]);
  await page.keyboard.press("Escape");
  assert.match(
    await inspector.locator(".body-facts > summary").textContent(),
    /shown.*no content decoding.*preview limited/,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/body-warning.png") });
  await page.evaluate(() => (window.__TAURI_INTERNALS__.invoke = globalThis.__bodyOriginalInvoke));
  await page.locator('tr[data-session-id="cached"]').click();
  await inspector
    .locator(".body-notice")
    .getByText(/304 Not Modified/)
    .waitFor();
  assert.equal(
    await inspector.locator("pre.body-preview").count(),
    0,
    "Absent body offered a body-copy surface",
  );
  await inspector.locator(".body-notice").click({ button: "right" });
  assert.deepEqual(await menu.getByRole("menuitem").allTextContents(), ["Copy explanation"]);
  await page.keyboard.press("Escape");
  await inspector.getByRole("button", { name: "Raw data", exact: true }).click();
  const raw = inspector.locator(".exchange-details");
  const rawText = await raw.textContent();
  await raw.click({ button: "right" });
  assert.deepEqual(await menu.getByRole("menuitem").allTextContents(), ["Copy raw data"]);
  await menu.getByRole("menuitem", { name: "Copy raw data", exact: true }).click();
  assert.equal(
    await page.evaluate(() => globalThis.__copied),
    rawText,
    "Raw copying lost line breaks",
  );
  await page.setViewportSize({ width: 1280, height: 800 });
}
