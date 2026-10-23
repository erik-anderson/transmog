import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { X509Certificate, createHash } from "node:crypto";
import { certificateAlpsFixture } from "./certificate-alps-fixture.mjs";

export async function checkCertificates(page, root) {
  const alpsFixture = certificateAlpsFixture();
  const certificates = [];
  for (const name of ["leaf", "root"]) {
    const pem = await readFile(
        resolve(root, "../../../crates/app/tests/fixtures/certificates/" + name + ".pem"),
        "utf8",
      ),
      certificate = new X509Certificate(pem);
    certificates.push({
      sha256: createHash("sha256").update(certificate.raw).digest("hex"),
      subject: certificate.subject,
      fields: [
        { label: "Subject", value: certificate.subject },
        { label: "Issuer", value: certificate.issuer },
        { label: "SHA-256", value: createHash("sha256").update(certificate.raw).digest("hex") },
      ],
      pem,
      derBase64: certificate.raw.toString("base64"),
      unavailable: "",
    });
  }
  const inspection = {
    certificates,
    connections: [
      {
        id: "upstream:failed",
        title: "Transmog → server",
        fields: [
          { label: "Recorded verification", value: "Rejected (code 20, depth 1)" },
          { label: "Offered cipher suites", value: "ECDHE-ECDSA-AES128-GCM-SHA256" },
        ],
        serverChain: certificates.map((certificate) => certificate.sha256),
        clientChain: [certificates[0].sha256],
        errors: ["Certificate issuer was not trusted by the recorded snapshot"],
      },
      {
        id: "client:browser",
        title: "Client → Transmog",
        fields: [
          { label: "Recorded verification", value: "Not recorded or verification was not reached" },
        ],
        serverChain: certificates.map((certificate) => certificate.sha256),
        clientChain: [],
        errors: [],
      },
    ],
  };
  inspection.connections[0].fields.push({ label: "Key exchange", value: "Unavailable" });
  inspection.connections[1].fields.push(
    { label: "Key exchange", value: "Hybrid post-quantum key exchange" },
    { label: "Negotiated group", value: "X25519MLKEM768" },
    { label: "Negotiated group ID", value: "0x11ec" },
    { label: "Local handshake signature", value: "mldsa44" },
  );
  inspection.connections.push({
    id: "upstream:resumed",
    title: "Transmog → server (resumed)",
    fields: [
      { label: "Negotiated TLS version", value: "TLSv1.2" },
      { label: "Session resumed", value: "Yes" },
      { label: "Key exchange", value: "Not performed (TLS 1.2 session resumed)" },
      { label: "Original session group", value: "P-256" },
      { label: "Original session group ID", value: "0x0017" },
      { label: "Peer handshake signature", value: "Not sent (session resumed)" },
    ],
    serverChain: certificates.map((certificate) => certificate.sha256),
    clientChain: [],
    errors: [],
  });
  inspection.defaultConnectionId = "upstream:connected";
  inspection.connections[0].title = "Transmog → server · TLS failed · failed";
  inspection.connections.push({
    id: "upstream:connected",
    title: "Transmog → server · Connected · origin",
    fields: [
      { label: "Outcome", value: "Connected" },
      { label: "Recorded verification", value: "Verified by the original connection" },
      { label: "Key exchange", value: "Hybrid post-quantum key exchange" },
      { label: "Negotiated group", value: "X25519MLKEM768" },
      { label: "Negotiated group ID", value: "0x11ec" },
      { label: "Peer handshake signature", value: "mldsa65" },
    ],
    serverChain: certificates.map((certificate) => certificate.sha256),
    clientChain: [],
    errors: [],
  });
  Object.assign(inspection.connections[3], {
    summary: "TLSv1.3 · X25519MLKEM768 · TLS_AES_128_GCM_SHA256",
    fields: [
      { label: "Outcome", value: "Connected" },
      { label: "Negotiated TLS version", value: "TLSv1.3" },
      { label: "Session resumed", value: "No" },
      { label: "Key exchange", value: "Hybrid post-quantum key exchange" },
      { label: "Negotiated group", value: "X25519MLKEM768" },
      { label: "Negotiated group ID", value: "0x11ec" },
      { label: "Peer handshake signature", value: "mldsa65" },
      { label: "Negotiated cipher", value: "TLS_AES_128_GCM_SHA256" },
      { label: "ALPN", value: "h2" },
      { label: "HelloRetryRequest", value: "No" },
      { label: "Recorded verification", value: "Verified by the original connection" },
    ],
    policyFields: [
      { label: "Connection", value: "origin" },
      { label: "Peer", value: "127.0.0.1:443" },
      { label: "Server name", value: "localhost" },
      { label: "Chain source", value: "verification-path" },
      { label: "Recorded trust source", value: "Recorded connection trust snapshot" },
      { label: "Recorded trust generation", value: "42" },
      { label: "TLS version bounds", value: "TLS1.2, TLS1.3" },
      {
        label: "Available cipher suites",
        value: Array(32)
          .fill("ECDHE-ECDSA-AES128-GCM-SHA256, ECDHE-RSA-AES256-GCM-SHA384")
          .join(", "),
      },
      { label: "Offered ALPN", value: "h2, http/1.1" },
    ],
    diagnosticFields: [
      { label: "https-server-cipher", value: "TLS_AES_128_GCM_SHA256" },
      { label: "https-server-protocol", value: "TLSv1.3" },
      { label: "https-server-sni", value: "localhost" },
    ],
  });
  inspection.connections.push({
    id: "upstream:controls",
    title: "Recorded server connection · Connected · controls",
    summary: "TLSv1.3 · X25519\\u{0000}MLKEM768 · TLS_AES_128_GCM_SHA256",
    fields: [
      {
        label: "Negotiated group",
        value: "X25519\0MLKEM768",
        displayValue: "X25519\\u{0000}MLKEM768",
        displayNote: "Invisible characters shown as Unicode escapes.",
      },
    ],
    policyFields: [],
    diagnosticFields: [
      { label: "Duplicate label", value: "first" },
      { label: "Duplicate label", value: "second" },
    ],
    serverChain: [],
    clientChain: [],
    errors: [],
  });
  inspection.connections[3].alps = alpsFixture.view;
  inspection.connections[3].policyFields.push(
    { label: "Certificate Transparency", value: "Compliant" },
    { label: "Verified SCTs", value: "3" },
    {
      label: "CT log list source",
      value:
        "C:\\Users\\Fixture\\AppData\\Local\\Microsoft\\Windows\\SharedWebView\\EBWebView\\PKIMetadata\\46.0.0.0\\ct_config.pb",
    },
    { label: "CT log list updated", value: "2026-10-04T12:00:00Z" },
    { label: "CRLSet result", value: "Inconclusive" },
    { label: "CRLSet sequence", value: "187" },
    { label: "Stapled OCSP", value: "Not provided" },
  );
  inspection.connections[3].fields.push({
    label: "HTTP/2 ALPS",
    value: `Negotiated (${alpsFixture.bytes.length} peer bytes)`,
  });
  const noTrustAnchorIds = "Not offered; selected metadata has no supported Trust Anchor IDs";
  inspection.connections[3].fields.push({ label: "Trust Anchor IDs", value: noTrustAnchorIds });
  inspection.connections.push({
    id: "upstream:alps-empty",
    title: "Transmog → server · Connected · empty ALPS",
    fields: [{ label: "HTTP/2 ALPS", value: "Negotiated (0 peer bytes)" }],
    alps: {
      peerBytes: 0,
      fields: [{ label: "Server value", value: "Empty (ALPS was negotiated)" }],
      notes: [],
      payloadBase64: "",
    },
    serverChain: [],
    clientChain: [],
    errors: [],
  });
  await page.evaluate(
    (inspection) => (globalThis.__workspaceFixture.certificateInspection = inspection),
    inspection,
  );
  const trigger = page
    .locator(".selection-actions")
    .getByRole("button", { name: "Details…", exact: true });
  await trigger.click();
  await page.getByRole("tab", { name: "Certificates", exact: true }).click();
  const dialog = page.locator(".details-dialog[open]");
  await page.getByRole("tab", { name: "Connection", exact: true }).click();
  const connection = dialog.getByRole("combobox", { name: "Certificate connection" });
  await dialog.getByText("mldsa65", { exact: true }).waitFor();
  assert.equal(
    await connection.inputValue(),
    "upstream:connected",
    "The successful origin was not the default",
  );
  assert.equal(
    await dialog.getByText(inspection.connections[0].errors[0], { exact: true }).count(),
    0,
    "A failed attempt was shown as the current connection",
  );
  const titles = await connection.locator("option").allTextContents();
  assert.equal(
    new Set(titles).size,
    titles.length,
    "Connection attempts have indistinguishable labels",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/tls-fallback-wide.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  const noIdStatus = dialog.getByText(noTrustAnchorIds, { exact: true });
  await noIdStatus.scrollIntoViewIfNeeded();
  assert.ok(
    await noIdStatus.evaluate((node) => {
      const rect = node.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= innerWidth;
    }),
    "Empty-catalog reason was clipped in the compact connection view",
  );
  assert.equal(
    await dialog.getByText("Requested Trust Anchor IDs", { exact: true }).count(),
    0,
    "An omitted offer acquired a requested-ID row",
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/trust-anchor-ids-omitted-compact.png"),
  });
  await dialog.getByLabel("Recorded TLS summary", { exact: true }).scrollIntoViewIfNeeded();
  const summary = dialog.getByLabel("Recorded TLS summary", { exact: true });
  assert.equal(await summary.textContent(), inspection.connections[3].summary);
  assert.equal(
    await summary.evaluate((node) => {
      const rect = node.getBoundingClientRect();
      return rect.top >= 0 && rect.bottom <= innerHeight && rect.height > 0;
    }),
    true,
    "The compact connection view buried its TLS summary",
  );
  const policy = dialog
    .locator("details")
    .filter({ has: page.getByText("Connection and recorded policy", { exact: true }) });
  const metadata = dialog
    .locator("details")
    .filter({ has: page.getByText("Additional recorded metadata", { exact: true }) });
  assert.equal(await policy.evaluate((node) => node.open), false);
  assert.equal(await metadata.evaluate((node) => node.open), false);
  const alps = dialog.locator(".certificate-alps");
  assert.equal(await alps.evaluate((node) => node.open), false);
  await alps.locator("summary").first().focus();
  await page.keyboard.press("Enter");
  await alps
    .getByText("Sec-CH-UA-Platform, Sec-CH-UA-Model", { exact: true })
    .waitFor({ state: "visible" });
  await alps.locator(".certificate-alps-raw > summary").click();
  await alps.locator(".hex-row").first().waitFor();
  assert.deepEqual(
    await alps.locator("hex-viewer").evaluate((node) => Array.from(node.preview.bytes)),
    Array.from(Buffer.from(alpsFixture.view.payloadBase64, "base64")),
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/alps-contents-compact.png"),
  });
  await connection.selectOption("upstream:alps-empty");
  await dialog.getByText("Empty (ALPS was negotiated)", { exact: true }).waitFor();
  assert.equal(await dialog.locator(".certificate-alps-raw").count(), 0);
  await connection.selectOption("upstream:failed");
  assert.equal(
    await dialog.locator(".certificate-alps").count(),
    0,
    "Prior server ALPS contents leaked into a failed attempt",
  );
  assert.equal(
    await dialog.getByText(noTrustAnchorIds, { exact: true }).count(),
    0,
    "Prior Trust Anchor ID status leaked into another attempt",
  );
  await connection.selectOption("upstream:connected");
  await policy.locator("summary").focus();
  await page.keyboard.press("Enter");
  await policy.getByText("Available cipher suites", { exact: true }).waitFor({ state: "visible" });
  await policy.getByText("Compliant", { exact: true }).scrollIntoViewIfNeeded();
  assert.equal(await metadata.getByText("Certificate Transparency", { exact: true }).count(), 0);
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/ct-policy-compact.png") });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/ct-policy-wide.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  await policy.locator("summary").click();
  await metadata.locator("summary").click();
  await metadata.getByText("https-server-cipher", { exact: true }).waitFor({ state: "visible" });
  await metadata.locator("summary").click();
  await dialog.getByText("mldsa65", { exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/tls-fallback-compact.png"),
  });
  await connection.selectOption("upstream:failed");
  await connection.selectOption("upstream:controls");
  await dialog.locator("dd").filter({ hasText: "X25519\\u{0000}MLKEM768" }).waitFor();
  await dialog
    .getByText("Invisible characters shown as Unicode escapes.", { exact: true })
    .waitFor();
  const rawDetails = dialog
    .locator("details")
    .filter({ has: page.getByText("Additional recorded metadata", { exact: true }) });
  await rawDetails.locator("summary").click();
  assert.deepEqual(
    await rawDetails.locator("dd").allTextContents(),
    ["first", "second"],
    "Duplicate raw labels lost a metadata row",
  );
  assert.equal(
    await dialog
      .locator("dd")
      .evaluateAll((nodes) => nodes.some((node) => node.textContent.includes("\0"))),
    false,
  );
  await connection.selectOption("upstream:failed");
  await dialog.getByText("Unavailable", { exact: true }).waitFor();
  await dialog.getByText(inspection.connections[0].errors[0], { exact: true }).waitFor();
  await connection.selectOption("client:browser");
  await dialog.getByText("Hybrid post-quantum key exchange", { exact: true }).waitFor();
  await dialog.getByText("X25519MLKEM768", { exact: true }).waitFor();
  await dialog.getByText("mldsa44", { exact: true }).waitFor();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/pqc-connection-wide.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await dialog.getByText("mldsa44", { exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/pqc-connection-compact.png"),
  });
  await connection.focus();
  await page.keyboard.press("ArrowUp");
  await dialog.getByText("Unavailable", { exact: true }).waitFor();
  assert.equal(
    await dialog.getByText("Hybrid post-quantum key exchange", { exact: true }).count(),
    0,
    "Client PQC evidence leaked into the origin connection",
  );
  await connection.selectOption("upstream:resumed");
  await dialog.getByText("Not performed (TLS 1.2 session resumed)", { exact: true }).waitFor();
  await dialog.getByText("Original session group", { exact: true }).waitFor();
  await dialog.getByText("P-256", { exact: true }).waitFor();
  assert.equal(
    await dialog.getByText("Negotiated group", { exact: true }).count(),
    0,
    "TLS 1.2 resumption claimed a new group",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/tls12-resumption-wide.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await dialog.getByText("Not sent (session resumed)", { exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/tls12-resumption-compact.png"),
  });
  await connection.selectOption("upstream:failed");
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.getByRole("tab", { name: "Certificates", exact: true }).click();
  await dialog.getByText(inspection.connections[0].errors[0], { exact: true }).waitFor();
  await dialog.getByText("Client authentication certificate chain", { exact: true }).waitFor();
  await dialog.locator(".certificate-chain").first().getByRole("button").nth(1).click();
  assert.match(await dialog.locator(".certificate-subject").textContent(), /fixture root/);
  await dialog.getByRole("button", { name: "PEM", exact: true }).click();
  assert.match(await dialog.locator(".certificate-pem").textContent(), /BEGIN CERTIFICATE/);
  await dialog.getByRole("button", { name: "Copy PEM", exact: true }).click();
  await dialog.getByText("Public certificate PEM copied.", { exact: true }).waitFor();
  assert.match(await page.evaluate(() => navigator.clipboard.readText()), /BEGIN CERTIFICATE/);
  await dialog.getByRole("button", { name: "Hex", exact: true }).click();
  await dialog.locator(".hex-row").first().waitFor();
  assert.equal(
    await dialog
      .locator(".hex-row")
      .first()
      .evaluate((node) => {
        const rect = node.getBoundingClientRect();
        return rect.height > 0 && rect.y >= 0 && rect.bottom <= innerHeight;
      }),
    true,
    "The selected certificate Hex rows were left outside the visible dialog",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/certificates-wide.png") });
  await page.setViewportSize({ width: 640, height: 520 });
  const bounds = await dialog.boundingBox();
  assert.ok(bounds.width <= 640 && bounds.height <= 520);
  assert.equal(
    await dialog.getByRole("button", { name: "Close", exact: true }).evaluate((node) => {
      const rect = node.getBoundingClientRect();
      return (
        node.getRootNode().elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) ===
        node
      );
    }),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/certificates-compact.png"),
  });
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
  assert.equal(await trigger.evaluate((node) => node === node.getRootNode().activeElement), true);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(
    () =>
      (globalThis.__workspaceFixture.certificateInspection = {
        certificates: [],
        connections: [
          {
            id: "none",
            title: "TLS context unavailable",
            fields: [],
            serverChain: [],
            clientChain: [],
            errors: [],
          },
        ],
      }),
  );
  await trigger.click();
  await page.getByRole("tab", { name: "Certificates", exact: true }).click();
  await page
    .locator(".details-dialog[open]")
    .getByText("No public certificate was recorded", { exact: true })
    .waitFor();
  await page
    .locator(".details-dialog[open]")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  // Late replies from a closed dialog must not replace the reopened selection.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferCertificates = true;
  });
  await trigger.click();
  await page.getByRole("tab", { name: "Certificates", exact: true }).click();
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.deferCertificates = false;
    fixture.releaseCertificates();
  });
  await trigger.click();
  await page.getByRole("tab", { name: "Certificates", exact: true }).click();
  await page
    .locator(".details-dialog[open]")
    .getByText("No public certificate was recorded", { exact: true })
    .waitFor();
  await page
    .locator(".details-dialog[open]")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.certificateInspection;
  });
}
