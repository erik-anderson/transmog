import assert from "node:assert/strict";
import { resolve } from "node:path";
export async function checkScriptDebugger(page, root) {
  const editor = page.locator("script-editor");
  await page.waitForFunction(
    () => document.querySelector("app-shell").shadowRoot.querySelector("script-editor").scriptReady,
  );
  await editor.locator(".debug-options>summary").click();
  await editor.getByRole("textbox", { name: "Breakpoints before initialization" }).fill("2, 200");
  await editor.getByRole("button", { name: "Debug test", exact: true }).click();
  const panel = editor.locator("script-debugger");
  await panel.getByRole("button", { name: /^Continue/ }).waitFor();
  await page.waitForFunction(
    () => document.querySelector("app-shell").shadowRoot.querySelector("script-debugger")?.paused,
  );
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.debugTestRequest)).configuration
      .breakpoints.length,
    2,
  );
  assert.equal(await panel.locator("img,script").count(), 0);
  await panel.getByRole("button", { name: "local · nested", exact: true }).click();
  await panel.getByRole("cell", { name: "7", exact: true }).waitFor();
  await panel.getByRole("button", { name: "[Object]", exact: true }).click();
  await panel.getByRole("cell", { name: "key-99", exact: true }).waitFor();
  assert.equal(await panel.locator(".property-page tbody tr").count(), 100);
  assert.equal(await panel.locator("img,script").count(), 0);
  await panel.getByRole("button", { name: "Next properties", exact: true }).click();
  await panel.getByRole("cell", { name: "key-104", exact: true }).waitFor();
  assert.equal(await panel.locator(".property-page tbody tr").count(), 5);
  await panel.getByRole("button", { name: "Parent value", exact: true }).click();
  await panel.getByRole("cell", { name: "7", exact: true }).waitFor();
  await panel.getByRole("button", { name: "Toggle breakpoint at line 1", exact: true }).click();
  assert.equal(
    await panel
      .getByRole("button", { name: "Toggle breakpoint at line 1" })
      .getAttribute("aria-pressed"),
    "true",
  );
  for (const name of ["Step over", "Step into", "Step out"]) {
    await page.evaluate(() => {
      globalThis.__workspaceFixture.debugAckDelay = 250;
    });
    await panel.getByRole("button", { name: new RegExp(`^${name}`) }).click();
    await page.waitForFunction(
      () => document.querySelector("app-shell").shadowRoot.querySelector("script-debugger")?.paused,
    );
    await panel.getByRole("button", { name: "local · nested", exact: true }).waitFor();
  }
  const commands = await page.evaluate(() => globalThis.__workspaceFixture.debugCommands);
  assert.deepEqual(
    commands.filter((value) => value.kind === "resume").map((value) => value.action),
    ["over", "into", "out"],
  );
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await panel.scrollIntoViewIfNeeded();
    assert.ok(
      await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      "Debugger exceeded its pane width",
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/script-debugger-${name}.png`),
    });
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/script-debugger-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.evaluate(() =>
    document
      .querySelector("app-shell")
      .shadowRoot.querySelector("script-editor")
      .markScriptChanged(),
  );
  await panel.getByText(/Draft edits are newer/).waitFor();
  await panel.locator(".debugger").focus();
  await page.keyboard.press("F5");
  await panel.getByText("Callback completed", { exact: true }).waitFor();
  assert.equal(await panel.getByRole("button", { name: /^Continue/ }).isDisabled(), true);
  await panel.getByRole("button", { name: "Close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });
  assert.ok(await page.evaluate(() => globalThis.__workspaceFixture.debugClosedSession));
  await editor.getByRole("button", { name: "Save & enable", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("script-editor").scriptActive,
  );
  await editor.locator(".debug-live>summary").click();
  await editor.getByRole("button", { name: "Review applied revision", exact: true }).click();
  const reservation = editor.getByRole("region", { name: "Live debugger reservation" });
  await reservation.getByRole("button", { name: "Arm for 60 seconds", exact: true }).waitFor();
  await reservation
    .getByRole("textbox", { name: "Host (blank matches all)", exact: true })
    .fill("example.test");
  await reservation.getByRole("button", { name: "Arm for 60 seconds", exact: true }).click();
  await panel
    .getByText("Armed · waiting for the next matching exchange", { exact: true })
    .waitFor();
  assert.equal(
    (await page.evaluate(() => globalThis.__workspaceFixture.liveDebugRequest)).filter.host,
    "example.test",
  );
  await page.evaluate(() => globalThis.__workspaceFixture.claimDebugExchange());
  await panel.getByRole("button", { name: "local · nested", exact: true }).waitFor();
  await panel.getByText(/selected-live-exchange/).waitFor();
  await panel.getByRole("button", { name: /^Stop/ }).click();
  await panel.getByText("Stopped by its owner", { exact: true }).waitFor();
  await panel.getByRole("button", { name: "Close debugger", exact: true }).click();
  await panel.waitFor({ state: "detached" });
}
