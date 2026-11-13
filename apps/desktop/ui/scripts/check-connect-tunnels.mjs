import assert from "node:assert/strict";
import { resolve } from "node:path";
import { waitForBodySource, waitForBodyText } from "./check-body-source.mjs";

export async function checkConnectTunnels(page, root) {
  const original = await page.evaluate(() =>
    structuredClone(document.querySelector("app-shell").traffic.selectedDetail),
  );
  const savedFixture = await page.evaluate(() => ({
    sessions: structuredClone(globalThis.__workspaceFixture.sessions),
    fullHeaders: globalThis.__workspaceFixture.fullHeaders,
  }));
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const timing = page.locator(".timing-dialog[open]");
  const tls = {
    tlsVersion: "TLSv1.3",
    tlsMicros: 1200,
    cipher: "TLS_AES_128_GCM_SHA256",
    alpn: "h2",
    tlsResumed: false,
  };
  const client = {
    leg: "client",
    connectionId: "connect-client",
    outcome: "connected",
    tunnelDestination: "media.stores.com:443",
    ...tls,
  };
  const upstream = {
    leg: "upstream",
    connectionId: "connect-origin",
    outcome: "connected",
    ...tls,
  };
  const apply = async (method, transports, routeSelection = null) =>
    page.evaluate(
      ({ original, method, transports, routeSelection }) => {
        const workspace = document.querySelector("app-shell").traffic,
          detail = structuredClone(original);
        detail.requests[0].method = method;
        detail.routeSelection = routeSelection;
        detail.storedBodies = [
          {
            exchangeId: detail.id,
            boundary: "client-response",
            observedBytes: 4,
            retainedBytes: 4,
            availability: "complete",
            mediaType: "text/plain",
            charset: "utf-8",
            contentCodings: [],
            sha256: null,
            reason: null,
          },
        ];
        detail.performance = { points: [], protocols: [], transports };
        globalThis.__workspaceFixture.performance = detail.performance;
        workspace.applyDetail(detail);
        workspace.querySelector('message-inspector[side="response"]').showMode("body");
      },
      { original, method, transports, routeSelection },
    );
  const body = async () => {
    await waitForBodyText(inspector, "body for " + original.id);
    await waitForBodySource(inspector);
  };
  try {
    // Intercepted HTTPS retains the CONNECT authority on the client leg.
    await apply("GET", [client, upstream]);
    await body();
    assert.equal(await inspector.getByText(/HTTPS decryption was disabled/).count(), 0);
    await page
      .locator(".selection-actions")
      .getByRole("button", { name: "Details…", exact: true })
      .click();
    await timing.getByRole("tab", { name: "Connection", exact: true }).click();
    await timing.getByText("Client ↔ proxy", { exact: true }).click();
    await timing.getByText("CONNECT destination", { exact: true }).waitFor({ state: "visible" });
    await timing.getByText("TLSv1.3", { exact: true }).first().waitFor({ state: "visible" });
    assert.equal(await timing.getByText(/Opaque TCP tunnel/).count(), 0);
    assert.equal(await timing.getByText("TLS handshake (connection)", { exact: true }).count(), 2);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/decrypted-connect-wide.png"),
    });
    await page.setViewportSize({ width: 800, height: 600 });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/decrypted-connect-compact.png"),
    });
    await page.keyboard.press("Escape");
    await body();
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/decrypted-body-compact.png"),
    });
    // A parsed plaintext request inside CONNECT also has an inspectable body.
    await apply("GET", [{ ...client, tlsVersion: null }, upstream]);
    await body();
    // Imported TLS connection summaries may themselves use CONNECT.
    await apply("CONNECT", [client, { ...upstream, tunnelDestination: client.tunnelDestination }]);
    await body();
    // Public TLS evidence is sufficient even when a legacy capture omits version.
    await apply("CONNECT", [
      {
        ...upstream,
        tunnelDestination: client.tunnelDestination,
        tlsVersion: null,
        tls: { serverName: "media.stores.com" },
      },
    ]);
    await body();
    await apply("CONNECT", [
      {
        leg: "upstream",
        connectionId: "opaque",
        outcome: "connected",
        tunnelDestination: "media.stores.com:443",
        bytesRead: 57,
        bytesWritten: 31,
      },
    ]);
    await inspector
      .getByText(/Opaque CONNECT tunnel to media.stores.com:443/)
      .waitFor({ state: "visible" });
    await page
      .locator(".selection-actions")
      .getByRole("button", { name: "Details…", exact: true })
      .click();
    await timing.getByRole("tab", { name: "Connection", exact: true }).click();
    await timing.getByText("Proxy ↔ upstream", { exact: true }).click();
    await timing.getByText("CONNECT destination", { exact: true }).waitFor({ state: "visible" });
    assert.equal(
      await timing.getByText("TLS version", { exact: true }).count(),
      0,
      "Opaque tunnel reported a TLS protocol",
    );
    assert.equal(await timing.getByText("TLS handshake (connection)", { exact: true }).count(), 0);
    await page.keyboard.press("Escape");
    await apply(
      "CONNECT",
      [
        {
          leg: "upstream",
          connectionId: "excluded-chrome",
          outcome: "connected",
          tunnelDestination: "media.stores.com:443",
          bytesRead: 57,
          bytesWritten: 31,
        },
      ],
      "process-bypass-tunnel: Process excluded from inspection; forwarding an opaque TCP tunnel without retaining CONNECT headers or contents",
    );
    await inspector.getByText(/The caller was excluded by the process filter/).waitFor();
    assert.equal(await inspector.getByText(/HTTPS decryption was disabled/).count(), 0);
    assert.equal(await inspector.getByText(/CONNECT headers were not recorded/).count(), 1);
    assert.equal(await inspector.getByText("body for " + original.id, { exact: true }).count(), 0);
    await page.evaluate(async (id) => {
      const state = globalThis.__workspaceFixture,
        workspace = document.querySelector("app-shell").traffic;
      const detail = structuredClone(workspace.selectedDetail);
      state.sessions = state.sessions.map((row) =>
        row.id === id
          ? {
              ...row,
              method: "CONNECT",
              host: "media.stores.com",
              url: "https://media.stores.com:443/",
              path: "/",
              caller: { kind: "local-process", processName: "chrome.exe", processId: 42 },
              terminal: "active",
              contentType: null,
              topLevelNavigation: false,
              fetchDestination: null,
              responseBytes: 0,
            }
          : row,
      );
      state.fullHeaders = [];
      await workspace.revealSession(id);
      detail.requests[0].target = "https://media.stores.com:443/";
      detail.requests[0].headers = [];
      detail.responses[0].headers = [];
      detail.caller = { kind: "local-process", processName: "chrome.exe", processId: 42 };
      detail.storedBodies = [];
      workspace.applyDetail(detail);
    }, original.id);
    const row = page.locator(`tr[data-session-id="${original.id}"]`);
    await row.getByText("CONNECT", { exact: true }).waitFor();
    await row.getByText("chrome.exe (42)", { exact: true }).waitFor();
    await inspector.getByText(/The caller was excluded by the process filter/).waitFor();
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await page.setViewportSize({ width, height });
      await page.screenshot({
        path: resolve(root, `../../../target/ui-check/excluded-connect-${width}.png`),
      });
    }
  } finally {
    await page.evaluate(
      async ({ original, savedFixture }) => {
        globalThis.__workspaceFixture.sessions = savedFixture.sessions;
        globalThis.__workspaceFixture.fullHeaders = savedFixture.fullHeaders;
        globalThis.__workspaceFixture.performance = original.performance;
        const workspace = document.querySelector("app-shell").traffic;
        await workspace.revealSession(original.id);
        workspace.applyDetail(original);
      },
      { original, savedFixture },
    );
    await page.setViewportSize({ width: 800, height: 600 });
  }
}
