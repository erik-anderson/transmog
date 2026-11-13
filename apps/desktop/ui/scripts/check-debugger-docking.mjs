import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerDocking(page, workspace, root) {
  const requestSplit = await page.locator("traffic-workspace").evaluate(async (element) => {
    const preferences = element.preferences;
    const expanded = element.inspectorExpanded;
    const expandedSplit = element.expandedRequestSplit;
    try {
      element.preferences = { ...preferences, requestSplit: 65 };
      element.inspectorExpanded = false;
      await element.$whenRendered();
      const normal = element.querySelector(
        'pane-divider[label="Resize request and response inspectors"]',
      ).value;
      element.expandedRequestSplit = 55;
      element.inspectorExpanded = true;
      await element.$whenRendered();
      return [
        normal,
        element.querySelector('pane-divider[label="Resize request and response inspectors"]').value,
      ];
    } finally {
      element.preferences = preferences;
      element.inspectorExpanded = expanded;
      element.expandedRequestSplit = expandedSplit;
      await element.$whenRendered();
    }
  });
  assert.deepEqual(
    requestSplit,
    ["65", "55"],
    "The request/response divider must use the current normal or expanded split",
  );
  const divider = page.getByRole("separator", { name: "Resize debugger", exact: true });
  const dock = workspace.getByRole("button", { name: "Dock", exact: true });
  for (const [action, title] of [
    [/^Open source/u, "Open source"],
    [/^Search sources/u, "Search sources"],
  ]) {
    for (const escape of [true, false]) {
      await workspace.getByRole("button", { name: "View", exact: true }).click();
      await workspace.locator("#debugger-view").getByRole("button", { name: action }).click();
      const dialog = workspace.getByRole("dialog", { name: title, exact: true });
      await dialog.waitFor();
      assert.equal(
        await workspace.locator("#debugger-view").isVisible(),
        false,
        "Opening source navigation must close its menu",
      );
      if (escape)
        await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
      else
        await dialog.getByRole("button", { name: "Close source navigation", exact: true }).click();
      await dialog.waitFor({ state: "hidden" });
      assert.ok(
        await workspace.evaluate(
          (element) =>
            element.editor.hasTextFocus() ||
            element.shadowRoot.activeElement === element.viewMenuButton,
        ),
        "Closing source navigation restores focus to a visible source control",
      );
    }
  }
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
        .debuggerWorkerOptions.length > 0,
  );
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  await prompt.press("Control+p");
  await workspace.getByRole("textbox", { name: "Source query", exact: true }).press("Escape");
  assert.ok(
    await prompt.evaluate((element) => element.getRootNode().activeElement === element),
    "Canceling source navigation preserves Console focus",
  );
  assert.equal(
    await workspace.getByRole("combobox", { name: "Debugger worker", exact: true }).inputValue(),
    await workspace.evaluate((element) => element.debuggerTarget),
    "Worker selector must show the active worker",
  );
  const checkMenu = async (name, id) => {
    const consoleVisible = await workspace.locator(".console").isVisible();
    const button = workspace.getByRole("button", { name, exact: true });
    await button.click();
    const menu = workspace.locator(id);
    await menu.waitFor();
    const trigger = await button.boundingBox();
    const popup = await menu.boundingBox();
    const gap = Math.min(
      Math.abs(popup.y - trigger.y - trigger.height),
      Math.abs(trigger.y - popup.y - popup.height),
    );
    assert.ok(gap <= 12, `${name} menu must anchor beside its trigger, gap ${gap}`);
    assert.ok(popup.x < trigger.x + trigger.width && popup.x + popup.width > trigger.x);
    await page.screenshot({
      path: resolve(
        root,
        `../../../target/ui-check/debugger-${name.toLowerCase()}-anchored-${page.viewportSize().width}.png`,
      ),
    });
    await menu.getByRole("button").first().focus();
    await page.keyboard.down("Escape");
    await menu.waitFor({ state: "hidden" });
    await page.keyboard.down("Escape");
    await page.keyboard.up("Escape");
    assert.ok(
      await button.evaluate((element) => element.getRootNode().activeElement === element),
      "Closing a menu restores focus to its trigger",
    );
    assert.equal(
      await workspace.locator(".console").isVisible(),
      consoleVisible,
      "Escape dismisses the menu before changing the Console",
    );
  };
  const drag = async (dx, dy) => {
    const box = await divider.boundingBox();
    const x = box.x + box.width / 2,
      y = box.y + box.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + dx, y + dy, { steps: 5 });
    await page.mouse.up();
  };
  for (const width of [1440, 800]) {
    await page.setViewportSize({ width, height: width === 800 ? 600 : 900 });
    await dock.click();
    await workspace.getByRole("button", { name: "Dock to right", exact: true }).click();
    assert.equal(await divider.getAttribute("aria-orientation"), "vertical");
    assert.equal(
      await divider.evaluate((element) => getComputedStyle(element).cursor),
      "col-resize",
    );
    await divider.press("Home");
    let before = await workspace.boundingBox();
    await drag(0, 25);
    assert.ok(
      Math.abs((await workspace.boundingBox()).width - before.width) < 2,
      "Vertical motion must not resize a right dock",
    );
    await drag(-50, 0);
    const growth = (await workspace.boundingBox()).width - before.width;
    assert.ok(
      growth >= 25 && growth <= 75,
      `A right dock follows horizontal motion without jumping, grew ${growth}`,
    );
    await divider.press("Home");
    const restored = await workspace.boundingBox();
    const handle = await divider.boundingBox();
    const x = handle.x + handle.width / 2,
      y = handle.y + handle.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x - 30, y);
    await divider.press("Escape");
    await page.mouse.up();
    assert.ok(
      Math.abs((await workspace.boundingBox()).width - restored.width) < 2,
      "Canceling a resize restores its original width",
    );
    await checkMenu("Dock", "#debugger-placement");
    await checkMenu("View", "#debugger-view");
    for (let step = 0; step < 5; step++) await divider.press("Shift+ArrowRight");
    const edge = await workspace.boundingBox();
    assert.ok(edge.x + edge.width <= width + 1, "Resizing must keep the debugger inside the app");
    await drag(-40, 0);
    const recovered = (await workspace.boundingBox()).width - edge.width;
    assert.ok(
      recovered > 15,
      `Dragging away from the minimum dock width must respond immediately, grew ${recovered}`,
    );
    await divider.press("Home");
  }
  await page.setViewportSize({ width: 1440, height: 600 });
  await dock.click();
  await workspace.getByRole("button", { name: "Dock to bottom", exact: true }).click();
  assert.equal(await divider.getAttribute("aria-orientation"), "horizontal");
  assert.equal(await divider.evaluate((element) => getComputedStyle(element).cursor), "row-resize");
  await divider.press("Home");
  const before = await workspace.boundingBox();
  await drag(25, 0);
  assert.ok(Math.abs((await workspace.boundingBox()).height - before.height) < 2);
  await drag(0, -35);
  const growth = (await workspace.boundingBox()).height - before.height;
  assert.ok(growth >= 20 && growth <= 55, `A bottom dock follows vertical motion, grew ${growth}`);
  await divider.press("Home");
  await checkMenu("Dock", "#debugger-placement");
  await checkMenu("View", "#debugger-view");
  for (let step = 0; step < 5; step++) await divider.press("Shift+ArrowDown");
  const edge = await workspace.boundingBox();
  assert.ok(edge.y + edge.height <= 601, "Resizing must keep the bottom dock inside the app");
  await drag(0, -35);
  assert.ok(
    (await workspace.boundingBox()).height - edge.height > 15,
    "Dragging away from the minimum dock height must respond immediately",
  );
  await divider.press("Home");
  await page.setViewportSize({ width: 1440, height: 900 });
}
