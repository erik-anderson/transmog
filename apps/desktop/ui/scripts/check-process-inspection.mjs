import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkProcessInspection(page, root) {
  await checkStatusCompletionOrder(page);
  await checkSettingsNavigationOwnership(page);
  const settings = page.locator("settings-workspace");
  const panel = settings.locator("process-inspection-settings");
  const tabs = settings.locator(".settings-tabs");
  const preferences = tabs.getByRole("button", { name: "Preferences", exact: true });
  const connection = tabs.getByRole("button", { name: "Connection", exact: true });
  const certificateSummary = settings.locator(".interception-certificate > summary");
  const mode = panel.getByRole("combobox", { name: "Inspect traffic from", exact: true });
  const name = panel.getByLabel("Executable name", { exact: true });
  const save = settings
    .locator("#connection-form")
    .getByRole("button", { name: "Save settings", exact: true });
  const fixture = async (values) =>
    page.evaluate((values) => Object.assign(globalThis.__workspaceFixture, values), values);
  const idle = () =>
    page.waitForFunction(() => {
      const settings = document.querySelector("app-shell").settings;
      return !settings.settingsBusy && !settings.querySelector(".process-settings").disabled;
    });
  const saved = () =>
    page.evaluate(() => globalThis.__workspaceFixture.savedProduct?.preferences.processInspection);
  const original = await page.evaluate(async () => ({
    product: await window.__TAURI_INTERNALS__.invoke("product_state"),
    bootstrap: globalThis.__workspaceFixture.caBootstrap,
    lifecycle: globalThis.__workspaceFixture.lifecycle,
    capture: globalThis.__workspaceFixture.capture,
    runtimeDecrypt: globalThis.__workspaceFixture.runtimeDecrypt,
    certificateDetailsOpen: document.querySelector("app-shell").settings.certificateDetailsOpen,
  }));
  try {
    await checkSettingsDrafts(page, settings, preferences, connection, fixture, idle);
    await connection.click();
    await settings.evaluate((element) => element.loadSettings("connection"));
    assert.equal(await mode.inputValue(), "all");
    assert.equal(await name.isVisible(), false);
    await mode.selectOption("only");
    if (await certificateSummary.evaluate((summary) => summary.parentElement.open)) {
      await certificateSummary.click();
    }
    await certificateSummary.press("Enter");
    await settings.getByLabel("CA private key", { exact: true }).waitFor();
    assert.equal(
      await certificateSummary.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
    await certificateSummary.press("Enter");
    assert.equal(
      await mode.inputValue(),
      "only",
      "Certificate navigation discarded the process draft",
    );
    await save.click();
    await panel.getByText("Add at least one process name, or choose All processes.").waitFor();
    assert.equal(await name.evaluate((input) => input.getRootNode().activeElement === input), true);
    await name.fill("C:\\Chrome\\chrome.exe");
    await name.press("Enter");
    await panel.getByText(/Enter an executable name/).waitFor();
    await name.fill("Chrome");
    await name.press("Enter");
    await panel.getByRole("button", { name: "Remove chrome.exe", exact: true }).waitFor();
    await name.fill("CHROME.EXE");
    await panel.getByRole("button", { name: "Add process", exact: true }).click();
    await panel.getByText(/already in this list/).waitFor();
    assert.equal(await panel.locator(".process-names li").count(), 1);
    await name.fill("");
    await panel.getByRole("button", { name: "Refresh suggestions", exact: true }).click();
    await page.waitForFunction(
      () =>
        document.querySelector("app-shell").settings.processInspectionEditor.processSuggestions
          .length === 3,
    );
    assert.deepEqual(
      await panel
        .locator("datalist option")
        .evaluateAll((options) => options.map((option) => option.value)),
      ["chrome.exe", "msedge.exe", "service.helper.exe"],
    );
    await mode.selectOption("except");
    assert.equal(await panel.locator(".process-names li").count(), 0);
    await name.fill("service.helper.exe");
    await name.press("Enter");
    await mode.selectOption("only");
    await panel.getByRole("button", { name: "Remove chrome.exe", exact: true }).waitFor();
    await fixture({ settingsSaveError: true });
    await save.click();
    await idle();
    await settings.getByText(/Settings save failed: Fixture settings persistence failed/).waitFor();
    assert.equal(await mode.inputValue(), "only");
    assert.equal(
      await panel.getByRole("button", { name: "Remove chrome.exe", exact: true }).count(),
      1,
    );
    await fixture({ settingsSaveError: false });
    await save.click();
    await idle();
    assert.deepEqual(await saved(), {
      mode: "only",
      include: ["chrome.exe"],
      exclude: ["service.helper.exe"],
    });
    await page.locator(".inspection-scope").getByText("1 process", { exact: true }).waitFor();
    // Another window changes the policy while this window is editing ordinary settings.
    await fixture({
      savedProduct: {
        ...original.product,
        preferences: {
          ...original.product.preferences,
          processInspection: { mode: "except", include: ["chrome.exe"], exclude: ["other.exe"] },
        },
      },
    });
    await preferences.click();
    await settings.getByLabel("Theme", { exact: true }).selectOption("dark");
    await settings
      .locator("#preferences-form")
      .getByRole("button", { name: "Save settings", exact: true })
      .click();
    await idle();
    assert.equal((await saved()).mode, "except");
    await connection.click();
    assert.equal(await mode.inputValue(), "only");
    await settings.evaluate((element) => element.loadSettings("connection"));
    assert.equal(await mode.inputValue(), "except");
    // A concurrent edit to the same policy retains the authored draft on conflict.
    await mode.selectOption("only");
    await fixture({
      savedProduct: {
        ...original.product,
        preferences: {
          ...original.product.preferences,
          processInspection: { mode: "only", include: ["different.exe"], exclude: [] },
        },
      },
    });
    await save.click();
    await idle();
    await settings.getByText(/Process inspection changed in another window/).waitFor();
    assert.equal(
      await panel.getByRole("button", { name: "Remove chrome.exe", exact: true }).count(),
      1,
    );
    await settings.getByRole("button", { name: "Revert changes", exact: true }).click();
    await idle();
    await panel.getByRole("button", { name: "Remove different.exe", exact: true }).click();
    assert.equal(await name.evaluate((input) => input.getRootNode().activeElement === input), true);
    await name.fill("msedge.exe");
    await name.press("Enter");
    await save.click();
    await idle();
    // Another writer saves tunnel mode while this Connection save holds an older snapshot.
    // The save must preserve the newer mode when its HTTPS field was untouched.
    await fixture({ holdProductRead: true, lifecycle: "stopped" });
    await settings.evaluate((element) => element.refreshStatus());
    await settings.evaluate((element) => {
      void element.saveSettings(new Event("submit", { cancelable: true }), "connection");
    });
    await page.waitForFunction(() => Boolean(globalThis.__workspaceFixture.releaseProductRead));
    assert.equal(await mode.isDisabled(), true);
    assert.equal(await name.isDisabled(), true);
    assert.equal(
      await panel.getByRole("button", { name: "Remove msedge.exe", exact: true }).isDisabled(),
      true,
    );
    await page.evaluate(() => {
      globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps = false;
    });
    assert.equal(
      await settings.getByRole("button", { name: "Start proxy", exact: true }).isDisabled(),
      true,
    );
    await page.evaluate(() => {
      globalThis.__workspaceFixture.releaseProductRead();
    });
    await idle();
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      false,
    );
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps,
      ),
      false,
    );
    await fixture({ lifecycle: "running", runtimeDecrypt: false, unresolvedProcessConnections: 7 });
    await settings.evaluate((element) => element.refreshStatus());
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
    // HTTPS mode changes use Save and keep the same listener and active recording.
    await fixture({ capture: { state: "active", path: "live-https.tmcap", bytesWritten: 4096 } });
    const listener = await page.evaluate(
      async () => (await window.__TAURI_INTERNALS__.invoke("app_status")).listener,
    );
    const lifecycleCalls = await page.evaluate(() => ({
      start: globalThis.__workspaceFixture.calls.start_proxy ?? 0,
      stop: globalThis.__workspaceFixture.calls.stop_application ?? 0,
    }));
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).check();
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt),
      false,
      "Checkbox applied before Save",
    );
    await fixture({ settingsSaveError: true });
    await save.click();
    await idle();
    await settings.getByText(/Fixture settings persistence failed/).waitFor();
    assert.equal(
      await settings
        .getByRole("button", { name: "Interception certificate…", exact: true })
        .isVisible(),
      false,
      "A persistence failure offered setup for a usable certificate",
    );
    await fixture({ settingsSaveError: false });
    await save.click();
    await idle();
    assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), true);
    assert.equal(await page.locator(".https-mode").isVisible(), false);
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).uncheck();
    await save.click();
    await idle();
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
    await certificateSummary.click();
    assert.equal(
      await settings.getByLabel("CA private key", { exact: true }).isVisible(),
      true,
      "Certificate settings disappeared when decryption was off",
    );
    await certificateSummary.click();
    await fixture({
      caBootstrap: {
        caFilesPresent: false,
        caFilesExist: false,
        ownedCaSha256: null,
        ownedCaTrusted: false,
      },
    });
    await name.fill("preserve-on-error.exe");
    await name.press("Enter");
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).check();
    await save.click();
    await idle();
    await settings
      .locator(".settings-save .field-error")
      .filter({ hasText: /Set up the interception certificate before enabling/ })
      .waitFor();
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      true,
    );
    assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), false);
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps,
      ),
      false,
    );
    assert.equal(
      await panel
        .getByRole("button", { name: "Remove preserve-on-error.exe", exact: true })
        .count(),
      1,
    );
    const errorCertificateButton = settings
      .locator(".settings-save")
      .getByRole("button", { name: "Interception certificate…", exact: true });
    await errorCertificateButton.click();
    assert.equal(
      await certificateSummary.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
    assert.equal(await settings.getByLabel("CA private key", { exact: true }).isVisible(), true);
    const setupCertificate = settings
      .locator(".interception-certificate")
      .getByRole("button", { name: /^(Set up|Create) interception certificate$/ });
    const contrast = await setupCertificate.evaluate((button) => {
      const luminance = (color) => {
        const [red, green, blue] = color
          .match(/\d+/g)
          .slice(0, 3)
          .map((channel) => {
            const srgb = Number(channel) / 255;
            return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4;
          });
        return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
      };
      const style = getComputedStyle(button);
      const [text, background] = [luminance(style.color), luminance(style.backgroundColor)];
      return (Math.max(text, background) + 0.05) / (Math.min(text, background) + 0.05);
    });
    assert.ok(contrast >= 4.5, "The certificate setup action has insufficient text contrast");
    for (const [width, height, suffix] of [
      [1440, 900, "wide"],
      [960, 720, "compact"],
      [800, 600, "small"],
    ]) {
      await page.setViewportSize({ width, height });
      await errorCertificateButton.click();
      await page.waitForFunction(() =>
        document.querySelector("app-shell").settings.interceptionSummary.matches(":focus"),
      );
      assert.equal(
        await setupCertificate.evaluate((button) => {
          const action = button.getBoundingClientRect();
          const fields = button.closest(".settings-fields").getBoundingClientRect();
          return action.top >= fields.top && action.bottom <= fields.bottom;
        }),
        true,
        "The recovery shortcut left the setup action outside the visible fields",
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/interception-certificate-${suffix}.png`),
      });
    }
    const installCalls = await page.evaluate(
      () => globalThis.__workspaceFixture.calls.install_certificate ?? 0,
    );
    const activeUsesSystemProxy = await settings.evaluate(
      (element) => element.connectionUsesSystemProxy,
    );
    if (activeUsesSystemProxy) {
      page.once("dialog", (dialog) => dialog.accept());
    }
    await setupCertificate.click();
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.calls.install_certificate ?? 0),
      installCalls + Number(activeUsesSystemProxy),
    );
    assert.equal(
      await page
        .locator(".notice")
        .getByRole("button", { name: "Start proxy", exact: true })
        .count(),
      0,
    );
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt),
      false,
      "Certificate setup enabled decryption without Save",
    );
    await page
      .locator(".notice")
      .getByRole("button", { name: "HTTPS settings", exact: true })
      .click();
    await page.waitForFunction(
      () => document.querySelector("app-shell").settings.settingsSection === "connection",
    );
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      true,
    );
    assert.equal(
      await panel
        .getByRole("button", { name: "Remove preserve-on-error.exe", exact: true })
        .count(),
      1,
    );
    await save.click();
    await idle();
    assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), true);
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).uncheck();
    await save.click();
    await idle();
    assert.equal(
      await page.evaluate(
        async () => (await window.__TAURI_INTERNALS__.invoke("app_status")).listener,
      ),
      listener,
    );
    assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.capture), {
      state: "active",
      path: "live-https.tmcap",
      bytesWritten: 4096,
    });
    assert.deepEqual(
      await page.evaluate(() => ({
        start: globalThis.__workspaceFixture.calls.start_proxy ?? 0,
        stop: globalThis.__workspaceFixture.calls.stop_application ?? 0,
      })),
      lifecycleCalls,
    );
    await fixture({ lifecycle: "draining" });
    await settings.evaluate((element) => element.refreshStatus());
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
    await settings.evaluate((element) => element.startProxy());
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
    await settings.getByRole("button", { name: "Support", exact: true }).click();
    await settings.getByRole("button", { name: "Refresh diagnostics", exact: true }).click();
    await settings.locator(".support-facts").getByText("7", { exact: true }).waitFor();
    await connection.click();
    await panel.locator("summary").click();
    await fixture({
      lifecycle: "draining",
      statusSummary: "Proxy routing off · 0 requests · 0 WebSockets open · 1 other connections",
    });
    await settings.evaluate((element) => element.refreshStatus());
    for (const [width, height, theme] of [
      [1440, 900, "light"],
      [960, 720, "dark"],
      [800, 600, "light"],
    ]) {
      await page.setViewportSize({ width, height });
      await preferences.click();
      await settings.getByLabel("Theme", { exact: true }).selectOption(theme);
      await settings
        .locator("#preferences-form")
        .getByRole("button", { name: "Save settings", exact: true })
        .click();
      await idle();
      await connection.click();
      await mode.scrollIntoViewIfNeeded();
      assert.equal(
        await page.getByRole("button", { name: "Copy proxy address", exact: true }).isVisible(),
        true,
      );
      for (const action of [
        page.getByRole("button", { name: "Copy proxy address", exact: true }),
        page.getByRole("button", { name: /routing and status/ }),
      ]) {
        assert.equal(
          await action.evaluate((element) => {
            const box = element.getBoundingClientRect();
            return box.width >= 20 && box.height > 0 && box.right <= innerWidth && box.left >= 0;
          }),
          true,
        );
      }
      for (const label of [page.locator(".inspection-scope"), page.locator(".https-mode")]) {
        assert.equal(
          await label.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
          true,
        );
      }
      if (width === 960) {
        const tools = page.getByRole("button", { name: /routing and status/ });
        await tools.click();
        await page
          .locator("#proxy-status-tools")
          .getByText("Proxy routing off · 0 requests · 0 WebSockets open · 1 other connections", {
            exact: true,
          })
          .waitFor();
        await page.keyboard.press("Escape");
        assert.equal(
          await tools.evaluate((element) => element.getRootNode().activeElement === element),
          true,
        );
      }
      assert.equal(
        await panel.getByRole("button", { name: "Add process", exact: true }).isEnabled(),
        true,
      );
      assert.equal(
        await panel.getByRole("button", { name: "Refresh suggestions", exact: true }).isEnabled(),
        true,
      );
      assert.equal(
        await panel.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
        true,
      );
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/process-inspection-${width}-${theme}.png`),
      });
    }
    await page.evaluate(() => document.querySelector("app-shell").activateView("traffic"));
    await page.setViewportSize({ width: 760, height: 520 });
    const compactControls = await page.evaluate(() => {
      const root = document.querySelector("app-shell").shadowRoot;
      const heading = root.querySelector("h1");
      const original = heading.textContent;
      try {
        heading.textContent = "Inspect localized traffic safely — ".repeat(8);
        return [
          ...root.querySelectorAll(
            ".app-view[data-active] button, .topbar button, .app-footer button",
          ),
        ]
          .filter(
            (button) =>
              !button.closest("[hidden]") &&
              !(button.closest("dialog") && !button.closest("dialog").open) &&
              !(
                button.closest("message-inspector") &&
                getComputedStyle(button.closest("message-inspector")).display === "none"
              ) &&
              !(
                button.closest("[popover]") && !button.closest("[popover]").matches(":popover-open")
              ),
          )
          .filter((button) => button.getBoundingClientRect().height <= 0)
          .map((button) => button.getAttribute("aria-label") ?? button.textContent.trim());
      } finally {
        heading.textContent = original;
      }
    });
    assert.deepEqual(compactControls, [], "Compact localized layout hid an interactive control");
    await page.evaluate(() => document.querySelector("app-shell").activateView("settings"));
    await page.setViewportSize({ width: 800, height: 600 });
    await page.emulateMedia({ forcedColors: "active" });
    assert.equal(await connection.getAttribute("aria-current"), "page");
    assert.notEqual(
      await connection.evaluate((button) => getComputedStyle(button).backgroundColor),
      await preferences.evaluate((button) => getComputedStyle(button).backgroundColor),
    );
    await mode.focus();
    await page.keyboard.press("Tab");
    assert.equal(
      await panel
        .getByRole("button", { name: "Remove msedge.exe", exact: true })
        .evaluate((button) => button.getRootNode().activeElement === button),
      true,
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/process-inspection-forced-colors.png"),
    });
    await page.emulateMedia({ forcedColors: "none" });
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).check();
    await save.click();
    await idle();
    await fixture({
      lifecycle: "stopped",
      caBootstrap: {
        caFilesPresent: false,
        caFilesExist: false,
        ownedCaSha256: null,
        ownedCaTrusted: false,
      },
      unresolvedProcessConnections: 0,
    });
    await checkStoppedHttpsSettings(page, settings, panel, fixture, idle);
    await settings.evaluate((element) => element.refreshStatus());
    await fixture({ holdCommand: "desktop_bootstrap", finishHeldCommand: null });
    await page.locator(".top-actions").getByRole("button", { name: "Start proxy" }).click();
    await page.waitForFunction(() => Boolean(globalThis.__workspaceFixture.finishHeldCommand));
    try {
      await preferences.click();
    } finally {
      await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
    }
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
    assert.equal(await preferences.getAttribute("aria-current"), "page");
    await page
      .locator(".notice")
      .getByRole("button", { name: "Start with HTTPS tunnels", exact: true })
      .waitFor();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/process-inspection-certificate-free-start.png"),
    });
    const caBefore = await page.evaluate(() => ({
      create: globalThis.__workspaceFixture.calls.create_ca ?? 0,
      install: globalThis.__workspaceFixture.calls.install_certificate ?? 0,
    }));
    // Keep an unsaved policy change while explicitly choosing certificate-free startup.
    await connection.click();
    await mode.selectOption("except");
    await name.fill("keep-draft.exe");
    await name.press("Enter");
    const startTunnels = page
      .locator(".notice")
      .getByRole("button", { name: "Start with HTTPS tunnels", exact: true });
    const decrypt = settings.getByLabel("Decrypt HTTPS traffic", { exact: true });
    await decrypt.uncheck();
    await fixture({ tunnelStartError: true });
    await startTunnels.click();
    await settings.getByText(/Start failed: Fixture tunnel start failed/).waitFor();
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
    assert.equal(
      await decrypt.isChecked(),
      false,
      "Failed start discarded an unsaved HTTPS choice",
    );
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps,
      ),
      true,
    );
    await fixture({ tunnelStartError: false, tunnelStartAfterSaveError: true });
    await startTunnels.click();
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      false,
    );
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps,
      ),
      false,
    );
    await fixture({ tunnelStartAfterSaveError: false });
    await fixture({ holdProxyStart: true });
    await settings.evaluate((element) => {
      void element.startHttpsTunnels();
    });
    await page.waitForFunction(() => Boolean(globalThis.__workspaceFixture.releaseProxyStart));
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isDisabled(),
      true,
    );
    await name.fill("during-start.exe");
    await name.press("Enter");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.holdProxyStart = false;
      globalThis.__workspaceFixture.releaseProxyStart();
    });
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
    assert.equal(await mode.inputValue(), "except");
    assert.equal(
      await panel.getByRole("button", { name: "Remove keep-draft.exe", exact: true }).count(),
      1,
    );
    assert.equal(
      await panel.getByRole("button", { name: "Remove during-start.exe", exact: true }).count(),
      1,
    );
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      false,
    );
    assert.deepEqual(
      await page.evaluate(() => ({
        create: globalThis.__workspaceFixture.calls.create_ca ?? 0,
        install: globalThis.__workspaceFixture.calls.install_certificate ?? 0,
      })),
      caBefore,
    );
    assert.equal(
      await page.evaluate(
        () => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps,
      ),
      false,
    );
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
  } finally {
    await page.emulateMedia({ forcedColors: "none" });
    await fixture({
      savedProduct: original.product,
      caBootstrap: original.bootstrap,
      lifecycle: original.lifecycle,
      settingsSaveError: false,
      unresolvedProcessConnections: 0,
      tunnelStartError: false,
      tunnelStartAfterSaveError: false,
      statusSummary: null,
      capture: original.capture,
      runtimeDecrypt: original.runtimeDecrypt,
    });
    await settings.evaluate(async (element) => {
      await element.loadSettings();
      await element.loadSettings("connection");
      await element.refreshStatus();
      element.dismissNotice();
      element.showSettingsSection("connection");
    });
    await settings.evaluate((element, open) => {
      element.certificateDetailsOpen = open;
    }, original.certificateDetailsOpen);
    await page.setViewportSize({ width: 800, height: 600 });
  }
}

async function checkSettingsDrafts(page, settings, preferences, connection, fixture, idle) {
  const theme = settings.getByLabel("Theme", { exact: true });
  const isolation = settings.getByLabel("Isolate outbound connections by caller process", {
    exact: true,
  });
  const preferencesForm = settings.locator("#preferences-form");
  const connectionForm = settings.locator("#connection-form");
  const saved = () => page.evaluate(() => globalThis.__workspaceFixture.savedProduct.preferences);
  await preferences.click();
  const originalTheme = await theme.inputValue();
  const differentTheme = originalTheme === "light" ? "dark" : "light";
  await theme.selectOption(differentTheme);
  await connection.click();
  const originalIsolation = await isolation.isChecked();
  await isolation.setChecked(!originalIsolation);
  await preferences.click();
  await preferencesForm.getByRole("button", { name: "Revert changes" }).click();
  await idle();
  assert.equal(await theme.inputValue(), originalTheme);
  await connection.click();
  assert.equal(
    await isolation.isChecked(),
    !originalIsolation,
    "Preferences revert dropped a Connection draft",
  );

  await preferences.click();
  await theme.selectOption(differentTheme);
  await fixture({ holdSettingsSaveReply: true, releaseSettingsSaveReply: null });
  await theme.press("Control+s");
  await page.waitForFunction(() => Boolean(globalThis.__workspaceFixture.releaseSettingsSaveReply));
  try {
    await connection.click();
    assert.equal(await isolation.isChecked(), !originalIsolation);
    assert.equal((await saved()).isolateOutboundConnections, originalIsolation);
    assert.equal(
      await connectionForm.getByRole("button", { name: "Save settings" }).isDisabled(),
      true,
    );
  } finally {
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      state.holdSettingsSaveReply = false;
      state.releaseSettingsSaveReply();
    });
  }
  await idle();

  await preferences.click();
  await theme.selectOption(originalTheme);
  await connection.click();
  await fixture({ holdSettingsSave: true, releaseSettingsSave: null });
  await connectionForm.getByRole("button", { name: "Save settings" }).click();
  await page.waitForFunction(() => Boolean(globalThis.__workspaceFixture.releaseSettingsSave));
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.savedProduct.window = { width: 1320, height: 820 };
    state.holdSettingsSave = false;
    state.releaseSettingsSave();
  });
  await idle();
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.savedProduct.window),
    { width: 1320, height: 820 },
    "A Connection save overwrote newer window geometry",
  );
  assert.equal(
    (await saved()).theme,
    differentTheme,
    "A Connection save persisted an appearance draft",
  );
  assert.equal((await saved()).isolateOutboundConnections, !originalIsolation);
  await preferences.click();
  assert.equal(await theme.inputValue(), originalTheme);
  await preferencesForm.getByRole("button", { name: "Save settings" }).click();
  await idle();
  assert.equal((await saved()).theme, originalTheme);

  await connection.click();
  const advanced = settings.locator("details.connection-advanced");
  const wasOpen = await advanced.evaluate((details) => details.open);
  const originalPort = (await saved()).proxyPort;
  if (!wasOpen) await advanced.locator("summary").click();
  await settings.getByLabel("Use a specific proxy port", { exact: true }).check();
  const port = settings.getByLabel("Proxy port", { exact: true });
  await port.fill("65536");
  await advanced.locator("summary").click();
  await connectionForm.getByRole("button", { name: "Save settings" }).click();
  await port.waitFor();
  assert.equal(
    await port.evaluate((input) => input.matches(":focus") && !input.validity.valid),
    true,
  );
  assert.equal((await saved()).proxyPort, originalPort);
  await connectionForm.getByRole("button", { name: "Revert changes" }).click();
  await idle();
  if (!wasOpen) await advanced.locator("summary").click();
  await isolation.setChecked(originalIsolation);
  await connectionForm.getByRole("button", { name: "Save settings" }).click();
  await idle();
}

async function checkSettingsNavigationOwnership(page) {
  for (const method of ["openMcpSettings", "openClientCertificateSettings"]) {
    for (const fails of [false, true]) {
      const result = await page.evaluate(
        async ({ method, fails }) => {
          const shell = document.querySelector("app-shell"),
            original = shell.$whenRendered,
            diagnostic = shell.diagnosticText;
          let release,
            entered,
            held = false;
          const gate = new Promise((resolve) => {
            release = resolve;
          });
          const waiting = new Promise((resolve) => {
            entered = resolve;
          });
          shell.$whenRendered = async () => {
            await original.call(shell);
            if (!held) {
              held = true;
              entered();
              await gate;
              if (fails) throw new Error("Older navigation failed");
            }
          };
          try {
            const older = shell[method]();
            await waiting;
            await shell.openConnectionSettings();
            release();
            await older;
            return {
              section: shell.settings.settingsSection,
              diagnostic: shell.diagnosticText,
              expectedDiagnostic: diagnostic,
            };
          } finally {
            release();
            shell.$whenRendered = original;
          }
        },
        { method, fails },
      );
      assert.equal(result.section, "connection", `${method} replaced a newer settings destination`);
      assert.equal(
        result.diagnostic,
        result.expectedDiagnostic,
        `${method} reported an obsolete navigation failure`,
      );
    }
  }
}

async function checkStatusCompletionOrder(page) {
  for (const olderFails of [true, false]) {
    const result = await page.evaluate(async (olderFails) => {
      const shell = document.querySelector("app-shell"),
        invoke = window.__TAURI_INTERNALS__.invoke,
        originalDiagnostic = shell.diagnosticText;
      await shell.settings.ready;
      let release,
        entered,
        calls = 0;
      const gate = new Promise((resolve) => (release = resolve)),
        waiting = new Promise((resolve) => (entered = resolve));
      window.__TAURI_INTERNALS__.invoke = async (command, args) => {
        if (command === "app_status") {
          calls++;
          if (calls === 1) {
            const value = await invoke(command, args);
            entered();
            await gate;
            if (olderFails) throw new Error("Older status failed");
            return value;
          }
          if (calls === 2 && !olderFails) throw new Error("Current status failed");
        }
        return invoke(command, args);
      };
      const older = shell.refreshStatus();
      try {
        await waiting;
        await shell.refreshStatus();
        const expected = shell.diagnosticText;
        release();
        await older;
        return { expected, actual: shell.diagnosticText };
      } finally {
        release();
        await older;
        window.__TAURI_INTERNALS__.invoke = invoke;
        shell.diagnosticText = originalDiagnostic;
      }
    }, olderFails);
    assert.equal(
      result.actual,
      result.expected,
      "An older status completion replaced current feedback",
    );
  }
}

async function checkStoppedHttpsSettings(page, settings, panel, fixture, idle) {
  const decrypt = settings.getByLabel("Decrypt HTTPS traffic", { exact: true });
  const name = panel.getByLabel("Executable name", { exact: true });
  const save = settings
    .locator("#connection-form")
    .getByRole("button", { name: "Save settings", exact: true });
  // Another window saves tunnel mode while this window still displays its older
  // checked preference. Start refreshes that unchanged field and keeps the
  // independent process draft.
  const before = await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.savedProduct.preferences.decryptHttps = false;
    return {
      starts: state.calls.start_proxy ?? 0,
      create: state.calls.create_ca ?? 0,
      install: state.calls.install_certificate ?? 0,
    };
  });
  await name.fill("stale-window-draft.exe");
  await name.press("Enter");
  await settings.evaluate(async (element) => {
    await element.refreshStatus();
    await element.startProxy();
  });
  assert.deepEqual(
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      return {
        starts: state.calls.start_proxy ?? 0,
        create: state.calls.create_ca ?? 0,
        install: state.calls.install_certificate ?? 0,
      };
    }),
    { ...before, starts: before.starts + 1 },
  );
  assert.equal(await decrypt.isChecked(), false, "Start kept a stale unchanged HTTPS preference");
  assert.equal(
    await panel.getByRole("button", { name: "Remove stale-window-draft.exe", exact: true }).count(),
    1,
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), false);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps),
    false,
  );
  await fixture({ lifecycle: "stopped" });
  await settings.evaluate(async (element) => {
    await element.refreshStatus();
    element.dismissNotice();
  });
  const certificateSummary = settings.locator(".interception-certificate > summary");
  if (await certificateSummary.evaluate((summary) => summary.parentElement.open)) {
    await certificateSummary.click();
  }
  // Enabling while stopped requires the same certificate validation as a live
  // save and leaves every authored field available for setup and retry.
  await name.fill("stopped-recovery.exe");
  await name.press("Enter");
  await decrypt.check();
  await settings.evaluate((element) => element.startProxy());
  assert.equal(await decrypt.isChecked(), true, "Start replaced an authored HTTPS draft");
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), false);
  await fixture({ lifecycle: "stopped" });
  await settings.evaluate((element) => element.refreshStatus());
  await name.fill("C:\\invalid.exe");
  await save.click();
  await panel.getByText(/Enter an executable name/).waitFor();
  assert.equal(
    await settings
      .getByRole("button", { name: "Interception certificate…", exact: true })
      .isVisible(),
    false,
    "A process validation error offered certificate setup",
  );
  await name.fill("");
  await save.click();
  await idle();
  await settings
    .locator(".settings-save .field-error")
    .filter({ hasText: /Set up the interception certificate before enabling/ })
    .waitFor();
  assert.equal(await decrypt.isChecked(), true);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps),
    false,
  );
  await fixture({
    caBootstrap: { caFilesPresent: false, caFilesExist: true, ownedCaSha256: null },
  });
  await save.click();
  await idle();
  await settings.getByRole("button", { name: "Reset certificate and set up again" }).waitFor();
  assert.equal(
    await settings
      .getByRole("button", {
        name: /^(Set up HTTPS interception|Create interception certificate)$/,
      })
      .isVisible(),
    false,
    "An incomplete certificate offered creation without a reset",
  );
  await fixture({
    caBootstrap: {
      caFilesPresent: false,
      caFilesExist: false,
      ownedCaSha256: null,
      ownedCaTrusted: false,
    },
  });
  await save.click();
  await idle();
  await settings
    .locator(".settings-save")
    .getByRole("button", { name: "Interception certificate…", exact: true })
    .click();
  assert.equal(
    await settings
      .locator(".interception-certificate > summary")
      .evaluate((element) => element.getRootNode().activeElement === element),
    true,
  );
  if (await settings.evaluate((element) => element.connectionUsesSystemProxy)) {
    page.once("dialog", (dialog) => dialog.accept());
  }
  await settings
    .locator(".interception-certificate")
    .getByRole("button", { name: /^(Set up|Create) interception certificate$/ })
    .click();
  await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
  await settings
    .getByText("Certificate ready. Save settings to enable HTTPS decryption.")
    .waitFor();
  assert.equal(await settings.getByText(/Settings save failed:/).isVisible(), false);
  await page
    .locator(".notice")
    .getByRole("button", { name: "HTTPS settings", exact: true })
    .click();
  assert.equal(await decrypt.isChecked(), true);
  assert.equal(
    await panel.getByRole("button", { name: "Remove stopped-recovery.exe", exact: true }).count(),
    1,
  );
  await save.click();
  await idle();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.savedProduct.preferences.decryptHttps),
    true,
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.lifecycle), "stopped");
  await fixture({
    caBootstrap: {
      caFilesPresent: false,
      caFilesExist: false,
      ownedCaSha256: null,
      ownedCaTrusted: false,
    },
  });
}
