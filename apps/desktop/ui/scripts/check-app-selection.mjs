import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkAppSelection(page, root) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.sessions.length === 4,
  );
  const traffic = page.locator("traffic-workspace"),
    cells = traffic.locator('tr[data-session-id] td[data-column-id="host"]');
  await cells.first().click({ position: { x: 20, y: 12 } });
  await cells.nth(3).click({ modifiers: ["Shift"], position: { x: 60, y: 12 } });
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedTrafficCount),
    4,
  );
  assert.equal(
    await page.evaluate(() => getSelection().toString()),
    "",
    "Shift-click selected row text",
  );
  const first = await cells.first().boundingBox(),
    last = await cells.nth(3).boundingBox();
  await page.mouse.move(first.x + 20, first.y + 12);
  await page.mouse.down();
  await page.mouse.move(last.x + 60, last.y + 12, { steps: 8 });
  await page.mouse.up();
  assert.equal(
    await page.evaluate(() => getSelection().toString()),
    "",
    "Dragging selected row text",
  );
  await cells.nth(3).click();
  const header = traffic
    .locator('message-inspector[side="request"] .headers-table tbody td')
    .first();
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.selectedDetail?.id === "first",
  );
  await header.waitFor();
  await header.dblclick({ position: { x: 20, y: 12 } });
  assert.ok(
    (await page.evaluate(() => getSelection().toString())).includes("Accept"),
    "Header evidence cannot be selected",
  );
  const body = traffic.locator('message-inspector[side="response"] .body-preview');
  await body.waitFor();
  await body.dblclick({ position: { x: 20, y: 12 } });
  assert.ok(
    (await page.evaluate(() => getSelection().toString())).length,
    "Body evidence cannot be selected",
  );
  await traffic.locator(".selected-url").click();
  const url = traffic.locator("#selected-url-full .full-value");
  await url.dblclick({ position: { x: 20, y: 12 } });
  assert.ok(
    (await page.evaluate(() => getSelection().toString())).length,
    "Full URL cannot be selected",
  );
  await page.keyboard.press("Escape");
  const input = traffic.getByLabel("Filter or search traffic", { exact: true });
  await input.fill("example");
  await input.press("Control+a");
  await input.press("Backspace");
  assert.equal(await input.inputValue(), "");
  await page.evaluate(() => {
    getSelection().removeAllRanges();
    document.querySelector("app-shell").traffic.clearTrafficSelection();
  });
  await traffic.locator(".traffic-table").focus();
  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+ArrowDown");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedTrafficCount),
    2,
  );
  assert.equal(await page.evaluate(() => getSelection().toString()), "");
  await page.keyboard.press("Escape");
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/app-selection.png") });
}
