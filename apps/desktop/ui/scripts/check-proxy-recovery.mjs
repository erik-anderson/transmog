import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkProxyRecovery(page, root) {
  await page.locator('app-shell a[data-view="settings"]').click();
  const workspace = page.locator("settings-workspace");
  await workspace.evaluate(async (settings) => {
    await settings.ready;
    settings.showSettingsSection("connection");
    settings.hostRecoveryPending = true;
    const fixture = globalThis.__workspaceFixture;
    fixture.hostRestorePending = true;
    fixture.recoveryOriginalLifecycle = fixture.lifecycle;
    fixture.lifecycle = "stopped";
    fixture.holdProxyRecovery = true;
    fixture.failProxyRecovery = false;
    fixture.recoveryCallsBefore = fixture.calls.recover_windows_proxy ?? 0;
  });
  const recover = workspace.getByRole("button", { name: "Recover host settings", exact: true });
  await recover.click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseProxyRecovery);
  assert.equal(await recover.isDisabled(), true);
  const viewport = page.viewportSize();
  await page.setViewportSize({ width: 960, height: 720 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/proxy-recovery-pending.png"),
  });
  await page.setViewportSize(viewport);
  await workspace.evaluate((settings) => void settings.recoverProxy());
  assert.equal(
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      return fixture.calls.recover_windows_proxy - fixture.recoveryCallsBefore;
    }),
    1,
    "A pending recovery must not submit a second command",
  );
  const preferences = workspace.getByRole("button", { name: "Preferences", exact: true });
  await preferences.click();
  await page.evaluate(() => globalThis.__workspaceFixture.releaseProxyRecovery());
  await page.waitForFunction(
    () =>
      !document.querySelector("app-shell").shadowRoot.querySelector("settings-workspace")
        .proxyPending,
  );
  assert.equal(
    await preferences.evaluate((node) => node === node.getRootNode().activeElement),
    true,
  );
  assert.equal(await workspace.locator(".preferences-form").isVisible(), true);
  await workspace.evaluate((settings) => {
    settings.showSettingsSection("connection");
    settings.hostRecoveryPending = true;
    const fixture = globalThis.__workspaceFixture;
    fixture.hostRestorePending = true;
    fixture.holdProxyRecovery = false;
    fixture.failProxyRecovery = true;
  });
  await recover.focus();
  await page.keyboard.press("Enter");
  await workspace.locator(".operation-status").filter({ hasText: "Recovery failed:" }).waitFor();
  await page.waitForFunction(() => {
    const settings = document
      .querySelector("app-shell")
      .shadowRoot.querySelector("settings-workspace");
    return !settings.proxyPending;
  });
  assert.equal(await recover.isEnabled(), true, "Failed recovery must remain available for retry");
  await page.evaluate(() => (globalThis.__workspaceFixture.failProxyRecovery = false));
  await recover.click();
  await recover.waitFor({ state: "hidden" });
  await page.evaluate(async () => {
    const fixture = globalThis.__workspaceFixture;
    fixture.lifecycle = fixture.recoveryOriginalLifecycle;
    fixture.holdProxyRecovery = false;
    await document.querySelector("app-shell").refreshStatus();
  });
  await page.locator('app-shell a[data-view="traffic"]').click();
}
