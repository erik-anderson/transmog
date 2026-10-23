import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkInspectorShortcutFocus(page) {
  const traffic = page.locator("traffic-workspace");
  await page.waitForFunction(() => {
    const traffic = document.querySelector("app-shell").traffic;
    return traffic.selectedDetail?.id === traffic.selectedSessionId;
  });
  const before = await traffic.evaluate((element) => ({
    id: element.selectedSessionId,
    selection: [...element.trafficSelection.ids],
    preferences: element.preferences,
    request: element.requestMessage.mode,
    response: element.responseMessage.mode,
  }));
  const current = traffic.locator("tr[data-current]");
  const controls = [
    ["Traffic grid", traffic.getByRole("grid", { name: "Traffic entries", exact: true })],
    ["Traffic row", current],
    ["request button", current.locator(".session-link")],
    ["URL button", traffic.locator(".selected-url")],
    ["Details button", traffic.getByRole("button", { name: "Details…", exact: true })],
    ["Expand button", traffic.getByRole("button", { name: "Expand inspector", exact: true })],
  ];
  const tab = traffic.locator(".message-tabs button:visible").first();
  if (await tab.count()) controls.push(["message tab", tab]);
  const streamButton = current.locator(".stream-identity");
  if (await streamButton.count()) controls.push(["stream button", streamButton]);
  const disclosure = traffic.locator(".details-pane summary:visible").first();
  if (await disclosure.count()) controls.push(["inspector disclosure", disclosure]);
  controls.push(["app header button", page.locator(".topbar button:visible").first()]);
  const navigation = page.locator('a[data-view="traffic"]:visible');
  if (await navigation.count()) controls.push(["Traffic navigation", navigation]);
  for (const [name, control] of controls) {
    await control.focus();
    assert.equal(
      await control.evaluate((node) => node === node.getRootNode().activeElement),
      true,
      `${name} did not receive focus`,
    );
    await page.keyboard.press("Control+Shift+Enter");
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      false,
      `Inspector did not expand from ${name}`,
    );
    assert.equal(
      await traffic.locator("dialog[open],[popover]:popover-open").count(),
      0,
      `Shortcut activated ${name}`,
    );
    await page.keyboard.press("Control+Shift+Enter");
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      true,
      `Inspector did not restore from ${name}`,
    );
    assert.equal(
      await traffic
        .getByRole("grid", { name: "Traffic entries", exact: true })
        .evaluate((node) => node === node.getRootNode().activeElement),
      true,
      "Restore lost Traffic focus",
    );
    assert.deepEqual(
      await traffic.evaluate((element) => ({
        id: element.selectedSessionId,
        selection: [...element.trafficSelection.ids],
        preferences: element.preferences,
        request: element.requestMessage.mode,
        response: element.responseMessage.mode,
      })),
      before,
      `Shortcut changed the selected evidence from ${name}`,
    );
  }
  // Window commands also work after focus leaves the app's controls.
  await traffic
    .getByRole("grid", { name: "Traffic entries", exact: true })
    .evaluate((node) => node.blur());
  await page.keyboard.press("Control+Shift+Enter");
  assert.equal(
    await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
    false,
    "Inspector shortcut required a focused app control",
  );
  await page.keyboard.press("Control+Shift+Enter");
  // A held shortcut must toggle once, including after focus moves to Restore.
  await page.keyboard.down("Control");
  await page.keyboard.down("Shift");
  try {
    await page.keyboard.down("Enter");
    await page.keyboard.down("Enter");
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      false,
      "Held shortcut toggled twice",
    );
  } finally {
    await page.keyboard.up("Enter");
    await page.keyboard.up("Shift");
    await page.keyboard.up("Control");
  }
  await page.keyboard.press("Control+Shift+Enter");
}

export async function checkWindowShortcuts(page, root) {
  const traffic = page.locator("traffic-workspace"),
    filter = traffic.getByRole("searchbox"),
    view = traffic.getByRole("button", { name: "View", exact: true });
  const count = (command) =>
    page.evaluate((command) => globalThis.__workspaceFixture.calls[command] ?? 0, command);
  for (const viewer of [false, true]) {
    await page.goto("https://workspace.test/" + (viewer ? "?viewer" : ""));
    await traffic.locator(".session-link").first().waitFor();
    // No focused app control is required for a window command.
    await page.keyboard.press("Control+n");
    await page.waitForFunction(() => globalThis.__workspaceFixture.openedViewer);
    assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.openedViewer), []);
    const opens = await count("open_trace_viewer");
    await filter.evaluate((node) =>
      node.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "n",
          ctrlKey: true,
          repeat: true,
          bubbles: true,
          composed: true,
          cancelable: true,
        }),
      ),
    );
    for (const shortcut of ["Control+Shift+n", "Control+Alt+n"])
      await page.keyboard.press(shortcut);
    assert.equal(
      await count("open_trace_viewer"),
      opens,
      "Repeated or modified shortcut opened a viewer",
    );
    await view.click();
    await page.keyboard.press("Control+f");
    assert.equal(await filter.evaluate((node) => node === node.getRootNode().activeElement), true);
    assert.equal(
      await traffic.locator("#table-settings").isVisible(),
      false,
      "Filter shortcut left the flyout open",
    );
    await traffic.locator(".session-link").first().click();
    const inspected = await traffic.evaluate((element) => element.selectedSessionId);
    await checkInspectorShortcutFocus(page);
    await view.click();
    const menuOpens = await count("open_trace_viewer");
    await traffic.getByRole("button", { name: "Open new viewer window", exact: true }).focus();
    await page.keyboard.press("Control+Shift+Enter");
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      true,
      "Inspector shortcut overrode an open menu",
    );
    assert.equal(
      await traffic.locator("#table-settings").isVisible(),
      true,
      "Inspector shortcut dismissed an open menu",
    );
    await page.keyboard.press("Enter");
    assert.equal(
      await count("open_trace_viewer"),
      menuOpens + 1,
      "Open menu lost its Enter action",
    );
    await traffic.getByRole("button", { name: "Expand inspector", exact: true }).click();
    await page.keyboard.press("Control+s");
    await traffic.locator(".trace-save-dialog").waitFor();
    await page.keyboard.press("Escape");
    assert.equal(
      await traffic.evaluate((element) => element.inspectorExpanded),
      true,
      "Dismissing Save copy collapsed the inspector",
    );
    assert.equal(
      await traffic
        .getByRole("button", { name: "Restore", exact: true })
        .evaluate((element) => element === element.getRootNode().activeElement),
      true,
      "Save copy dismissal lost focus in the expanded inspector",
    );
    await page.keyboard.press("Control+f");
    assert.equal(
      await filter.isVisible(),
      true,
      "Filter shortcut targeted the hidden toolbar in an expanded inspector",
    );
    assert.equal(
      await filter.evaluate((node) => node === node.getRootNode().activeElement),
      true,
      "Filter shortcut did not focus the restored search field",
    );
    assert.equal(
      await traffic.evaluate((element) => element.selectedSessionId),
      inspected,
      "Filter shortcut lost the inspected request",
    );
    await filter.fill("example");
    await view.focus();
    await page.keyboard.press("Control+f");
    assert.deepEqual(
      await filter.evaluate((node) => [node.selectionStart, node.selectionEnd]),
      [0, 7],
    );
    await page.keyboard.down("Control");
    await page.keyboard.down("Shift");
    await filter.evaluate((node) =>
      node.addEventListener("keydown", (event) => event.preventDefault(), { once: true }),
    );
    try {
      await page.keyboard.press("Enter");
    } finally {
      await page.keyboard.up("Shift");
      await page.keyboard.up("Control");
    }
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      true,
      "Inspector shortcut overrode a text field key handler",
    );
    await filter.evaluate((node) =>
      node.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          ctrlKey: true,
          shiftKey: true,
          isComposing: true,
          bubbles: true,
          composed: true,
          cancelable: true,
        }),
      ),
    );
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      true,
      "Inspector shortcut interrupted text composition",
    );
    await page.keyboard.press("Control+Shift+Enter");
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      false,
      "Unclaimed shortcut did not expand from the search field",
    );
    await page.keyboard.press("Control+Shift+Enter");
    await traffic.getByRole("button", { name: "Expand inspector", exact: true }).click();
    await page.keyboard.press("Control+Shift+f");
    await page.waitForFunction(
      () => document.querySelector("app-shell").traffic.trafficSearchKind === "contents",
    );
    assert.equal(await filter.inputValue(), "example");
    assert.equal(
      await filter.isVisible(),
      true,
      "Content search shortcut targeted the hidden expanded toolbar",
    );
    assert.equal(await filter.evaluate((node) => node === node.getRootNode().activeElement), true);
    await page.keyboard.press("Control+s");
    await traffic.locator(".trace-save-dialog").waitFor();
    const beforeModal = {
      open: await count("open_trace_viewer"),
      pick: await count("pick_trace_path"),
      close: await count("close_trace_window"),
    };
    for (const shortcut of [
      "Control+n",
      "Control+o",
      "Control+f",
      "Control+w",
      "Control+,",
      "Control+Shift+Enter",
    ])
      await page.keyboard.press(shortcut);
    assert.deepEqual(
      {
        open: await count("open_trace_viewer"),
        pick: await count("pick_trace_path"),
        close: await count("close_trace_window"),
      },
      beforeModal,
    );
    assert.equal(await traffic.locator(".trace-save-dialog").isVisible(), true);
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      true,
      "Inspector shortcut ran behind a dialog",
    );
    await page.keyboard.press("Escape");
    // A canceled picker is harmless and cannot be opened twice while pending.
    await page.evaluate(() => (globalThis.__workspaceFixture.deferTracePicker = true));
    await page.keyboard.press("Control+o");
    await page.waitForFunction(() => globalThis.__workspaceFixture.releaseTracePicker);
    const picks = await count("pick_trace_path");
    await page.keyboard.press("Control+o");
    assert.equal(await count("pick_trace_path"), picks);
    await page.evaluate(() => {
      globalThis.__workspaceFixture.deferTracePicker = false;
      globalThis.__workspaceFixture.releaseTracePicker();
    });
    await page.waitForFunction(() => !document.querySelector("app-shell").traffic.pickingTrace);
    await page.locator('a[data-view="composer"]').click();
    await page.waitForFunction(() => document.querySelector("app-shell").activeView === "composer");
    await page.keyboard.press("Control+Shift+Enter");
    assert.equal(
      await traffic.evaluate((element) => element.inspectorExpanded),
      false,
      "Inspector shortcut ran in another workspace",
    );
    await page.keyboard.press("Control+o");
    await page.waitForFunction(
      () =>
        document.querySelector("app-shell").activeView === "traffic" &&
        !document.querySelector("app-shell").traffic.pickingTrace,
    );
    assert.equal(await count("pick_trace_path"), picks + 1);
    await page.evaluate(
      () => (globalThis.__workspaceFixture.pickedTrace = "C:/captures/shortcut.saz"),
    );
    await page.keyboard.press("Control+o");
    await page.waitForFunction(
      () =>
        globalThis.__workspaceFixture.lastImport?.path === "C:/captures/shortcut.saz" &&
        !document.querySelector("app-shell").traffic.pickingTrace,
    );
    await page.keyboard.press("Control+w");
    assert.equal(
      await count("close_trace_window"),
      viewer ? 1 : 0,
      "Close shortcut did not respect the window role",
    );
    await page.keyboard.press("Control+,");
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").activeView),
      viewer ? "traffic" : "settings",
    );
    if (!viewer) {
      // Form saves must still belong to Settings; Traffic cannot capture them.
      const reviews = await count("review_traffic_export");
      await page.keyboard.press("Control+s");
      assert.equal(await count("review_traffic_export"), reviews);
      await page.locator('a[data-view="traffic"]').click();
    }
    // A child that handles its own Find must win over the application shortcut.
    await view.focus();
    await page.keyboard.down("Control");
    await view.evaluate((node) =>
      node.addEventListener("keydown", (event) => event.preventDefault(), { once: true }),
    );
    await page.keyboard.press("f");
    await page.keyboard.up("Control");
    assert.equal(await view.evaluate((node) => node === node.getRootNode().activeElement), true);
  }
  await page.goto("https://workspace.test/");
  await traffic.locator(".session-link").first().click();
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await checkInspectorShortcutFocus(page);
    await traffic.getByRole("button", { name: "Expand inspector", exact: true }).click();
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/inspector-shortcut-${width}.png`),
    });
    await page.keyboard.press("Control+Shift+Enter");
    await view.click();
    const button = traffic.getByRole("button", { name: "Open new viewer window", exact: true });
    const box = await button.boundingBox();
    assert.ok(
      box.x >= 0 && box.y >= 0 && box.x + box.width <= width && box.y + box.height <= height,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/view-shortcuts-${width}.png`),
    });
    const opens = await count("open_trace_viewer");
    await button.click();
    assert.equal(await count("open_trace_viewer"), opens + 1);
    assert.equal(await view.evaluate((node) => node === node.getRootNode().activeElement), true);
  }
  await page.emulateMedia({ forcedColors: "active" });
  await view.click();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/view-shortcuts-forced-colors.png"),
  });
  await page.keyboard.press("Escape");
  await page.emulateMedia({ forcedColors: "none" });
  try {
    await page.evaluate(() => (document.documentElement.style.zoom = "2"));
    await view.click();
    const box = await traffic
      .getByRole("button", { name: "Open new viewer window", exact: true })
      .boundingBox();
    assert.ok(
      box.x >= 0 && box.y >= 0 && box.x + box.width <= 800 && box.y + box.height <= 600,
      "New viewer action was clipped at 200% zoom",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/view-shortcuts-zoom.png"),
    });
    await page.keyboard.press("Escape");
  } finally {
    await page.evaluate(() => (document.documentElement.style.zoom = ""));
  }
  await page.setViewportSize({ width: 1280, height: 800 });
}
