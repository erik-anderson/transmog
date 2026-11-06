import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerDocking(page, workspace, root) {
  const divider = page.getByRole("separator", { name: "Resize debugger", exact: true });
  const dock = workspace.getByRole("button", { name: "Dock", exact: true });
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
    await button.press("Escape");
    await menu.waitFor({ state: "hidden" });
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
    await page.setViewportSize({ width, height: 900 });
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
    await checkMenu("Dock", "#debugger-placement");
    await checkMenu("View", "#debugger-view");
  }
  await page.setViewportSize({ width: 1440, height: 900 });
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
}
