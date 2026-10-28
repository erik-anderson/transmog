import assert from "node:assert/strict";
import { resolve } from "node:path";
import { newTrafficRule } from "./check-unified-rules.mjs";

export async function checkMcp(page, root) {
  await page.goto("https://workspace.test/");
  await page.locator('a[data-view="settings"]').click();
  await page
    .locator("settings-workspace")
    .getByRole("button", { name: "Integrations", exact: true })
    .click();
  const panel = page.locator("mcp-settings");
  await panel.getByText("MCP is off.", { exact: false }).waitFor();
  await panel.getByLabel("Allow MCP connections", { exact: true }).check();
  await panel.getByText("MCP is available on this computer.", { exact: true }).waitFor();
  await page
    .locator("mcp-indicator")
    .getByRole("button", { name: "MCP enabled", exact: true })
    .waitFor();
  await panel.getByRole("button", { name: "Create access key…", exact: true }).click();
  const editor = panel.getByRole("dialog", { name: "Create access key", exact: true });
  await editor.getByLabel("Name", { exact: true }).fill("Codex · shared rule collaboration");
  await editor.getByLabel("Traffic scope", { exact: true }).selectOption("hosts");
  await editor.getByLabel("Exact hosts", { exact: true }).fill("api.example.test\nlocalhost");
  await editor.getByLabel("Edit shared rules", { exact: true }).check();
  assert.equal(
    await editor.getByLabel("Omit authentication headers and bodies", { exact: true }).isChecked(),
    true,
  );
  assert.equal(
    await editor.getByLabel(/Read rules/).count(),
    0,
    "Rule reads must not need a permission checkbox",
  );
  for (const width of [1440, 960, 800]) {
    await page.setViewportSize({ width, height: width === 800 ? 600 : 900 });
    assert.equal(
      await editor.evaluate((dialog) => dialog.scrollWidth <= dialog.clientWidth + 1),
      true,
      "Permission dialog overflows horizontally",
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/mcp-permissions-${width}.png`),
    });
  }
  await editor.getByRole("button", { name: "Create key", exact: true }).click();
  const key = panel.getByRole("dialog", { name: "Save your access key", exact: true });
  await key.waitFor();
  assert.match(
    await key.getByLabel("Agent configuration containing access key", { exact: true }).inputValue(),
    /TRANSMOG_ACCESS_KEY = "tmk_fixture_secret_shown_once"/,
  );
  await key.getByLabel("Agent configuration", { exact: true }).selectOption("copilot");
  const config = JSON.parse(
    await key.getByLabel("Agent configuration containing access key", { exact: true }).inputValue(),
  );
  assert.equal(config.mcpServers.transmog.env.TRANSMOG_ACCESS_KEY, "tmk_fixture_secret_shown_once");
  assert.equal(config.mcpServers.transmog.env.LOCALAPPDATA, "C:\\Users\\Fixture\\AppData\\Local");
  await page.keyboard.press("Escape");
  await key.waitFor({ state: "hidden" });
  await page.waitForFunction(
    () => !document.querySelector("app-shell").settings.querySelector("mcp-settings").mcpSecret,
  );
  assert.equal(await panel.evaluate((element) => element.mcpSecret), "");
  assert.equal(await panel.evaluate((element) => element.mcpConfig), "");
  assert.equal(
    await panel
      .getByRole("button", { name: "Create access key…", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  const row = panel.locator("article").filter({ hasText: "Codex · shared rule collaboration" });
  await checkSensitiveApprovals(page, root, panel);
  await row.getByRole("button", { name: "Edit permissions…", exact: true }).click();
  const edit = panel.getByRole("dialog", { name: "Edit agent permissions", exact: true });
  assert.equal(await edit.getByLabel("Edit shared rules", { exact: true }).isChecked(), true);
  await edit.getByLabel("Name", { exact: true }).fill("Human and agent collaboration");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.mcpFailure =
      "Permissions could not be saved. Retry after checking Transmog.";
  });
  await edit.getByRole("button", { name: "Save permissions", exact: true }).click();
  await edit.getByRole("alert").filter({ hasText: "Permissions could not be saved" }).waitFor();
  assert.equal(
    await edit.getByLabel("Name", { exact: true }).inputValue(),
    "Human and agent collaboration",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/mcp-save-failure.png") });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.mcpFailure = null;
  });
  await edit.getByRole("button", { name: "Save permissions", exact: true }).click();
  await edit.waitFor({ state: "hidden" });
  await panel.getByRole("button", { name: "Revoke…", exact: true }).click();
  const confirmation = panel.getByRole("dialog", { name: "Revoke access", exact: true });
  await confirmation.getByRole("button", { name: "Revoke access", exact: true }).click();
  await panel.getByText("Revoked", { exact: true }).waitFor();
  assert.equal(
    await panel
      .getByRole("button", { name: "Create access key…", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  await panel.getByRole("button", { name: "Remove record", exact: true }).click();
  await panel.locator("article").waitFor({ state: "hidden" });
  await panel.getByLabel("Allow MCP connections", { exact: true }).uncheck();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/mcp-settings.png") });
  await page.getByRole("link", { name: /Automation/ }).click();
  await page.waitForFunction(
    () => !!document.querySelector("app-shell").automation?.autoresponseState,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.automation = structuredClone(
      document.querySelector("app-shell").automation.autoresponseState,
    );
  });
  await newTrafficRule(page, "Change response headers…");
  const ruleEditor = page.locator('header-rules[editor-kind="headers"]');
  await ruleEditor.getByLabel("Rule name", { exact: true }).fill("Shared with the agent");
  await ruleEditor.getByLabel("Header operation", { exact: true }).selectOption("set");
  await ruleEditor.getByLabel("Operation header name", { exact: true }).fill("X-Collaboration");
  await ruleEditor.getByLabel("Operation header value", { exact: true }).fill("saved value");
  await ruleEditor.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.headerRulesEditor.hrBusy,
  );
  await ruleEditor
    .getByLabel("Rule name", { exact: true })
    .fill("Unsaved human draft must survive");
  await page.evaluate(() => {
    const editor = document.querySelector("app-shell").automation.headerRulesEditor;
    const state = globalThis.__workspaceFixture;
    const rule = state.automation.rules.find((rule) => rule.id === editor.hrId);
    rule.displayName = "Agent saved a newer revision";
    rule.revision++;
    state.automation.generation++;
    state.emitNative("mcp-automation-changed");
  });
  await page.waitForFunction(
    () => document.querySelector("app-shell").automation.headerRulesEditor.hrStale,
  );
  assert.equal(
    await ruleEditor.getByLabel("Rule name", { exact: true }).inputValue(),
    "Unsaved human draft must survive",
  );
  assert.equal(
    await page.evaluate(
      () =>
        globalThis.__workspaceFixture.automation.rules.find(
          (rule) => rule.displayName === "Agent saved a newer revision",
        ).revision > 1,
    ),
    true,
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/mcp-collaboration.png") });
  await page.goto("https://workspace.test/");
}

async function checkSensitiveApprovals(page, root, panel) {
  const indicator = page.locator("mcp-indicator");
  const request = async (id) =>
    page.evaluate((id) => {
      const state = globalThis.__workspaceFixture;
      const grant = state.mcp.grants[0];
      state.mcpSensitive = [
        {
          id,
          grantId: grant.id,
          revision: grant.revision,
          name: grant.name,
          scope: grant.scope,
          reason: "Inspect an expired token response without losing the current human draft.",
          state: "pending",
          remainingSeconds: 300,
        },
      ];
      state.emitNative("mcp-access-changed");
    }, id);
  await request("first");
  const button = indicator.getByRole("button", { name: /MCP · 1 access request/ });
  await button.waitFor();
  assert.equal(
    await indicator.getByRole("dialog").isVisible(),
    false,
    "An agent request stole focus with an automatic popup",
  );
  await button.focus();
  await page.keyboard.press("Enter");
  const access = indicator.getByRole("dialog", { name: "Agent access", exact: true });
  for (const width of [1440, 960, 800]) {
    await page.setViewportSize({ width, height: width === 800 ? 600 : 900 });
    assert.equal(
      await access.evaluate((panel) => panel.scrollWidth <= panel.clientWidth + 1),
      true,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/mcp-sensitive-request-${width}.png`),
    });
  }
  await access.getByRole("button", { name: "For next 1 minute", exact: true }).click();
  await indicator.getByRole("button", { name: /MCP · original values 1:00/ }).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.mcp.grants[0].omitSensitiveValues),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/mcp-sensitive-minute.png"),
  });
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferMcpSummary = true;
    void document.querySelector("app-shell").shadowRoot.querySelector("mcp-indicator").refresh();
  });
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishMcpSummary);
  await access.getByRole("button", { name: "Revoke access now", exact: true }).click();
  await access
    .getByText("No original-value requests or temporary approvals.", { exact: false })
    .waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.finishMcpSummary());
  assert.equal(
    await indicator.evaluate((component) => component.mcpSensitiveTemporary.length),
    0,
    "A late status response restored revoked access in the UI",
  );
  await page.keyboard.press("Escape");
  await access.waitFor({ state: "hidden" });
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .shadowRoot.querySelector("mcp-indicator")
        .querySelector("button[popovertarget]")
        .matches(":focus"),
    {},
    { timeout: 3000 },
  );
  assert.equal(
    await indicator
      .getByRole("button", { name: "MCP enabled", exact: true })
      .evaluate((button) => button.matches(":focus")),
    true,
  );
  await request("permanent");
  await button.click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.mcpSensitiveFailure =
      "Could not save approval. Retry after checking Transmog.";
  });
  await access.getByRole("button", { name: "Always", exact: true }).click();
  await access.getByRole("alert").filter({ hasText: "Could not save approval" }).waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.mcp.grants[0].omitSensitiveValues),
    true,
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.mcpSensitiveFailure = null;
  });
  await access.getByRole("button", { name: "Always", exact: true }).click();
  await access
    .getByText("No original-value requests or temporary approvals.", { exact: false })
    .waitFor();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.mcp.grants[0].omitSensitiveValues),
    false,
  );
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").settings.querySelector("mcp-settings").mcpRows[0]
        ?.omitSensitiveValues === false,
  );
  await panel.getByRole("button", { name: "Edit permissions…", exact: true }).click();
  const editor = panel.getByRole("dialog", { name: "Edit agent permissions", exact: true });
  assert.equal(
    await editor.getByLabel("Omit authentication headers and bodies", { exact: true }).isChecked(),
    false,
  );
  // Restore protection through the saved setting before leaving this flow.
  await editor.getByLabel("Omit authentication headers and bodies", { exact: true }).check();
  await editor.getByRole("button", { name: "Save permissions", exact: true }).click();
  await editor.waitFor({ state: "hidden" });
  await page.setViewportSize({ width: 1440, height: 900 });
}
