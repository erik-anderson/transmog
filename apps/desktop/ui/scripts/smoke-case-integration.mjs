import assert from "node:assert/strict";
import { glob, readFile, writeFile, copyFile } from "node:fs/promises";
import { resolve } from "node:path";
import { spawn } from "node:child_process";
import { nativeFixture } from "./native-fixture.mjs";

// Every file, process and window is owned by the isolated native harness.
export async function smokeCaseIntegration({ page, output, chromium, processId, closeHelper }) {
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  const bare = resolve(output, "unsupported.tmcase.zip");
  await writeFile(
    bare,
    await readFile(
      new URL("../../../../verification/fixtures/portable/packages/simple.tmcase", import.meta.url),
    ),
  );
  assert.equal(await invoke("saved_file_is_case", { path: bare }), false);
  const rejected = await page.evaluate(async (path) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("open_case", { path, password: null });
      return null;
    } catch (error) {
      return { category: error.category, message: error.message };
    }
  }, bare);
  assert.equal(rejected?.category, "unsupported");
  assert.match(rejected.message, /Choose a Transmog case or rule collection in TMCap format/);
  const bytes = (s) => Array.from(Buffer.from(s));
  const records = [];
  for (const [id, path, type, body] of [
    [
      101,
      "/case-page",
      "text/html",
      `<h1 id="case-heading">Portable fixture</h1><link rel="stylesheet" href="/case.css"><script>globalThis.caseScript=true;fetch('/missing').catch(()=>globalThis.caseMiss=true)</script>`,
    ],
    [102, "/case.css", "text/css", "h1 {color: green;}"],
    [103, "/case.css", "text/css", "h1 {color: red;}"],
  ]) {
    records.push(
      ...[
        {
          kind: "request-head",
          payload: {
            boundary: "client-request",
            method: "GET",
            target: "https://case.invalid" + path,
            headers: [],
          },
        },
        {
          kind: "response-head",
          payload: {
            boundary: "client-response",
            status: 200,
            headers: [
              { name: bytes("content-type"), value: bytes(type) },
              { name: bytes("cache-control"), value: bytes("max-age=3600") },
            ],
          },
        },
        {
          kind: "body-segment",
          payload: {
            boundary: "client-response",
            byte_count: Buffer.byteLength(body),
            bytes: bytes(body),
            truncated: false,
          },
        },
        { kind: "completed", payload: null },
      ].map((row) => ({ ...row, exchange_id: id })),
    );
  }
  for (let i = 0; i < 96; i++)
    records.push(
      {
        kind: "request-head",
        exchange_id: 200 + i,
        payload: {
          boundary: "client-request",
          method: "GET",
          target: `https://case.invalid/assets/item-${i}`,
          headers: [],
        },
      },
      {
        kind: "response-head",
        exchange_id: 200 + i,
        payload: { boundary: "client-response", status: 204, headers: [] },
      },
      { kind: "completed", exchange_id: 200 + i, payload: null },
    );
  const input = resolve(output, "case-integration.tmcap");
  await writeFile(input, nativeFixture(records, 101));
  await page.evaluate((path) => {
    void document.querySelector("app-shell").traffic.importTrace(path);
  }, input);
  await page.waitForFunction(
    () => {
      const t = document.querySelector("app-shell").traffic;
      return (
        t.sessions.some((r) => r.path === "/case-page") ||
        t.importStatus.startsWith("Import failed:")
      );
    },
    null,
    { timeout: 30000 },
  );
  assert.equal(
    await page.evaluate(() =>
      document.querySelector("app-shell").traffic.importStatus.startsWith("Import failed:"),
    ),
    false,
    await page.evaluate(() => document.querySelector("app-shell").traffic.importStatus),
  );
  const rows = await page.evaluate(() =>
    document
      .querySelector("app-shell")
      .traffic.sessions.filter((r) => r.url.startsWith("https://case.invalid/")),
  );
  const css = rows.filter((r) => r.path === "/case.css");
  assert.equal(css.length, 2);
  const asset = await invoke("create_response_asset", {
    input: {
      id: "native-case-css",
      revision: 1,
      status: 200,
      headers: [],
      body: bytes("h1 {color: blue;}"),
      mediaType: "text/css",
    },
  });
  const rule = {
    id: "native-case-rule",
    displayName: "Frozen case CSS",
    revision: 1,
    priority: 0,
    enabled: true,
    matcher: {
      method: "GET",
      url: { kind: "exact", value: "https://case.invalid/case.css" },
      scheme: null,
      host: null,
      port: null,
      pathPrefix: null,
      query: null,
      requestHeaders: [],
      responseHeaders: [],
      responseStatus: null,
      responseStatusClass: null,
    },
    request: {
      headers: [],
      replaceBody: null,
      discardBody: false,
      abortReason: null,
      responseAsset: asset.id + "@" + asset.revision,
      allowNonIdempotentBodyReplacement: false,
    },
    response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
  };
  const sibling = {
    ...rule,
    id: "native-case-sibling",
    priority: 1,
    matcher: {
      ...rule.matcher,
      url: { kind: "exact", value: "https://case.invalid/uncaptured-sibling" },
    },
  };
  let status = await invoke("automation_status");
  status = await invoke("change_response_set", {
    generation: status.generation,
    change: {
      action: "create",
      id: "native-case-experiment",
      name: "Native case responses",
      notes: "Portable current response snapshot",
      rules: [rule, sibling],
    },
  });
  for (const [id, label, path] of [
    ["offscreen-rule", "Matches request beyond visible rows", "/assets/item-95"],
    ["absent-rule", "Missing resource override", "/not-captured"],
  ]) {
    const draft = {
      ...rule,
      id,
      displayName: label,
      enabled: false,
      matcher: { ...rule.matcher, url: { kind: "exact", value: "https://case.invalid" + path } },
      request: {
        ...rule.request,
        responseAsset: null,
        headers: [{ operation: "remove", field: "X-Picker-Only" }],
      },
    };
    status = await invoke("change_header_rule", {
      generation: status.generation,
      change: { action: "save", rule: draft, expectedRevision: null, placement: "after" },
    });
  }
  const owner = status.responseSets.find((set) => set.id === "native-case-experiment"),
    selection = {
      experimentId: owner.id,
      incarnation: owner.incarnation,
      variantId: "",
      revision: owner.revision,
    };
  const original = JSON.stringify(status);
  const traffic = page.locator("traffic-workspace"),
    dialog = page.locator("portable-case dialog");
  await traffic.getByRole("button", { name: "Files", exact: true }).click();
  await traffic.getByRole("button", { name: "Save case…", exact: true }).click();
  await dialog.getByLabel("Traffic to include", { exact: true }).selectOption("filtered");
  await dialog.getByText("99 included requests.", { exact: false }).waitFor();
  await dialog.getByText("Rules and response sets · 0 selected", { exact: true }).click();
  await dialog.getByLabel("Matches request beyond visible rows", { exact: false }).check();
  await dialog.getByLabel("Native case responses / Current responses", { exact: false }).check();
  assert.equal(await dialog.getByLabel("Missing resource override", { exact: false }).count(), 0);
  assert.equal(
    await dialog.getByLabel("Frozen case CSS", { exact: false }).count(),
    0,
    "Owned rules must not need a separate selection",
  );
  await dialog.getByLabel("Show all", { exact: true }).check();
  await dialog.getByLabel("Missing resource override", { exact: false }).check();
  await dialog.getByLabel("Show all", { exact: true }).uncheck();
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await dialog.locator(".case-scroll").evaluate((el) => (el.scrollTop = 0));
    await page.screenshot({ path: resolve(output, `case-picker-native-${width}.png`) });
  }
  await dialog.getByRole("button", { name: "Review selected content", exact: true }).click();
  await page.waitForFunction(
    () => document.querySelector("app-shell").traffic.portableCase.caseStage === "export",
  );
  const selected = await dialog.locator("pre").first().textContent();
  const selectedManifest = JSON.parse(selected);
  assert.equal(
    selectedManifest.experiments[0].variants.find((v) => v.id === "current").revisions[0].rules
      .length,
    2,
  );
  assert.equal(selectedManifest.rules.length, 2);
  assert.equal(JSON.stringify(await invoke("automation_status")), original);
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  const comparison = await invoke("compare_evidence", {
    input: {
      operationId: "native-case-compare",
      left: { kind: "exchange", id: css[0].id },
      right: { kind: "exchange", id: css[1].id },
      options: { excludedHeaders: ["date"], normalizeNewlines: true },
    },
  });
  const captures = await invoke("compare_captures", {
    operationId: "native-case-scopes",
    left: { kind: "selected", ids: [css[0].id] },
    right: { kind: "selected", ids: [css[1].id] },
    options: { excludedQueryNames: ["nonce"], requestHeaders: [], requestBodyDigest: false },
  });
  await invoke("capture_comparison_page", {
    id: captures.id,
    offset: 0,
    options: null,
    left: css[0].id,
    right: css[1].id,
    action: "pair",
  });
  assert.ok(rows[0].traceId);
  const review = await invoke("prepare_case_export", {
    input: {
      title: "Native frozen case",
      scope: { kind: "source", traceId: rows[0].traceId },
      ruleIds: [],
      selections: [selection],
      notes: "Portable frozen responses and saved comparisons",
      includeContext: true,
      comparisonIds: [comparison.id, captures.id],
    },
  });
  assert.equal(review.manifest.comparisons.length, 2);
  assert.equal(review.traffic.entries, 99);
  assert.equal(review.manifest.sources[0].requestCount, "99");
  const packagePath = await invoke("save_case_export", { password: null });
  assert.equal(await invoke("saved_file_is_case", { path: input }), false);
  assert.equal(await invoke("saved_file_is_case", { path: packagePath }), true);
  const renamed = resolve(output, "received.tmcap");
  await copyFile(packagePath, renamed);
  await page.evaluate(
    async (path) => document.querySelector("app-shell").traffic.importPaths([path]),
    renamed,
  );
  await dialog.getByRole("heading", { name: "Native frozen case", exact: true }).waitFor();
  assert.match(await dialog.locator(".case-summary").textContent(), /99 requests/);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await dialog.locator(".case-scroll").evaluate((el) => (el.scrollTop = 0));
    const action = await dialog
      .getByRole("button", { name: "Open in Traffic", exact: true })
      .boundingBox();
    assert.ok(action.y >= 0 && action.y + action.height <= height);
    await page.screenshot({ path: resolve(output, `case-inventory-native-${width}.png`) });
  }
  await dialog.getByRole("button", { name: "Preview…", exact: true }).click();
  await dialog
    .getByLabel("Responses", { exact: true })
    .selectOption({ label: "Native case responses / Current responses / revision 1" });
  assert.equal(
    await dialog.getByLabel("Enable scripts for this preview", { exact: true }).isChecked(),
    false,
  );
  await dialog.getByRole("button", { name: "Open preview", exact: true }).click();
  await dialog.getByText(/^Case preview opened/).waitFor();
  let previewBrowser;
  const findPreview = async () => {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      for await (const path of glob("**/DevToolsActivePort", { cwd: output })) {
        const port = (await readFile(resolve(output, path), "utf8")).split(/\r?\n/)[0];
        const targets = await fetch(`http://127.0.0.1:${port}/json/list`)
          .then((r) => r.json())
          .catch(() => []);
        if (targets.some((t) => t.url === "https://case.invalid/case-page")) {
          previewBrowser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
          return previewBrowser
            .contexts()
            .flatMap((c) => c.pages())
            .find((p) => p.url() === "https://case.invalid/case-page");
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error("Case preview DevTools target unavailable");
  };
  try {
    const preview = await findPreview();
    await preview.waitForFunction(
      () => getComputedStyle(document.getElementById("case-heading")).color === "rgb(0, 0, 255)",
    );
    assert.equal(await preview.evaluate(() => globalThis.caseScript), undefined);
    await preview.screenshot({ path: resolve(output, "case-frozen-preview-native.png") });
    const label = await page.evaluate(
      () => document.querySelector("app-shell").traffic.previewReportLabel,
    );
    const report = await invoke("captured_page_report", { label });
    assert.match(report.caseOrigin, /native-case-experiment\/current\/1/);
    assert.match(report.runtime, /WebView2/);
    assert.equal(report.scriptsEnabled, false);
    assert.equal(report.rules.length, 2);
    assert.ok(report.requests.some((r) => r.outcome === "auto-response"));
    await dialog.getByRole("button", { name: "More", exact: true }).click();
    await dialog.getByRole("button", { name: "Preview diagnostics…", exact: true }).click();
    const diagnostics = page.getByRole("dialog", {
      name: "Captured preview diagnostics",
      exact: true,
    });
    await diagnostics.getByText(/These rules belong to the frozen case/).waitFor();
    assert.equal(
      await diagnostics
        .getByRole("button", { name: "Show saved rule", exact: true })
        .evaluateAll(
          (buttons) => buttons.length === 2 && buttons.every((button) => button.disabled),
        ),
      true,
    );
    await diagnostics.getByRole("button", { name: "Close", exact: true }).click();
    const count = await page.evaluate(
      () => document.querySelector("app-shell").traffic.totalMatched,
    );
    await dialog.getByRole("button", { name: "Open in Traffic", exact: true }).click();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").traffic.portableCase.caseDialog.open,
    );
    assert.equal(
      await page.evaluate(() => document.querySelector("app-shell").traffic.totalMatched),
      count,
      "Repeated source inspection duplicated Traffic",
    );
    await traffic.getByRole("button", { name: "Files", exact: true }).click();
    await traffic.getByRole("button", { name: "Open case…", exact: true }).click();
    await dialog
      .locator(".case-row")
      .filter({ hasText: review.manifest.comparisons[0].label })
      .getByRole("button", { name: "Recompute comparison", exact: true })
      .click();
    const pair = page.getByRole("dialog", { name: "Compare requests and responses" });
    await pair.waitFor();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").comparison.comparisonBusy,
    );
    const result = await page.evaluate(
      () => document.querySelector("app-shell").comparison.comparisonResult,
    );
    assert.ok(
      result,
      await page.evaluate(() => document.querySelector("app-shell").comparison.comparisonStatus),
    );
    assert.equal(result.different, comparison.different);
    assert.deepEqual(result.options, { excludedHeaders: ["date"], normalizeNewlines: true });
    await pair.getByRole("button", { name: "Close", exact: true }).click();
    await traffic.getByRole("button", { name: "Files", exact: true }).click();
    await traffic.getByRole("button", { name: "Open case…", exact: true }).click();
    await dialog
      .locator(".case-row")
      .filter({ hasText: review.manifest.comparisons[1].label })
      .getByRole("button", { name: "Recompute comparison", exact: true })
      .click();
    const scope = page.getByRole("dialog", { name: "Compare capture scopes" });
    await scope.waitFor();
    await page.waitForFunction(() => !document.querySelector("app-shell").comparison.captureBusy);
    const restored = await page.evaluate(
      () => document.querySelector("app-shell").comparison.captureResult,
    );
    assert.ok(
      restored,
      await page.evaluate(() => document.querySelector("app-shell").comparison.captureStatus),
    );
    assert.deepEqual(restored.options.excludedQueryNames, ["nonce"]);
    assert.ok(restored.rows.some((r) => r.reason === "Explicit user choice"));
    await scope.getByRole("button", { name: "Close", exact: true }).click();
    assert.equal(JSON.stringify(await invoke("automation_status")), original);
    // Native case drag/drop reaches the same overview, and import review uses names.
    await traffic.getByRole("button", { name: "Files", exact: true }).click();
    await traffic.getByRole("button", { name: "Current case…", exact: true }).click();
    const saved = await invoke("save_open_case", { password: null });
    const copy = resolve(output, "received.tmcase.tmcap");
    await copyFile(saved, copy);
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await traffic.locator("trace-drop-target").evaluate((element, path) => {
      const b = element.dropRegion.getBoundingClientRect();
      element.handleNativeDrag({
        type: "drop",
        paths: [path],
        position: { x: (b.left + 10) * devicePixelRatio, y: (b.top + 10) * devicePixelRatio },
      });
    }, copy);
    await dialog.getByRole("heading", { name: "Native frozen case", exact: true }).waitFor();
    await dialog.getByText("Import rules and response sets", { exact: true }).click();
    await dialog.getByLabel("Native case responses · Ready to import", { exact: true }).check();
    await dialog.getByRole("button", { name: "Review rule import", exact: true }).click();
    await dialog.getByRole("heading", { name: "Rules to import", exact: true }).waitFor();
    await dialog.getByText("Frozen case CSS", { exact: true }).first().waitFor();
    assert.equal(JSON.stringify(await invoke("automation_status")), original);
    await writeFile(
      resolve(output, "case-integration.json"),
      JSON.stringify(
        {
          nativeWebView2: true,
          inventoryRequests: 99,
          fullScopeRuleMatching: true,
          selectedUnmatchedRuleRetained: true,
          wholeVersionRules: 2,
          frozenPackageCss: true,
          scriptsOff: true,
          caseOrigin: report.caseOrigin,
          runtime: report.runtime,
          savedExchangeComparison: true,
          savedOptions: true,
          manualPairingRestored: true,
          deduplicatedSourceInspection: true,
          liveAutomationUnchanged: true,
        },
        null,
        2,
      ),
    );
  } finally {
    await previewBrowser?.close();
    const child = spawn(
      "pwsh.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-File",
        closeHelper,
        "-ProbeProcessId",
        processId,
        "-WindowTitle",
        "Modified preview · Scripts off — Transmog",
      ],
      { windowsHide: true, stdio: "inherit" },
    );
    await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) =>
        code === 0 ? resolve() : reject(new Error("Case preview close failed " + code)),
      );
    });
  }
  process.stdout.write(
    "Native case frozen preview, exact options, saved comparison recomputation and manual pairing verified.\n",
  );
}
