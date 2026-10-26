import assert from "node:assert/strict";
import { resolve } from "node:path";
import { waitForBodySource, waitForBodyText } from "./check-body-source.mjs";

export async function checkConnectTunnels(page, root) {
  const original = await page.evaluate(() =>
    structuredClone(document.querySelector("app-shell").traffic.selectedDetail),
  );
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
  const apply = async (method, transports) =>
    page.evaluate(
      ({ original, method, transports }) => {
        const workspace = document.querySelector("app-shell").traffic,
          detail = structuredClone(original);
        detail.requests[0].method = method;
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
      { original, method, transports },
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
  } finally {
    await page.evaluate((original) => {
      globalThis.__workspaceFixture.performance = original.performance;
      document.querySelector("app-shell").traffic.applyDetail(original);
    }, original);
    await page.setViewportSize({ width: 800, height: 600 });
  }
}
