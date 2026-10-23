import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkResponseSetPages(page, root) {
  await page.goto("https://workspace.test/?empty");
  await page.waitForFunction(() => document.querySelector("app-shell").workspaceLoaded);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      s = globalThis.__workspaceFixture;
    const action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
    const rule = {
      id: "facebook-post",
      displayName: "Saved Facebook POST",
      enabled: true,
      revision: 1,
      priority: 1,
      matcher: {
        method: "POST",
        url: { kind: "exact", value: "https://facebook.test/api" },
        requestHeaders: [],
        responseHeaders: [],
      },
      request: { ...action, responseAsset: "facebook@1" },
      response: action,
    };
    const set = (id) => ({
      id,
      incarnation: "fixture-" + id,
      name: id,
      notes: "Page association fixture",
      revision: 2,
      modifiedAt: 1800000000000,
      rules: [rule],
      versions: [],
      applied: null,
      undoApply: { generation: 9, applied: null, paused: false },
      paused: false,
      previewPage: null,
      previewPageRevision: 0,
    });
    s.automation = {
      schemaVersion: 2,
      generation: 9,
      rules: [],
      autoresponsesEnabled: false,
      diagnostics: [],
      responseSets: [set("Facebook response"), set("Another set")],
    };
    s.sessions = [];
    s.captureNonce = "capture-A";
    await shell.activateView("automation");
    shell.automation.renderAutomation(structuredClone(s.automation));
    shell.automation.showAutomationSection("experiments");
    await shell.automation.openResponseSet(
      new CustomEvent("set-open", { detail: { id: "Facebook response" } }),
    );
  });
  const manager = page.locator("response-set-workspace"),
    preview = manager.getByRole("button", { name: "Preview captured page…", exact: true });
  const chooser = page.getByRole("dialog", {
      name: "Choose a page for this response set",
      exact: true,
    }),
    warning = page.locator(".captured-page-warning[open]");
  const openPreview = async () => {
    await manager.getByRole("button", { name: "Try changes…", exact: true }).click();
    await preview.click();
  };
  const data = () => page.evaluate(() => structuredClone(globalThis.__workspaceFixture.automation));
  const before = await data();
  await openPreview();
  await chooser.waitFor();
  assert.match(await chooser.textContent(), /No complete captured HTML pages/);
  assert.equal(await manager.isVisible(), true);
  await page.keyboard.press("Escape");
  assert.equal(
    await manager
      .getByRole("button", { name: "Try changes…", exact: true })
      .evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.sessions = [
      {
        ...s.emptyExample,
        id: "post",
        method: "POST",
        url: "https://facebook.test/api",
        contentType: "application/json",
      },
      {
        ...s.emptyExample,
        id: "page-A",
        requestNumber: "41",
        method: "GET",
        url: "https://transmog.net/",
        contentType: "text/html",
        traceId: "first.har",
        startedAt: 1800000000000,
      },
      {
        ...s.emptyExample,
        id: "page-B",
        requestNumber: "42",
        method: "GET",
        url: "https://another.test/" + "long-path/".repeat(25),
        contentType: "text/html",
        traceId: "second.har",
        startedAt: 1800000000001,
      },
      {
        ...s.emptyExample,
        id: "same-url",
        requestNumber: "43",
        method: "GET",
        url: "https://transmog.net/",
        contentType: "text/html",
        traceId: "second.har",
        startedAt: 1800000000002,
      },
    ];
  });
  await openPreview();
  await chooser.waitFor();
  assert.equal(await chooser.getByRole("radio", { checked: true }).count(), 0);
  assert.equal(
    await chooser.getByRole("button", { name: "Use page", exact: true }).isEnabled(),
    false,
  );
  assert.match(
    await chooser.textContent(),
    /Saved responses apply only to page requests that match/,
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({
      path: resolve(root, `../../../target/response-set-page-association/chooser-${width}.png`),
    });
    const box = await chooser.boundingBox();
    assert.ok(box.width <= width && box.height <= height);
  }
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  await page.emulateMedia({ forcedColors: "active" });
  await chooser.getByRole("button", { name: "Close", exact: true }).focus();
  await page.screenshot({
    path: resolve(root, "../../../target/response-set-page-association/chooser-zoom-contrast.png"),
  });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.emulateMedia({ forcedColors: "none" });
  await chooser.getByRole("radio", { name: /Request #41/ }).check();
  await chooser.getByRole("button", { name: "Use page", exact: true }).click();
  await warning.waitFor();
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.previewRulesBusy);
  assert.equal(
    await warning.getByLabel("Captured page", { exact: true }).count(),
    0,
    "Named preview still offered implicit replacement",
  );
  assert.deepEqual(
    await page.evaluate(() => document.querySelector("app-shell").traffic.previewVariantSelection),
    {
      experimentId: "Facebook response",
      incarnation: "fixture-Facebook response",
      variantId: "",
      revision: 2,
    },
  );
  await warning.getByRole("button", { name: "Open preview", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.previewPageBusy);
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.previewArgs.options.pageReference.entryId,
    ),
    "page-A",
  );
  const associated = await data();
  assert.equal(associated.generation, before.generation);
  assert.deepEqual(associated.rules, before.rules);
  assert.deepEqual(associated.responseSets[0].rules, before.responseSets[0].rules);
  assert.deepEqual(associated.responseSets[0].undoApply, before.responseSets[0].undoApply);
  await page.evaluate(() => document.querySelector("app-shell").activateView("automation"));
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").automation.responseSets.pageUrl ===
      "https://transmog.net/",
  );
  await page.evaluate(() => {
    globalThis.__workspaceFixture.sessions.unshift({
      ...globalThis.__workspaceFixture.sessions[2],
      id: "new-traffic",
      url: "https://new.test/",
    });
    document.querySelector("app-shell").automation.refreshUsageSoon();
  });
  await openPreview();
  await warning.waitFor();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.previewPageId),
    "page-A",
  );
  await warning.getByRole("button", { name: "Cancel", exact: true }).click();
  await manager.getByRole("button", { name: "Change…", exact: true }).click();
  await chooser.waitFor();
  assert.equal(await chooser.getByRole("radio", { checked: true }).count(), 0);
  await chooser.getByRole("radio", { name: /Request #43/ }).check();
  await page.keyboard.press("Escape");
  assert.deepEqual(await data(), associated);
  await manager.getByRole("button", { name: "← Response sets", exact: true }).click();
  await manager.getByRole("button", { name: "Another set", exact: true }).click();
  await openPreview();
  await chooser.waitFor();
  assert.equal(await chooser.locator(".remembered-page").count(), 0);
  await page.keyboard.press("Escape");
  await openPreview();
  await chooser.waitFor();
  await chooser.getByRole("radio", { name: /^https:\/\/another.test\// }).check();
  await chooser.getByRole("button", { name: "Use page", exact: true }).click();
  await warning.waitFor();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.previewPageId),
    "page-B",
  );
  await warning.getByRole("button", { name: "Open preview", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.previewPageBusy);
  await page.evaluate(() => document.querySelector("app-shell").activateView("automation"));
  await manager.getByRole("button", { name: "← Response sets", exact: true }).click();
  await manager.getByRole("button", { name: "Facebook response", exact: true }).click();
  await openPreview();
  await warning.waitFor();
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").traffic.previewPageId),
    "page-A",
    "Another set’s last preview replaced this set’s page",
  );
  await warning.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => document.querySelector("app-shell").activateView("automation"));
  // Delayed lookup must not reopen after Escape or a variant/workspace/focus change.
  for (const action of ["escape", "focus", "navigate"]) {
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.deferSetPages = true;
      s.releaseSetPages = null;
    });
    await openPreview();
    await page.waitForFunction(() => globalThis.__workspaceFixture.releaseSetPages);
    if (action === "escape") await page.keyboard.press("Escape");
    if (action === "focus")
      await manager.getByRole("button", { name: "← Response sets", exact: true }).focus();
    if (action === "navigate")
      await page.evaluate(async () => {
        const shell = document.querySelector("app-shell");
        await shell.activateView("traffic");
        await shell.activateView("automation");
      });
    await page.evaluate(() => globalThis.__workspaceFixture.releaseSetPages());
    await page.waitForTimeout(30);
    assert.equal(await warning.count(), 0);
    assert.equal(await chooser.isVisible(), false);
  }
  // Explicit Change saves only metadata. Busy Use stays focused, accepts one submit,
  // and closing a pending write never navigates on late completion.
  await manager.getByRole("button", { name: "Change…", exact: true }).click();
  await chooser.waitFor();
  await chooser.getByRole("radio", { name: /Request #43/ }).check();
  await page.evaluate(() => (globalThis.__workspaceFixture.deferSetPageSave = true));
  const calls = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.set_experiment_preview_page,
  );
  const use = chooser.getByRole("button", { name: "Use page", exact: true });
  await use.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.releaseSetPageSave);
  assert.equal(await use.evaluate((button) => button.getRootNode().activeElement === button), true);
  await use.press("Enter");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.set_experiment_preview_page),
    calls + 1,
  );
  await page.keyboard.press("Escape");
  await page.evaluate(() => globalThis.__workspaceFixture.releaseSetPageSave());
  await page.waitForFunction(
    () => !document.querySelector("app-shell").responseSetPagePicker.saving,
  );
  assert.equal(await warning.count(), 0);
  assert.equal((await data()).responseSets[0].previewPage.entryId, "same-url");
  // Reused URL and entry ID after restart cannot silently retarget the remembered page.
  const saved = await data();
  await page.evaluate(() => {
    const s = globalThis.__workspaceFixture;
    s.captureNonce = "capture-B";
    document.querySelector("app-shell").automation.responseSets.refreshPage();
  });
  await page.waitForFunction(
    () =>
      document.querySelector("app-shell").automation.responseSets.pageAvailability ===
      "Capture unavailable",
  );
  await openPreview();
  await chooser.waitFor();
  assert.equal(await chooser.getByRole("radio", { checked: true }).count(), 0);
  assert.match(await chooser.textContent(), /Capture unavailable/);
  await page.keyboard.press("Escape");
  assert.deepEqual(await data(), saved);
  await openPreview();
  await chooser.waitFor();
  await chooser.getByRole("radio", { name: /Request #41/ }).check();
  await chooser.getByRole("button", { name: "Use page", exact: true }).click();
  await warning.waitFor();
  assert.equal(
    await page.evaluate(
      () => document.querySelector("app-shell").traffic.previewPageReference.capture,
    ),
    "capture-B",
  );
  await warning.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => document.querySelector("app-shell").activateView("automation"));
  await page.evaluate(
    (old) => document.querySelector("app-shell").automation.renderAutomation(old),
    before,
  );
  assert.match(
    await manager.locator(".set-page > span").textContent(),
    /https:\/\/transmog.net\//,
    "Stale same-generation refresh erased a newer page choice",
  );
  for (const name of ["start_proxy", "open_trace_dialog", "activate_automation"])
    assert.equal(
      await page.evaluate((name) => globalThis.__workspaceFixture.calls[name] ?? 0, name),
      0,
    );
  await page.setViewportSize({ width: 1440, height: 900 });
}
