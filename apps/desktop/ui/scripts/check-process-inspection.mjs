import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkProcessInspection(page, root) {
  const settings = page.locator("settings-workspace");
  const panel = settings.locator("process-inspection-settings");
  const mode = panel.getByRole("combobox", { name: "Inspect traffic from", exact: true });
  const name = panel.getByLabel("Executable name", { exact: true });
  const save = settings.getByRole("button", { name: "Save settings", exact: true });
  const fixture = async (values) =>
    page.evaluate((values) => Object.assign(globalThis.__workspaceFixture, values), values);
  const idle = () =>
    page.waitForFunction(() => {
      const settings = document.querySelector("app-shell").settings;
      return !settings.settingsBusy && !settings.querySelector(".network-settings").disabled;
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
    await settings.getByRole("button", { name: "Preferences", exact: true }).click();
    await settings.evaluate((element) => element.loadSettings());
    assert.equal(await mode.inputValue(), "all");
    assert.equal(await name.isVisible(), false);
    await mode.selectOption("only");
    const certificateButton = settings
      .locator(".https-preference-row")
      .getByRole("button", { name: "Interception certificate…", exact: true });
    await certificateButton.click();
    const certificateSummary = settings.locator(".interception-certificate > summary");
    assert.equal(
      await certificateSummary.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
    assert.equal(await settings.getByLabel("CA private key", { exact: true }).isVisible(), true);
    await settings.getByRole("button", { name: "Back to preferences", exact: true }).click();
    assert.equal(
      await certificateButton.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
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
    await settings.getByLabel("Theme", { exact: true }).selectOption("dark");
    await save.click();
    await idle();
    assert.equal((await saved()).mode, "except");
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
    // A second window saves tunnel mode after this Save reads an older snapshot.
    // Local Start then refreshes the visible baseline before Save continues.
    // The unrelated Save must still use its original baseline and retain false.
    await fixture({ holdProductRead: true, lifecycle: "stopped" });
    await settings.evaluate((element) => element.refreshStatus());
    await settings.evaluate((element) => {
      void element.saveSettings(new Event("submit", { cancelable: true }));
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
    await settings.evaluate((element) => element.startProxy());
    assert.equal(
      await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).isChecked(),
      false,
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
    await save.click();
    await idle();
    assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.runtimeDecrypt), true);
    assert.equal(await page.locator(".https-mode").isVisible(), false);
    await settings.getByLabel("Decrypt HTTPS traffic", { exact: true }).uncheck();
    await save.click();
    await idle();
    await page.locator(".https-mode").getByText("HTTPS tunnels", { exact: true }).waitFor();
    await certificateButton.click();
    assert.equal(
      await settings.getByLabel("CA private key", { exact: true }).isVisible(),
      true,
      "Certificate settings disappeared when decryption was off",
    );
    await settings.getByRole("button", { name: "Back to preferences", exact: true }).click();
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
    await settings.getByLabel("Theme", { exact: true }).selectOption("dark");
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
    await settings
      .locator(".connection-panel .operation-status")
      .filter({ hasText: /Set up the interception certificate before enabling/ })
      .waitFor();
    assert.equal(
      await certificateSummary.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
    await settings.getByRole("button", { name: "Back to preferences", exact: true }).click();
    assert.equal(
      await errorCertificateButton.evaluate(
        (element) => element.getRootNode().activeElement === element,
      ),
      true,
    );
    assert.equal(await settings.getByLabel("Theme", { exact: true }).inputValue(), "dark");
    await errorCertificateButton.click();
    for (const [width, height, suffix] of [
      [1440, 900, "wide"],
      [960, 720, "compact"],
    ]) {
      await page.setViewportSize({ width, height });
      await certificateSummary.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/interception-certificate-${suffix}.png`),
      });
    }
    page.once("dialog", (dialog) => dialog.accept());
    await settings
      .locator(".interception-certificate")
      .getByRole("button", { name: "Set up interception certificate", exact: true })
      .click();
    await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
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
      .getByRole("button", { name: "Back to preferences", exact: true })
      .click();
    await page.waitForFunction(
      () => document.querySelector("app-shell").settings.settingsSection === "preferences",
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
    assert.equal(await settings.getByLabel("Theme", { exact: true }).inputValue(), "dark");
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
    await settings.getByRole("button", { name: "Preferences", exact: true }).click();
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
      await settings.getByLabel("Theme", { exact: true }).selectOption(theme);
      await save.click();
      await idle();
      await mode.scrollIntoViewIfNeeded();
      assert.equal(
        await page.getByRole("button", { name: "Copy proxy address", exact: true }).isVisible(),
        true,
      );
      for (const action of [
        page.getByRole("button", { name: "Copy proxy address", exact: true }),
        page.getByRole("button", { name: "Proxy status and tools", exact: true }),
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
        const tools = page.getByRole("button", { name: "Proxy status and tools", exact: true });
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
    await settings.evaluate(async (element) => {
      await element.refreshStatus();
      await element.startProxy();
    });
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
    await settings.getByRole("button", { name: "Preferences", exact: true }).click();
    await mode.selectOption("except");
    await name.fill("keep-draft.exe");
    await name.press("Enter");
    await fixture({ tunnelStartAfterSaveError: true });
    await page
      .locator(".notice")
      .getByRole("button", { name: "Start with HTTPS tunnels", exact: true })
      .click();
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
      tunnelStartAfterSaveError: false,
      statusSummary: null,
      capture: original.capture,
      runtimeDecrypt: original.runtimeDecrypt,
    });
    await settings.evaluate(async (element) => {
      await element.loadSettings();
      await element.refreshStatus();
      element.dismissNotice();
      element.showSettingsSection("preferences");
    });
    await settings.evaluate((element, open) => {
      element.certificateDetailsOpen = open;
    }, original.certificateDetailsOpen);
    await page.setViewportSize({ width: 800, height: 600 });
  }
}

async function checkStoppedHttpsSettings(page, settings, panel, fixture, idle) {
  const decrypt = settings.getByLabel("Decrypt HTTPS traffic", { exact: true });
  const name = panel.getByLabel("Executable name", { exact: true });
  const save = settings.getByRole("button", { name: "Save settings", exact: true });
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
  page.once("dialog", (dialog) => dialog.accept());
  await settings
    .locator(".interception-certificate")
    .getByRole("button", { name: "Set up interception certificate", exact: true })
    .click();
  await page.waitForFunction(() => !document.querySelector("app-shell").settings.proxyPending);
  await page
    .locator(".notice")
    .getByRole("button", { name: "Back to preferences", exact: true })
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
