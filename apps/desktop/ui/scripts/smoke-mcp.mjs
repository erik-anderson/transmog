import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";

// The owning PowerShell harness supplies an isolated user profile. Keys created
// here are disposable test credentials; never print their values in evidence.
export async function smokeMcp(call, evaluate, screenshotPath) {
  const invoke = (command, args = {}) =>
    evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`,
    );
  const initial = await invoke("mcp_status");
  let grantId;
  const cli = async (key, tool, input) => {
    const child = spawn(initial.cliCommand, ["mcp", "call", tool], {
      windowsHide: true,
      env: { ...process.env, TRANSMOG_ACCESS_KEY: key },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (data) => {
      output += data;
      if (output.length > 2 * 1024 * 1024) child.kill();
    });
    // Do not echo process stderr: failure evidence must not contain test keys.
    child.stderr.resume();
    const timer = setTimeout(() => child.kill(), 15000);
    child.stdin.end(JSON.stringify(input));
    try {
      const code = await new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("exit", resolve);
      });
      if (code !== 0) return { failed: true };
      return JSON.parse(output);
    } finally {
      clearTimeout(timer);
    }
  };
  try {
    await invoke("mcp_set_enabled", { enabled: true });
    const created = await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell');await shell.activateView('settings');
      const settings=shell.settings;settings.showSettingsSection('integrations');
      const panel=settings.querySelector('mcp-settings');await panel.ready;await panel.refresh();panel.create();
      const form=panel.mcpForm;form.elements.namedItem('name').value='Native MCP collaboration';
      form.elements.namedItem('scope').value='hosts';form.elements.namedItem('hosts').value='example.test';
      form.elements.namedItem('edit-shared-rules').checked=true;panel.changed();
      await panel.save(new Event('submit',{cancelable:true}));
      if(!panel.mcpKeyDialog.open||!panel.mcpSecret)throw new Error('Native key creation did not show its one-time secret');
      if(form.elements.namedItem('read-rules'))throw new Error('Rule reads require an unintended permission');
      const result={key:panel.mcpSecret,id:panel.mcpRows.at(-1).id};panel.closeKey();
      await new Promise(resolve=>setTimeout(resolve,25));
      if(panel.mcpSecret||panel.mcpConfig)throw new Error('Closing native key dialog retained secret configuration');
      return result;
    })()`);
    grantId = created.id;
    const status = await cli(created.key, "transmog_connect", {});
    assert.equal(
      status.connected,
      true,
      "Native CLI could not authenticate to the loopback endpoint",
    );
    const rules = await cli(created.key, "transmog_rules_list", { instanceId: status.instanceId });
    assert(Array.isArray(rules.rules), "An authenticated key could not read rules");
    assert.equal(
      await evaluate(`(async()=>{
      const indicator=document.querySelector('app-shell').shadowRoot.querySelector('mcp-indicator');
      await indicator.refresh();
      return indicator.mcpIndicatorText==='MCP · 1 connected' && indicator.mcpActiveAgents[0]?.name==='Native MCP collaboration';
    })()`),
      true,
      "Native connection indicator did not identify the authenticated key",
    );
    assert.equal(status.sensitiveAccess.headersAllowed, false);
    assert.equal(status.sensitiveAccess.bodiesAllowed, false);
    const requested = await cli(created.key, "transmog_sensitive_access_request", {
      reason: "Inspect the unredacted authentication values for the native qualification.",
    });
    assert.equal(requested.request.state, "pending");
    assert.equal(requested.headersAllowed, false);
    const respond = async (choice) =>
      evaluate(`(async()=>{
      const indicator=document.querySelector('app-shell').shadowRoot.querySelector('mcp-indicator');
      await indicator.refresh();
      if(!indicator.mcpAccessPanel.matches(':popover-open'))indicator.querySelector('button[popovertarget]').click();
      const approval=indicator.mcpSensitiveRequests[0];
      if(!approval)throw new Error('The native access request is missing');
      const label=${JSON.stringify(choice)}==='minute'?'For next 1 minute':'Always';
      const button=[...indicator.mcpAccessPanel.querySelectorAll('button')].find(button=>button.textContent===label);
      if(!button)throw new Error('The requested native consent action is missing');
      button.click();
      const deadline=Date.now()+10000;
      while(indicator.mcpSensitiveBusy){if(Date.now()>deadline)throw new Error('Native consent timed out');await new Promise(resolve=>setTimeout(resolve,20));}
      if(indicator.mcpSensitiveError)throw new Error(indicator.mcpSensitiveError);
      return true;
    })()`);
    await respond("minute");
    const temporary = await cli(created.key, "transmog_sensitive_access_status", {});
    assert.equal(temporary.headersAllowed, true);
    assert.equal(temporary.bodiesAllowed, true);
    assert.equal(temporary.omitSensitiveValues, true);
    assert.equal(
      await evaluate(`(async()=>{
      const indicator=document.querySelector('app-shell').shadowRoot.querySelector('mcp-indicator');
      await indicator.refresh();
      const badge=indicator.querySelector('.mcp-access-badge');
      const label=indicator.querySelector('.mcp-indicator-label');
      return badge.textContent.startsWith('Unredacted · ') && label.textContent.includes('connected') && badge.getBoundingClientRect().top < label.getBoundingClientRect().top;
    })()`),
      true,
      "Native unredacted badge did not stay above connection status",
    );
    if (screenshotPath) {
      const image = await call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-mcp-sensitive-minute.png"),
        Buffer.from(image.data, "base64"),
      );
    }
    await evaluate(`(async()=>{
      const indicator=document.querySelector('app-shell').shadowRoot.querySelector('mcp-indicator');
      await indicator.revoke(indicator.mcpSensitiveTemporary[0].id);
      if(indicator.mcpSensitiveError)throw new Error(indicator.mcpSensitiveError);
    })()`);
    assert.equal(
      (await cli(created.key, "transmog_sensitive_access_status", {})).headersAllowed,
      false,
    );
    await cli(created.key, "transmog_sensitive_access_request", {
      reason: "Test the explicit persistent unredacted decision.",
    });
    await respond("always");
    const permanent = await cli(created.key, "transmog_sensitive_access_status", {});
    assert.equal(permanent.omitSensitiveValues, false);
    assert.equal(permanent.bodiesAllowed, true);
    await evaluate(
      `document.querySelector('app-shell').shadowRoot.querySelector('mcp-indicator').mcpAccessPanel.hidePopover()`,
    );
    assert.equal(
      (await cli("invalid-caller-secret", "transmog_status", {})).failed,
      true,
      "Another caller reused an existing connection",
    );
    const current = await invoke("mcp_status");
    assert.match(current.endpoint, /^https:\/\/127\.0\.0\.1:\d+$/);
    assert.equal(
      current.grants
        .find((grant) => grant.id === grantId)
        .permissions.includes("edit-shared-rules"),
      true,
    );
    assert.equal(
      JSON.stringify(current).includes(created.key),
      false,
      "Management status disclosed the secret",
    );
    if (screenshotPath) {
      const result = await call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshotPath.replace(/\.png$/i, "-mcp.png"),
        Buffer.from(result.data, "base64"),
      );
    }
    const grant = current.grants.find((grant) => grant.id === grantId);
    const replacement = await invoke("mcp_replace_key", { id: grantId, revision: grant.revision });
    assert.equal((await cli(created.key, "transmog_status", {})).failed, true);
    assert.equal((await cli(replacement.key, "transmog_status", {})).connected, true);
    const latest = replacement.status.grants.find((grant) => grant.id === grantId);
    await invoke("mcp_revoke_key", { id: grantId, revision: latest.revision });
    assert.equal((await cli(replacement.key, "transmog_status", {})).failed, true);
    return {
      nativePermissionDialog: true,
      callerIsolation: true,
      loopbackOnly: true,
      ruleReadsWithoutPermission: true,
      keyReplacementAndRevocation: true,
      defaultOmission: true,
      temporarySensitiveAccess: true,
      earlySensitiveRevocation: true,
      persistentSensitiveSetting: true,
      connectionIndicator: true,
      unredactedValuesBadge: true,
    };
  } finally {
    if (grantId) {
      let current = await invoke("mcp_status");
      let grant = current.grants.find((grant) => grant.id === grantId);
      if (grant && !grant.revoked) {
        await invoke("mcp_revoke_key", { id: grantId, revision: grant.revision });
        current = await invoke("mcp_status");
        grant = current.grants.find((grant) => grant.id === grantId);
      }
      if (grant) await invoke("mcp_remove_key", { id: grantId, revision: grant.revision });
    }
    await invoke("mcp_set_enabled", { enabled: initial.enabled });
  }
}
