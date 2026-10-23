import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkAuth(page, root) {
  await page.goto("https://workspace.test/");
  const response = page.locator('traffic-workspace message-inspector[side="response"]');
  await page.locator('tr[data-session-id="first"]').click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .traffic.querySelector('message-inspector[side="response"]')?.authCount === 0,
  );
  assert.equal(
    await response.locator(".message-tabs").getByRole("button", { name: /^Auth/ }).count(),
    0,
    "Empty message exposed an Auth tab",
  );
  assert.equal(
    await page.evaluate(() =>
      globalThis.__workspaceFixture.authRequests.some((request) => request.scanBody),
    ),
    true,
    "Automatic discovery skipped the selected body",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.authInspection = {
      findings: [
        {
          id: "body-only",
          format: "Body-only token",
          role: "Response",
          recognition: "recognized",
          status: "decoded",
          verification: "Not checked",
          source: { kind: "body", label: "JSON response", transforms: [] },
          fields: [],
          children: [],
          diagnostics: [],
        },
      ],
      coverage: [],
      diagnostics: [],
    };
  });
  await page.locator('tr[data-session-id="second"]').click();
  await response
    .locator(".message-tabs")
    .getByRole("button", { name: "Auth (1)", exact: true })
    .waitFor();
  assert.equal(
    await response.evaluate((element) => element.mode),
    "body",
    "Discovery selected Auth automatically",
  );
  await response
    .locator(".message-tabs")
    .getByRole("button", { name: "Auth (1)", exact: true })
    .click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.authInspection = null;
  });
  await page.locator('tr[data-session-id="first"]').click();
  await page.waitForFunction(() => {
    const inspector = document
      .querySelector("app-shell")
      .traffic.querySelector('message-inspector[side="response"]');
    return inspector?.authCount === 0 && inspector.mode === "body";
  });
  assert.equal(
    await response.locator(".message-tabs").getByRole("button", { name: /^Auth/ }).count(),
    0,
    "Empty message retained the prior Auth view",
  );
  await page.evaluate(() => {
    const item = (format, recognition = "recognized") => ({
      findings: [
        {
          id: "body-format",
          format,
          role: "Whole body",
          recognition,
          status: "decoded",
          verification: "Not checked",
          source: { kind: "body", label: "Retained body", transforms: [] },
          fields: [],
          children: [],
          diagnostics: [],
        },
      ],
      coverage: [{ source: "body", status: "complete", detail: "Complete body." }],
      diagnostics: [],
    });
    globalThis.__workspaceFixture.authBodyInspections = {
      jose: item("Explicit auth interpretation", "candidate"),
      saml: item("SAML XML"),
      ntlm: item("NTLM"),
      negotiate: item("SPNEGO"),
    };
  });
  const bodyViewer = response.getByLabel("Body viewer", { exact: true }),
    bodyAuth = response.locator("auth-inspector");
  await bodyViewer.selectOption("auth:jose");
  await bodyAuth
    .getByText("No JWT / JWS / JWE recognized in this body.", { exact: true })
    .waitFor();
  assert.equal(
    await response
      .locator(".message-tabs")
      .getByRole("button", { name: /^Auth/ })
      .evaluate((node) => node === node.getRootNode().activeElement),
    true,
    "Opening a body format lost keyboard focus",
  );
  assert.equal(
    await bodyAuth.locator(".auth-finding").count(),
    0,
    "Unmatched body produced an empty generic finding",
  );
  assert.equal(await bodyAuth.getByText("View body as auth…", { exact: true }).count(), 0);
  await bodyAuth.getByRole("button", { name: "Back to body", exact: true }).click();
  assert.equal(
    await bodyViewer.evaluate((node) => node === node.getRootNode().activeElement),
    true,
    "Returning from a body format lost keyboard focus",
  );
  await page.waitForFunction(
    () =>
      document
        .querySelector("app-shell")
        .traffic.querySelector('message-inspector[side="response"]')?.authCount === 0,
  );
  for (const [format, label] of [
    ["saml", "SAML XML"],
    ["ntlm", "NTLM"],
    ["negotiate", "Negotiate"],
  ]) {
    await bodyViewer.selectOption("auth:" + format);
    await bodyAuth
      .getByText("Body inspected as " + label + ". Review the decoded fields and status below.", {
        exact: true,
      })
      .waitFor();
    assert.equal(
      await page.evaluate(
        (format) =>
          globalThis.__workspaceFixture.authRequests.some(
            (request) => request.interpretation === format && request.scanBody,
          ),
        format,
      ),
      true,
    );
    if (format === "saml")
      await page.screenshot({
        path: resolve(root, "../../../target/ui-check/auth-body-format.png"),
      });
    await bodyAuth.getByRole("button", { name: "Back to body", exact: true }).click();
  }
  await bodyViewer.selectOption("auth:ntlm");
  await bodyAuth.getByText("Body as NTLM", { exact: true }).waitFor();
  await page.locator('tr[data-session-id="second"]').click();
  await page.waitForFunction(() => {
    const inspector = document
      .querySelector("app-shell")
      .traffic.querySelector('message-inspector[side="response"]');
    return (
      inspector.authInterpretation === "auto" &&
      inspector.authCount === 0 &&
      inspector.mode === "body"
    );
  });
  await response.evaluate((element) => {
    element.detail = {
      ...element.detail,
      storedBodies: element.detail.storedBodies.map((body) => ({
        ...body,
        availability: "disabled",
      })),
    };
  });
  assert.equal(
    await bodyViewer.locator('option[value="auth:jose"]').isDisabled(),
    true,
    "Missing body enabled an interpretation action",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.failAuth = true;
  });
  await response.evaluate((element) => {
    element.detail = {
      ...element.detail,
      authRevisions: { ...element.detail.authRevisions, [element.boundary]: "failed-discovery" },
    };
  });
  const unavailable = response
    .locator(".message-tabs")
    .getByRole("button", { name: "Auth unavailable", exact: true });
  await unavailable.waitFor();
  await unavailable.click();
  await bodyAuth
    .getByText(
      "Auth evidence could not be inspected or changed while loading. Retry the current selection.",
      { exact: true },
    )
    .waitFor();
  await bodyAuth.getByRole("button", { name: "Retry inspection", exact: true }).waitFor();
  await page.goto("https://workspace.test/");
  const source = {
    kind: "header",
    label: "Authorization · field 521",
    occurrence: 520,
    offset: 0,
    length: 100,
    transforms: ["Base64", "SPNEGO mechanism OCTET STRING"],
  };
  const field = (id, name, value, sensitive = false, note = "") => ({
    id,
    name,
    value,
    available: true,
    sensitive,
    note,
  });
  const finding = (id, format, fields, children = [], status = "decoded") => ({
    id,
    format,
    role: "Synthetic retained message",
    recognition: "recognized",
    status,
    verification: "Not checked",
    source,
    fields,
    children,
    diagnostics: [],
  });
  const inspection = {
    findings: [
      finding(
        "f0",
        "Bearer",
        [field("v0", "Original credential", "••••••••", true)],
        [
          finding("f0.0", "JWT / JWS", [
            field("v12", "JOSE.alg", "none"),
            field("v13", "Signature length", "0"),
            field("v1", "iss", "https://issuer.example.invalid"),
            field("v2", "aud[0]", "workbench"),
            field(
              "v3",
              "exp",
              "1700000100",
              false,
              "Expiry: 2023-11-14T22:15:00Z. Capture time unavailable; no relative-time conclusion.",
            ),
            field("v4", "Decoded JSON payload", "••••••••", true),
          ]),
        ],
      ),
      finding("f1", "SAML 2.0 XML", [
        field("v5", "NameID", "••••••••", true),
        field("v6", "Decoded XML", "••••••••", true),
      ]),
      finding("f2", "NTLM", [
        field("v7", "Username", "synthetic-user"),
        field("v8", "NT response", "••••••••", true),
      ]),
      finding(
        "f3",
        "Kerberos V5",
        [
          field("v9", "Ticket realm", "EXAMPLE.TEST"),
          field("v10", "Service principal[0]", "HTTP"),
          field("v11", "Ticket · encryption type", "18"),
        ],
        [],
        "encrypted",
      ),
    ],
    coverage: [
      {
        source: "headers",
        status: "complete",
        detail: "All retained header fields, including fields beyond the display page.",
      },
      { source: "body", status: "unsupported", detail: "Multipart parts are not scanned." },
    ],
    diagnostics: [],
  };
  await page.evaluate((inspection) => {
    const state = globalThis.__workspaceFixture;
    state.authInspection = inspection;
    state.authValues = {
      v0: "private-bearer-value",
      v4: '{"nonce":"private-claim"}',
      v5: "private-name-id",
      v6: "<Assertion>private-xml</Assertion>",
      v8: "private-response",
    };
  }, inspection);
  await page.locator('tr[data-session-id="first"]').click();
  const inspector = page.locator('traffic-workspace message-inspector[side="response"]');
  const auth = inspector.locator("auth-inspector");
  const tab = inspector.locator(".message-tabs").getByRole("button", { name: /^Auth/ });
  await tab.focus();
  await page.keyboard.press("Enter");
  await auth.getByText("JWT / JWS", { exact: true }).waitFor();
  await auth.getByText("Kerberos V5", { exact: true }).waitFor();
  assert.match(await auth.locator(".auth-overview").textContent(), /1 of 2.*limited coverage/);
  const about = auth.locator(".auth-options");
  assert.equal(
    await about.evaluate((element) => element.open),
    false,
    "Shared inspection explanation started expanded",
  );
  await about.getByText("About this inspection", { exact: true }).focus();
  await page.keyboard.press("Enter");
  assert.equal(
    await about.getByText(/^Captured values are unverified\./).isVisible(),
    true,
    "Shared verification limits are unavailable by keyboard",
  );
  assert.equal(
    await auth.getByText(/^Captured values are unverified\./).count(),
    1,
    "Shared verification caveat repeats",
  );
  await page.keyboard.press("Enter");
  assert.equal(await about.evaluate((element) => element.open), false);
  assert.equal(
    await auth.locator(".auth-coverage").evaluate((element) => element.open),
    false,
    "Routine coverage started expanded",
  );
  assert.equal(
    await auth.getByRole("button", { name: "Retry inspection", exact: true }).count(),
    0,
    "Successful inspection still promotes Retry",
  );
  const beforeRefresh = await page.evaluate(
    () => globalThis.__workspaceFixture.authRequests.length,
  );
  await inspector.evaluate((element) => {
    element.detail = structuredClone(element.detail);
  });
  await page.waitForTimeout(150);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.authRequests.length),
    beforeRefresh,
    "Unrelated detail refresh repeated auth parsing",
  );
  assert.equal(
    await auth.locator("img,script,a").count(),
    0,
    "Captured markup or URLs became executable UI",
  );
  const secrets = [
    "private-bearer-value",
    "private-claim",
    "private-name-id",
    "private-xml",
    "private-response",
  ];
  for (const secret of secrets)
    assert.equal(
      await auth.evaluate((element, secret) => element.outerHTML.includes(secret), secret),
      false,
      "Masked material leaked through DOM or attributes",
    );
  const bearer = auth.locator(".auth-scroll > .auth-finding").first();
  const bearerTechnical = bearer.locator(":scope > .auth-technical");
  assert.equal(
    await bearer.locator(":scope > .auth-summary").count(),
    0,
    "Envelope repeated the decoded child summary",
  );
  assert.equal(
    await auth.getByText("Meaning and limits", { exact: true }).count(),
    0,
    "Field explanations added repetitive disclosures",
  );
  const issuer = auth
    .locator(".auth-field")
    .filter({ has: page.locator("dt", { hasText: /^iss$/ }) });
  assert.equal(
    await issuer.locator(".auth-meaning").count(),
    0,
    "Ordinary claims repeat generic field notes",
  );
  const expiry = auth
    .locator(".auth-field")
    .filter({ has: page.locator("dt", { hasText: /^exp$/ }) });
  assert.equal(
    await expiry.locator(".auth-meaning").isVisible(),
    true,
    "Useful field interpretation requires another disclosure",
  );
  assert.equal(
    await bearer.locator(":scope > .auth-fields dt").first().textContent(),
    "iss",
    "JOSE metadata displaced the readable claims",
  );
  assert.equal(
    await auth.locator("details.auth-provenance").count(),
    0,
    "Provenance adds a nested technical disclosure",
  );
  assert.equal(
    await bearer.locator(".auth-finding").count(),
    0,
    "Bearer/JWT retained a redundant envelope disclosure",
  );
  assert.equal(
    await bearer.getByRole("button", { name: "Show original source", exact: true }).count(),
    1,
    "Same source repeats its action",
  );
  assert.equal(await bearer.locator(".auth-layer").count(), 2, "Decoder chain lost its provenance");
  assert.equal(
    await bearer.evaluate(
      (element) =>
        !!(
          element
            .querySelector(":scope > .auth-fields")
            .compareDocumentPosition(element.querySelector(":scope > .auth-technical")) &
          Node.DOCUMENT_POSITION_FOLLOWING
        ),
    ),
    true,
    "Raw evidence appears before decoded fields",
  );
  await bearerTechnical.locator("summary").first().click();
  const reveal = auth.getByRole("button", {
    name: "Reveal or hide Original credential",
    exact: true,
  });
  assert.equal(await auth.getByRole("button", { name: /Copy actual value/ }).count(), 0);
  assert.equal(await reveal.getAttribute("aria-pressed"), "false");
  const credential = auth
    .locator(".auth-field")
    .filter({ has: page.locator("dt", { hasText: "Original credential" }) });
  await credential.scrollIntoViewIfNeeded();
  const valueBox = await credential.locator("pre").boundingBox(),
    eyeBox = await reveal.boundingBox();
  assert.ok(
    eyeBox.x >= valueBox.x + valueBox.width - 1,
    "Reveal control did not stay beside the value",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auth-eye-inline.png") });
  await reveal.focus();
  await page.keyboard.press("Enter");
  await auth.getByText("private-bearer-value", { exact: true }).waitFor();
  assert.equal(await reveal.getAttribute("aria-pressed"), "true");
  await reveal.click();
  assert.equal(await auth.getByText("private-bearer-value", { exact: true }).count(), 0);
  await reveal.click();
  await auth.getByText("private-bearer-value", { exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.emitNative("visualizers-suspended"));
  await auth
    .getByText("Auth inspection paused while the window was hidden. Retry inspection.", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    await auth.getByText("private-bearer-value", { exact: true }).count(),
    0,
    "Native hide retained a reveal",
  );
  await auth.getByRole("button", { name: "Retry inspection", exact: true }).click();
  await auth.getByText("JWT / JWS", { exact: true }).waitFor();
  await bearerTechnical.locator("summary").first().click();
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (value) => {
          globalThis.__authClipboard = value;
        },
      },
    });
  });
  const copyMenu = page.getByRole("menu", { name: "Copy inspection value", exact: true });
  await bearer.locator(":scope > summary").click({ button: "right" });
  await copyMenu.getByRole("menuitem", { name: "Copy name", exact: true }).click();
  assert.match(
    await page.evaluate(() => globalThis.__authClipboard),
    /^JWT \/ JWS.*Bearer/,
    "Artifact name copy retained only its hidden envelope name",
  );
  await credential.focus();
  await page.keyboard.press("Shift+F10");
  await copyMenu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await page.waitForFunction(() => globalThis.__authClipboard === "private-bearer-value");
  assert.equal(
    await auth.getByText("private-bearer-value", { exact: true }).count(),
    0,
    "Copy revealed a masked value in the DOM",
  );
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async () => {
          throw new Error("Synthetic clipboard failure");
        },
      },
    });
  });
  await credential.click({ button: "right" });
  await copyMenu.getByRole("menuitem", { name: "Copy value", exact: true }).click();
  await copyMenu
    .getByText("Copy failed. The clipboard or retained value is unavailable.", { exact: true })
    .waitFor();
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferAuthValue = true;
  });
  await reveal.click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishAuthValue === "function",
  );
  await inspector
    .locator(".message-tabs")
    .getByRole("button", { name: "Headers", exact: true })
    .click();
  await page.evaluate(() => globalThis.__workspaceFixture.finishAuthValue());
  await tab.click();
  await auth.getByText("JWT / JWS", { exact: true }).waitFor();
  assert.equal(
    await auth.getByText("private-bearer-value", { exact: true }).count(),
    0,
    "Late reveal restored a hidden view",
  );
  await auth.getByRole("button", { name: "Related traffic", exact: true }).click();
  await auth.getByRole("region", { name: "Related auth traffic" }).waitFor();
  await auth
    .getByText("No related traffic found in this searched scope.", { exact: true })
    .waitFor();
  await auth.getByRole("button", { name: "Back to evidence", exact: true }).click();
  assert.equal(
    await auth
      .getByRole("button", { name: "Related traffic", exact: true })
      .evaluate((element) => element === element.getRootNode().activeElement),
    true,
    "Closing related results left focus on a removed button",
  );
  await auth.locator(".auth-scroll").evaluate((element) => (element.scrollTop = 100));
  const sourceScroll = await auth.locator(".auth-scroll").evaluate((element) => element.scrollTop);
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.authRelated = {
      entries: [
        {
          sessionId: "second",
          boundary: "client-response",
          label: "Related second request",
          reason: "Same capture and authority",
        },
      ],
      searched: 4,
      limited: true,
      detail: "Retained headers only; body contents were not searched.",
    };
    const stream = {
      state: { protocol: "web-socket", status: "open", atMillis: 1000, reason: null },
      imported: false,
      revision: 1,
      observed: 0,
      evicted: 0,
      sequenceLoss: 0,
      retained: 0,
      clientBytes: 0,
      serverBytes: 0,
      openedAt: 1000,
      lastActivity: 1000,
    };
    state.streams = { first: stream, second: stream };
  });
  await auth.getByRole("button", { name: "Related traffic", exact: true }).click();
  await auth
    .getByText("Retained headers only; body contents were not searched.", { exact: true })
    .waitFor();
  await auth
    .getByRole("button", { name: "Related second request · client-response", exact: true })
    .click();
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.selectedSessionId === "second",
  );
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.selectedDetail?.id === "second",
  );
  assert.equal(
    await page.locator("traffic-workspace").evaluate((element) => element.streamPanel),
    false,
    "Related Auth navigation left the stream pane covering its evidence",
  );
  assert.equal(await tab.isVisible(), true, "Related Auth tab was hidden behind stream messages");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "query_traffic_view";
    delete globalThis.__workspaceFixture.finishHeldCommand;
  });
  await page
    .locator("traffic-workspace")
    .getByRole("button", { name: "Back to auth source", exact: true })
    .click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  await page.getByRole("searchbox", { name: "Filter or search traffic", exact: true }).focus();
  await page.evaluate(() => {
    document.querySelector("app-shell").traffic.diagnostic = "Newer return status";
    globalThis.__workspaceFixture.finishHeldCommand();
  });
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.authReturnBusy);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedSessionId),
    "second",
    "Superseded return changed the selected request",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").diagnosticText),
    "Newer return status",
    "Superseded Auth return replaced the newer status",
  );
  await page
    .locator("traffic-workspace")
    .getByRole("button", { name: "Back to auth source", exact: true })
    .click();
  await page.waitForFunction(() => {
    const traffic = document.querySelector("app-shell").traffic;
    return (
      traffic.selectedSessionId === "first" &&
      !traffic.responseMessage.querySelector("auth-inspector").authBusy &&
      traffic.responseMessage.querySelector("auth-inspector").authFindings.length > 0
    );
  });
  assert.equal(
    await auth.locator(".auth-scroll").evaluate((element) => element.scrollTop),
    sourceScroll,
    "Related navigation lost the original inspection position",
  );
  assert.equal(
    await tab.evaluate((element) => element === element.getRootNode().activeElement),
    true,
    "Related return lost useful keyboard focus",
  );
  assert.equal(
    await page
      .locator("traffic-workspace")
      .getByRole("button", { name: "Back to auth source", exact: true })
      .count(),
    0,
  );
  assert.equal(
    await page.locator("traffic-workspace").evaluate((element) => element.streamPanel),
    false,
    "Returning to an Auth source reopened stream messages",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.streams = {}));
  await auth.getByRole("button", { name: "Related traffic", exact: true }).click();
  await auth
    .getByRole("button", { name: "Related second request · client-response", exact: true })
    .waitFor();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "query_traffic_view";
    delete globalThis.__workspaceFixture.finishHeldCommand;
  });
  await auth
    .getByRole("button", { name: "Related second request · client-response", exact: true })
    .click();
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
  );
  const search = page.getByRole("searchbox", { name: "Filter or search traffic", exact: true });
  await search.focus();
  const navigationStatus = await page.evaluate(
    () => (document.querySelector("app-shell").traffic.diagnostic = "Newer inspection status"),
  );
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForTimeout(80);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.selectedSessionId),
    "first",
    "Late related navigation replaced the source",
  );
  assert.equal(
    await search.evaluate((element) => element === element.getRootNode().activeElement),
    true,
    "Late related navigation stole focus",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").diagnosticText),
    navigationStatus,
    "Superseded navigation reported retained evidence as unavailable",
  );
  await page.evaluate(() => (document.querySelector("app-shell").diagnosticText = ""));
  await auth.getByRole("button", { name: "Back to evidence", exact: true }).click();

  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.authSource = {
      sessionId: "first",
      boundary: "client-response",
      source: {
        kind: "header",
        label: "WWW-Authenticate · field 1",
        occurrence: 0,
        offset: 0,
        length: 20,
        transforms: [],
      },
    };
  });
  await auth.getByRole("button", { name: "Show original source", exact: true }).first().click();
  await inspector.getByText(/Original retained source may contain secrets/).waitFor();
  await tab.click();
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    // Narrow Traffic retains its existing request/response switch; select Response when present.
    const response = page.getByRole("button", { name: "Response", exact: true });
    if ((await response.count()) && (await response.first().isVisible()))
      await response.first().click();
    await auth.getByText("JWT / JWS", { exact: true }).waitFor();
    assert.equal(
      await auth.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      true,
      "Auth viewport overflows horizontally",
    );
    await page.screenshot({ path: resolve(root, `../../../target/ui-check/auth-${width}.png`) });
  }
  // Expansion preserves persisted splits, the selected evidence, and return focus.
  const traffic = page.locator("traffic-workspace"),
    expand = traffic.getByRole("button", { name: "Expand inspector", exact: true });
  const previous = await traffic.evaluate((element) => ({
    preferences: structuredClone(element.preferences),
    id: element.selectedSessionId,
    mode: element.responseMessage.mode,
  }));
  for (const zoom of ["", "1.25"]) {
    await page.evaluate((zoom) => (document.documentElement.style.zoom = zoom), zoom);
    await auth.locator(".auth-scroll").evaluate((element) => (element.scrollTop = 0));
    await expand.click();
    await traffic.getByRole("button", { name: "Restore", exact: true }).waitFor();
    const geometry = await issuer.evaluate((element) => {
      const box = element.getBoundingClientRect(),
        scroll = element.closest(".auth-scroll").getBoundingClientRect();
      return {
        top: box.top,
        bottom: box.bottom,
        scrollTop: scroll.top,
        scrollBottom: scroll.bottom,
      };
    });
    assert.ok(
      geometry.top >= geometry.scrollTop - 1 && geometry.bottom <= geometry.scrollBottom + 1,
      "Expanded compact inspector did not expose the first readable field",
    );
    assert.equal(
      await traffic.getByRole("grid", { name: "Traffic entries", exact: true }).isVisible(),
      false,
    );
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/auth-expanded-${zoom ? "zoom" : "800"}.png`),
    });
    await auth.locator(".auth-scroll").evaluate((element) => (element.scrollTop = 100));
    await page.keyboard.press("Control+Shift+Enter");
    await expand.waitFor();
    assert.equal(
      await traffic.evaluate(
        (element) => element.trafficTable === element.getRootNode().activeElement,
      ),
      true,
      "Restore did not return keyboard focus to Traffic",
    );
    assert.deepEqual(
      await traffic.evaluate((element) => ({
        preferences: element.preferences,
        id: element.selectedSessionId,
        mode: element.responseMessage.mode,
      })),
      previous,
      "Expansion changed persisted layout or evidence",
    );
    assert.equal(
      await auth.locator(".auth-scroll").evaluate((element) => element.scrollTop),
      0,
      "Restore lost a reading position at the top",
    );
  }
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.setViewportSize({ width: 1440, height: 900 });
  await auth.locator(".auth-scroll").evaluate((element) => (element.scrollTop = 70));
  const readingPosition = await auth
    .locator(".auth-scroll")
    .evaluate((element) => element.scrollTop);
  await expand.click();
  const expandedDivider = traffic.getByRole("separator", {
    name: "Resize request and response inspectors",
    exact: true,
  });
  await expandedDivider.focus();
  await page.keyboard.press("Shift+ArrowRight");
  assert.equal(
    await traffic.evaluate((element) => element.preferences.requestSplit),
    previous.preferences.requestSplit,
    "Expanded resize overwrote saved divider",
  );
  await traffic.getByRole("button", { name: "Restore", exact: true }).click();
  assert.equal(
    await auth.locator(".auth-scroll").evaluate((element) => element.scrollTop),
    readingPosition,
    "Restore lost the prior reading position",
  );
  await page.setViewportSize({ width: 800, height: 600 });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auth-forced-colors.png") });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auth-dark.png") });
  await page.emulateMedia({ colorScheme: "light" });
  await page.evaluate(() => (document.documentElement.style.zoom = "1.25"));
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auth-zoom.png") });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  const divider = page.getByRole("separator", {
    name: "Resize request and response inspectors",
    exact: true,
  });
  await divider.focus();
  for (let step = 0; step < 3; step++) await page.keyboard.press("Shift+ArrowRight");
  await expiry.scrollIntoViewIfNeeded();
  assert.equal(
    await auth.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
    true,
    "Resized Auth pane overflows",
  );
  assert.equal(
    await expiry.locator(".auth-meaning").isVisible(),
    true,
    "Resizing lost readable interpretation",
  );
  await page.screenshot({ path: resolve(root, "../../../target/ui-check/auth-resized.png") });
  await divider.focus();
  await page.keyboard.press("Home");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferAuth = true;
  });
  await auth.locator(".auth-options > summary").click();
  await auth.getByRole("button", { name: "Refresh inspection", exact: true }).click();
  await page.waitForFunction(() => typeof globalThis.__workspaceFixture.finishAuth === "function");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.authInspection = {
      findings: [
        {
          id: "f0",
          format: "Newest selection",
          role: "Selected second request",
          status: "opaque",
          verification: "Not checked",
          source: { kind: "header", label: "Current source", occurrence: 0, transforms: [] },
          fields: [],
          children: [],
          diagnostics: [],
        },
      ],
      coverage: [],
      diagnostics: [],
    };
  });
  await page.locator('tr[data-session-id="second"]').click();
  await auth.getByText("Newest selection", { exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.finishAuth());
  await page.waitForTimeout(100);
  assert.equal(
    await auth.getByText("JWT / JWS", { exact: true }).count(),
    0,
    "Late analysis overwrote a newer selection",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.authInspection = null;
  });
  await inspector
    .locator(".message-tabs")
    .getByRole("button", { name: "Headers", exact: true })
    .click();
  // Reset the page so clipboard test doubles, delayed replies and fixture data cannot affect the next feature check.
  await page.goto("https://workspace.test/");
}
