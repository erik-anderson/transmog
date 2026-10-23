import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkClientCertificates(page, root) {
  for (const width of [1280, 640]) {
    await page.setViewportSize({ width, height: 800 });
    const aligned = await page.evaluate(() => {
      const root = document.querySelector("app-shell").shadowRoot;
      const brand = root.querySelector(".brand").getBoundingClientRect();
      const actions = root.querySelector(".top-actions").getBoundingClientRect();
      return Math.abs(brand.y + brand.height / 2 - actions.y - actions.height / 2) < 2;
    });
    assert.equal(aligned, true, "Empty certificate requests added a header row");
    assert.equal(await page.locator("client-certificate-requests").isVisible(), false);
  }
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.locator('a[data-view="settings"]').click();
  const settings = page.locator("settings-workspace");
  await settings.getByRole("button", { name: "Preferences", exact: true }).click();
  const advanced = settings
    .locator("details.connection-advanced")
    .filter({ has: page.getByText("Advanced network settings", { exact: true }) });
  await advanced.locator("summary").click();
  await advanced.getByRole("button", { name: "Client certificates…", exact: true }).click();
  const editor = page.locator("client-certificate-settings");
  await editor.getByLabel("Default client certificate action").waitFor();
  await page.waitForFunction(
    () => document.querySelector("app-shell").settings.clientCertificates.loaded,
  );
  await editor.getByRole("button", { name: "Add host rule", exact: true }).click();
  await editor
    .getByLabel("Client certificate HTTPS host", { exact: true })
    .fill("certificate.example");
  await editor.getByLabel("Client certificate HTTPS port", { exact: true }).fill("8443");
  await editor
    .getByLabel("Host client certificate action", { exact: true })
    .selectOption("automatic");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").settings.clientCertificates.certificateIdentities
        .length === 2,
  );
  await editor
    .getByLabel("Automatic client certificate", { exact: true })
    .selectOption("windows:first");
  await editor.getByRole("button", { name: "Add host rule", exact: true }).click();
  await editor
    .getByLabel("Client certificate HTTPS host", { exact: true })
    .fill("certificate.example");
  await editor.getByLabel("Client certificate HTTPS port", { exact: true }).fill("8443");
  await editor.getByLabel("Host client certificate action", { exact: true }).selectOption("prompt");
  await editor
    .getByRole("button", { name: "Move certificate.example:8443 up", exact: true })
    .last()
    .click();
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").settings.clientCertificates.policy.rules[0].mode.action,
    ),
    "prompt",
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.calls.save_client_certificate_policy ?? 0,
    ),
    0,
    "Draft editing activated certificate policy",
  );
  const handles = editor.getByRole("button", {
    name: "Reorder certificate.example:8443",
    exact: true,
  });
  const transfer = await page.evaluateHandle(() => new DataTransfer());
  await handles.first().dispatchEvent("dragstart", { dataTransfer: transfer });
  await editor
    .locator(".certificate-rule")
    .last()
    .dispatchEvent("dragover", { dataTransfer: transfer });
  await editor
    .locator(".certificate-rule")
    .last()
    .dispatchEvent("drop", { dataTransfer: transfer });
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").settings.clientCertificates.policy.rules[0].mode.action,
    ),
    "automatic",
  );
  await editor.getByRole("button", { name: "Save certificate settings", exact: true }).click();
  await editor.getByText(/Certificate settings saved/).waitFor();
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.certificatePolicy.rules[0].mode.selector.identity,
    ),
    "windows:first",
  );
  await editor.getByRole("button", { name: "Remove selected host rule", exact: true }).click();
  await editor.getByRole("button", { name: "Undo removal", exact: true }).click();
  assert.equal(await editor.locator(".certificate-rule").count(), 2);
  await editor.locator(".rule-select").filter({ hasText: "Automatic certificate" }).click();
  await page.evaluate(() => (globalThis.__workspaceFixture.certificateIdentities = []));
  await editor.getByRole("button", { name: "Refresh Windows certificates", exact: true }).click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").settings.clientCertificates.certificateIdentityMissing,
  );
  assert.equal(
    await editor.getByLabel("Automatic client certificate", { exact: true }).inputValue(),
    "windows:first",
    "Missing pinned identity silently changed",
  );
  await editor
    .getByLabel("Host client certificate action", { exact: true })
    .selectOption("automatic");
  await editor.getByLabel("Certificate selector mode", { exact: true }).selectOption("attributes");
  await editor.getByLabel("Subject Common name", { exact: true }).fill("Renewable client");
  await editor.getByLabel("Issuer Organization", { exact: true }).fill("Certificate issuer");
  await editor.getByRole("button", { name: "Save certificate settings", exact: true }).click();
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.certificatePolicy.rules[0].mode.selector.subject.CN ===
      "Renewable client",
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.certificatePolicy.rules[0].mode.selector.identity,
    ),
    null,
  );
  await editor.getByLabel("Default client certificate action").selectOption("prompt");
  await page.evaluate(() => (globalThis.__workspaceFixture.certificateSaveError = true));
  await editor.getByRole("button", { name: "Save certificate settings", exact: true }).click();
  await editor
    .getByText("Fixture certificate policy persistence failed", { exact: true })
    .waitFor();
  await page
    .locator(".operation-error-dialog[open]")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  assert.equal(await editor.getByLabel("Default client certificate action").inputValue(), "prompt");
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").settings.clientCertificates.certificateSettingsDirty,
    ),
    true,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.certificateSaveError = false));
  await editor.getByRole("button", { name: "Revert certificate changes", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").settings.clientCertificates.certificateSettingsBusy,
  );
  await page.setViewportSize({ width: 640, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/client-certificate-settings-compact.png"),
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/client-certificate-settings-wide.png"),
  });
  await page.locator('a[data-view="traffic"]').click();
  const id = "9007199254740999";
  await page.evaluate((id) => {
    globalThis.__workspaceFixture.certificatePending = [
      {
        id,
        host: "certificate.example",
        port: 8443,
        caller: "Composer",
        connections: 1,
        choiceCount: 2,
        warning: null,
      },
      {
        id: "other",
        host: "other.example",
        port: 443,
        caller: "Composer",
        connections: 1,
        choiceCount: 2,
        warning: null,
      },
    ];
  }, id);
  const queue = page.locator("client-certificate-requests");
  await queue
    .getByRole("button", { name: "2 connections waiting for a client certificate", exact: true })
    .waitFor();
  assert.equal(
    await queue.locator('span[role="status"]').evaluate((node) => {
      const box = node.getBoundingClientRect();
      return box.width <= 1 && box.height <= 1 && getComputedStyle(node).position === "absolute";
    }),
    true,
    "The accessibility announcement occupied visible header space",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").activeView),
    "traffic",
    "A background certificate request stole navigation",
  );
  await queue
    .getByRole("button", { name: "2 connections waiting for a client certificate", exact: true })
    .click();
  const panel = queue.locator(".certificate-request-panel:popover-open");
  await panel.getByLabel("Eligible client certificate").selectOption("windows:first");
  assert.equal(
    await panel.locator(".certificate-pending-row").first().getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await panel.locator(".certificate-pending-row").last().getAttribute("aria-pressed"),
    "false",
  );
  await panel.locator(".certificate-pending-row").last().click();
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").shadowRoot.querySelector("client-certificate-requests")
        .certificateRequest?.pending.id === "other",
  );
  assert.equal(
    await panel.locator(".certificate-pending-row").first().getAttribute("aria-pressed"),
    "false",
  );
  assert.equal(
    await panel.locator(".certificate-pending-row").last().getAttribute("aria-pressed"),
    "true",
  );
  await panel.locator(".certificate-pending-row").first().click();
  await panel.getByLabel("Eligible client certificate").selectOption("windows:first");
  assert.equal(await panel.getAttribute("aria-modal"), "false");
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/client-certificate-prompt-wide.png"),
  });
  await page.setViewportSize({ width: 640, height: 520 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/client-certificate-prompt-compact.png"),
  });
  const close = panel.getByRole("button", { name: "Close certificate requests", exact: true });
  assert.equal(
    await close.evaluate((node) => {
      const r = node.getBoundingClientRect();
      return node.getRootNode().elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) === node;
    }),
    true,
  );
  await panel.getByRole("button", { name: "Use selected certificate", exact: true }).click();
  await panel.getByText(/Selected certificate allowed/).waitFor();
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.certificateDecision), {
    id,
    decision: { action: "certificate", identity: "windows:first" },
  });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.save_client_certificate_policy),
    3,
    "A connection answer saved a host rule",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.certificatePending = []));
  await panel.getByText("No connections are waiting.", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await panel.waitFor({ state: "hidden" });
  await page.setViewportSize({ width: 1280, height: 800 });
  // Preserve the inspected expired context and reject an answer after peer closure.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.certificatePending = [
      {
        id: "closed",
        host: "closed.example",
        port: 443,
        caller: "Proxy client",
        connections: 1,
        choiceCount: 0,
        warning: "No eligible certificates",
      },
    ];
  });
  await queue
    .getByRole("button", { name: "1 connection waiting for a client certificate", exact: true })
    .click();
  await panel.getByRole("button", { name: /closed.example:443/ }).click();
  await panel
    .getByText(
      "No compatible certificates are available. You can still try this connection without a certificate.",
      { exact: true },
    )
    .waitFor();
  await page.evaluate(() => (globalThis.__workspaceFixture.certificatePending = []));
  await panel.getByText(/This connection is no longer waiting/).waitFor();
  assert.equal(
    await panel
      .getByRole("button", { name: "Continue without a certificate", exact: true })
      .isDisabled(),
    true,
  );
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.certificatePolicy;
    localStorage.removeItem("certificatePolicy");
  });
}
