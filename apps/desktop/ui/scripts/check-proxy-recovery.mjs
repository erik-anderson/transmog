import assert from "node:assert/strict";
import { resolve } from "node:path";

async function checkManualCertificateTrust(page, workspace, root) {
  const shell = page.locator("app-shell");
  const routing = shell.getByRole("combobox", { name: "Routing for next start" });
  const notice = shell.locator(".notice");
  await workspace.evaluate(async (settings) => {
    const fixture = globalThis.__workspaceFixture;
    fixture.caBootstrap = { ownedCaTrusted: false };
    await settings.refreshCertificateState();
  });
  await notice
    .getByText(/trust Transmog’s interception certificate for your Windows account/)
    .waitFor();
  const system = shell.getByRole("button", { name: "System proxy, routing and status" });
  await system.focus();
  await page.keyboard.press("Enter");
  await routing.selectOption("manual");
  const manual = shell.getByRole("button", { name: "Manual proxy, routing and status" });
  await manual.waitFor();
  await notice.waitFor({ state: "hidden" });
  await routing.selectOption("system");
  await notice.getByText(/for your Windows account/).waitFor();
  await routing.selectOption("manual");
  await manual.waitFor();
  await page.keyboard.press("Escape");
  await workspace.getByText("Interception certificate", { exact: true }).click();
  const copy = workspace.getByRole("button", { name: "Copy public certificate path" });
  await copy.click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "fixture.pem");
  const trust = workspace.getByRole("button", { name: "Trust for my Windows account…" });
  await page.evaluate(() => (globalThis.__workspaceFixture.caTrustError = true));
  await trust.click();
  await workspace.getByText(/Windows certificate trust could not be set up/).waitFor();
  await copy.waitFor();
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.caTrustError = false;
    fixture.holdCommand = "desktop_bootstrap";
    fixture.finishHeldCommand = null;
  });
  await trust.click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await workspace.evaluate((settings) => settings.refreshStatus());
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.caBootstrap.hostRestorePending = true;
    fixture.finishHeldCommand();
  });
  await workspace.getByText("Interception certificate trusted for this Windows account.").waitFor();
  await workspace
    .getByRole("button", { name: "Trusted for this Windows account" })
    .waitFor({ timeout: 3000 });
  assert.equal(await workspace.evaluate((settings) => settings.hostRecoveryPending), false);
  await page.evaluate(() => delete globalThis.__workspaceFixture.caBootstrap.hostRestorePending);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/manual-proxy-trust.png") });
  await workspace.evaluate(async (settings) => {
    const fixture = globalThis.__workspaceFixture;
    fixture.savedProduct ??= await window.__TAURI_INTERNALS__.invoke("product_state");
    fixture.savedProduct.preferences.decryptHttps = true;
    fixture.trustedCertificate = fixture.caBootstrap;
    fixture.hostRestorePending = true;
    fixture.caBootstrap = {
      caFilesPresent: false,
      caFilesExist: false,
      ownedCaSha256: null,
      ownedCaTrusted: false,
      hostRestorePending: true,
    };
    fixture.startsBeforeCertificateBlock = fixture.calls.start_proxy ?? 0;
    settings.showSettingsSection("preferences");
    settings.showNotice(
      "Windows proxy recovery required",
      "A previous run left recovery pending.",
      "Restore settings",
      "recover-proxy",
    );
  });
  await shell.locator(".top-actions").getByRole("button", { name: "Start proxy" }).click();
  await workspace
    .locator(".connection-panel .operation-status")
    .filter({ hasText: "Create an interception certificate" })
    .waitFor({ timeout: 3000 });
  await notice.getByText("Windows proxy recovery required").waitFor();
  await workspace.evaluate(async (settings) => {
    const fixture = globalThis.__workspaceFixture;
    fixture.hostRestorePending = false;
    fixture.caBootstrap = fixture.trustedCertificate;
    delete fixture.trustedCertificate;
    await settings.refreshCertificateState();
    settings.dismissNotice();
  });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.start_proxy ?? 0),
    await page.evaluate(() => globalThis.__workspaceFixture.startsBeforeCertificateBlock),
  );
  await workspace.locator(".connection-panel .operation-status").waitFor({ state: "hidden" });
  await manual.click();
  await page.evaluate(() => (globalThis.__workspaceFixture.routingSaveError = true));
  await routing.selectOption("system");
  await shell.getByText(/Proxy routing could not be saved/).waitFor();
  assert.equal(await routing.inputValue(), "manual");
  await page.evaluate(() => (globalThis.__workspaceFixture.routingSaveError = false));
  await routing.selectOption("system");
  await system.waitFor();
  await page.keyboard.press("Escape");
  await workspace.getByText("Interception certificate", { exact: true }).click();
}

async function checkManualRouting(page, workspace, root) {
  const shell = page.locator("app-shell");
  const tools = shell.getByRole("button", { name: /routing and status/ });
  const routing = shell.getByRole("combobox", { name: "Routing for next start" });
  await tools.click();
  assert.equal(await routing.inputValue(), "system");
  await workspace.evaluate((settings) => {
    globalThis.__workspaceFixture.holdSettingsSaveReply = true;
    void settings.saveSettings(new Event("submit", { cancelable: true }), "connection");
  });
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseSettingsSaveReply);
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.holdCommand = "save_proxy_routing";
    fixture.finishHeldCommand = null;
    fixture.startCallsBeforeRoutingSave = fixture.calls.start_proxy ?? 0;
  });
  await routing.selectOption("manual");
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  assert.equal(
    await shell
      .locator(".top-actions proxy-toggle")
      .getByRole("button", { name: "Start proxy" })
      .isDisabled(),
    true,
  );
  await workspace.evaluate((settings) => settings.startProxy());
  assert.equal(
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      return fixture.calls.start_proxy ?? 0;
    }),
    await page.evaluate(() => globalThis.__workspaceFixture.startCallsBeforeRoutingSave),
  );
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(
    () => document.querySelector("app-shell").routingLabel === "Manual proxy",
  );
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.holdSettingsSaveReply = false;
    fixture.releaseSettingsSaveReply();
  });
  await page.waitForFunction(() => !document.querySelector("app-shell").settings.settingsBusy);
  assert.equal(await routing.inputValue(), "manual");
  assert.equal(await tools.textContent(), "Manual proxy");
  await page.keyboard.press("Escape");
  await workspace.evaluate((settings) => {
    const fixture = globalThis.__workspaceFixture;
    fixture.listenerAddress = "0.0.0.0:8888";
    fixture.manualRecoveryCalls = fixture.calls.recover_windows_proxy ?? 0;
    settings.showNotice(
      "Windows proxy recovery required",
      "A previous run left recovery pending.",
      "Restore settings",
      "recover-proxy",
    );
    return settings.startProxy();
  });
  await workspace.locator(".operation-status").filter({ hasText: "127.0.0.1:8888" }).waitFor();
  await shell.locator(".notice").getByRole("button", { name: "Restore settings" }).waitFor();
  await workspace.getByRole("button", { name: "Recover host settings" }).waitFor();
  await shell.getByRole("button", { name: "Copy proxy address" }).click();
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "127.0.0.1:8888");
  await tools.click();
  await routing.selectOption("system");
  await page.waitForFunction(() => document.querySelector("app-shell").configureSystemProxy);
  assert.equal(await tools.textContent(), "Manual proxy");
  await workspace.getByText(/From another device, use this computer’s LAN address/).waitFor();
  const viewport = page.viewportSize();
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/manual-proxy.png") });
  await page.setViewportSize(viewport);
  await page.keyboard.press("Escape");
  await workspace.evaluate((settings) => settings.stopProxy());
  await page.evaluate(() => (globalThis.__workspaceFixture.listenerAddress = undefined));
  assert.equal(await tools.textContent(), "System proxy");
  assert.equal(
    await page.evaluate(() => {
      const fixture = globalThis.__workspaceFixture;
      return (fixture.calls.recover_windows_proxy ?? 0) - fixture.manualRecoveryCalls;
    }),
    0,
  );
}

export async function checkProxyRecovery(page, root) {
  await page.locator('app-shell a[data-view="settings"]').click();
  const workspace = page.locator("settings-workspace");
  await workspace.evaluate(async (settings) => {
    await settings.ready;
    settings.showSettingsSection("connection");
  });
  await checkManualCertificateTrust(page, workspace, root);
  await workspace.evaluate(async (settings) => {
    await settings.ready;
    settings.showSettingsSection("connection");
    settings.hostRecoveryPending = true;
    const fixture = globalThis.__workspaceFixture;
    fixture.hostRestorePending = true;
    fixture.recoveryOriginalLifecycle = fixture.lifecycle;
    fixture.recoveryOriginalRoutingMode = fixture.runtimeRoutingMode;
    fixture.lifecycle = "stopped";
    fixture.holdProxyRecovery = true;
    fixture.failProxyRecovery = false;
    fixture.recoveryCallsBefore = fixture.calls.recover_windows_proxy ?? 0;
  });
  await checkManualRouting(page, workspace, root);
  const recover = workspace.getByRole("button", { name: "Recover host settings", exact: true });
  await workspace.evaluate((settings) => (settings.hostRecoveryPending = true));
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
  assert.equal(await workspace.locator("#preferences-form").isVisible(), true);
  await page
    .locator("app-shell .notice")
    .getByText("Windows proxy recovery required")
    .waitFor({ state: "hidden", timeout: 5000 });
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
    fixture.runtimeRoutingMode = fixture.recoveryOriginalRoutingMode;
    fixture.holdProxyRecovery = false;
    await document.querySelector("app-shell").refreshStatus();
  });
  await page.locator('app-shell a[data-view="traffic"]').click();
}
