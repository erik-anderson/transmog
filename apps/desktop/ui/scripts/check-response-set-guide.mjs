import assert from "node:assert/strict";
import { resolve } from "node:path";

/** Optional current-workspace guide; preference writes never mutate saved work. */
export async function checkResponseSetGuide(page, root) {
  await page.addInitScript(() => sessionStorage.setItem("startup-update-tested", "true"));
  const ready = async () => {
    await page.goto("https://workspace.test/");
    await page.waitForFunction(() => document.querySelector("app-shell").workspaceLoaded);
  };
  await ready();
  await page.evaluate(() => localStorage.removeItem("workspace"));
  await ready();
  const manager = page.locator("response-set-workspace"),
    panel = manager.locator("response-set-tour"),
    tabs = page.getByRole("navigation", { name: "Automation tools" });
  const more = manager.getByRole("button", { name: "More…", exact: true });
  const help = async () => {
    await more.click();
    await manager.getByRole("button", { name: "How response sets work", exact: true }).click();
  };
  const open = async () => {
    await page.evaluate(async () => {
      const s = document.querySelector("app-shell");
      await s.activateView("automation");
      await s.automation.showAutomationSection("experiments");
    });
  };
  const seed = async (empty = false) =>
    page.evaluate(async (empty) => {
      const state = globalThis.__workspaceFixture,
        shell = document.querySelector("app-shell"),
        a = shell.automation,
        action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
      const rule = {
        id: "guide-rule",
        displayName: "Checkout response",
        enabled: true,
        revision: 1,
        priority: 1,
        matcher: {
          method: "GET",
          url: { kind: "exact", value: "https://example.test/checkout.js" },
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
          ...action,
          responseAsset: "guide-response@1",
          allowNonIdempotentBodyReplacement: false,
        },
        response: action,
      };
      const set = (id) => ({
        id,
        incarnation: "owner-" + id,
        name: "Checkout changes",
        notes: "Notes kept during the tour",
        revision: 2,
        modifiedAt: 1800000000000,
        rules: empty ? [] : [rule],
        versions: [],
        applied: { revision: 1, rules: [rule] },
        undoApply: { generation: 9, applied: null, paused: true },
        paused: true,
        previewPage: null,
        previewPageRevision: 0,
      });
      state.assets = [
        {
          id: "guide-response",
          revision: 1,
          status: 200,
          headers: [],
          display: "Saved guide body",
          bodyBytes: 16,
          sha256: "0".repeat(64),
          mediaType: "application/javascript",
          provenance: { kind: "authored" },
        },
      ];
      state.automation = {
        schemaVersion: 2,
        generation: 9,
        rules: [],
        autoresponsesEnabled: false,
        diagnostics: [],
        responseSets: [set("guide-set"), set("other-set")],
      };
      a.renderAutomation(structuredClone(state.automation));
      await a.openResponseSet(new CustomEvent("set-open", { detail: { id: "guide-set" } }));
    }, empty);
  const defer = async (failure = false) =>
    page.evaluate((failure) => {
      const s = globalThis.__workspaceFixture;
      s.deferGuide = true;
      s.finishGuide = null;
      s.guideFailure = failure;
    }, failure);
  const pending = async () => {
    await page.waitForFunction(() => globalThis.__workspaceFixture.finishGuide);
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
  };
  const finish = async () => {
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.deferGuide = false;
      s.finishGuide();
    });
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.responseSets.guidePending,
    );
  };
  const start = async () => {
    await help();
    await manager.getByRole("button", { name: "Replay tour", exact: true }).click();
    await panel.getByText("1 of 4", { exact: true }).waitFor();
  };
  await open();
  await page.evaluate(() => (document.querySelector("app-shell").workspaceLoaded = false));
  await seed();
  assert.equal(
    await manager.locator(".response-set-invitation").count(),
    0,
    "Invitation preceded native preferences",
  );
  await page.evaluate(() => (document.querySelector("app-shell").workspaceLoaded = true));
  await manager.getByText("New to response sets?", { exact: true }).waitFor();
  assert.equal(await panel.count(), 0, "Invitation opened automatically");
  assert.match(await manager.locator(".set-live-controls").textContent(), /Paused/);
  const invitation = manager
    .locator(".response-set-invitation")
    .getByRole("button", { name: "Take a quick tour", exact: true });
  await defer(true);
  await invitation.click();
  await pending();
  assert.equal(
    await invitation.evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  const calls = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.update_response_set_guide,
  );
  await page.keyboard.press("Enter");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.update_response_set_guide),
    calls,
  );
  await finish();
  await manager
    .getByText("Tour could not start: Fixture preference save failed", { exact: true })
    .waitFor();
  assert.equal(
    await invitation.evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await defer();
  await invitation.click();
  await pending();
  await finish();
  await panel.getByText("1 of 4", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  // Dismissal is durable independently of starting or completing the guide.
  await page.evaluate(() => localStorage.removeItem("workspace"));
  await ready();
  await open();
  await seed();
  await manager.getByText("New to response sets?", { exact: true }).waitFor();
  await manager.getByRole("button", { name: "Not now", exact: true }).click();
  assert.equal(await more.evaluate((node) => node.getRootNode().activeElement === node), true);
  await ready();
  await open();
  await seed();
  assert.equal(
    await manager.locator(".response-set-invitation").count(),
    0,
    "Dismissal did not survive reload",
  );
  const before = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.automation),
  );
  const changesBefore = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.change_response_set ?? 0,
  );
  await help();
  await defer();
  const starts = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.update_response_set_guide ?? 0,
  );
  await page.evaluate(() => {
    const v = document.querySelector("app-shell").automation.responseSets;
    void v.startGuide();
    void v.startGuide();
  });
  await pending();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.update_response_set_guide),
    starts + 1,
  );
  await tabs.getByRole("button", { name: "Rules", exact: true }).click();
  await finish();
  assert.equal(await panel.count(), 0);
  assert.equal(
    await tabs
      .getByRole("button", { name: "Rules", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await open();
  await seed();
  await help();
  await defer();
  await page.keyboard.press("Tab");
  const replay = manager.getByRole("button", { name: "Replay tour", exact: true });
  assert.equal(await replay.evaluate((node) => node.getRootNode().activeElement === node), true);
  await page.keyboard.press("Enter");
  await pending();
  assert.equal(await replay.evaluate((node) => node.getRootNode().activeElement === node), true);
  await finish();
  await panel.getByText("1 of 4", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  for (const mode of ["focus", "close", "selection", "busy", "navigate"]) {
    await help();
    await defer();
    await replay.click();
    await pending();
    if (mode === "focus") await page.keyboard.press("Tab");
    else if (mode === "close")
      await manager.getByRole("button", { name: "Close", exact: true }).click();
    else if (mode === "selection")
      await page.evaluate(() =>
        document.querySelector("app-shell").automation.responseSets.open("other-set"),
      );
    else if (mode === "busy")
      await page.evaluate(
        () => (document.querySelector("app-shell").automation.responseSets.busy = true),
      );
    else await page.evaluate(() => document.querySelector("app-shell").activateView("traffic"));
    await finish();
    assert.equal(await panel.count(), 0, `Late guide survived ${mode}`);
    if (mode === "focus") {
      const close = manager.getByRole("button", { name: "Close", exact: true });
      assert.equal(await close.evaluate((node) => node.getRootNode().activeElement === node), true);
      await close.click();
    }
    if (mode === "busy")
      await page.evaluate(
        () => (document.querySelector("app-shell").automation.responseSets.busy = false),
      );
    if (mode === "navigate") await open();
    if (mode === "selection")
      await page.evaluate(() =>
        document.querySelector("app-shell").automation.responseSets.open("guide-set"),
      );
  }
  const titles = [
    "Keep a debugging task together",
    "Collect your response changes",
    "Save your work",
    "Try it now or demonstrate it later",
  ];
  // Every step is optional, can end early, and replay always begins at one.
  for (let step = 1; step <= 4; step++) {
    await start();
    for (let n = 1; n < step; n++)
      await panel.getByRole("button", { name: "Next", exact: true }).click();
    await panel.getByRole("heading", { name: titles[step - 1], exact: true }).waitFor();
    assert.equal(await manager.locator("[data-tour-target]").count(), 1);
    assert.equal(await more.isDisabled(), true);
    assert.equal(
      await manager.getByRole("button", { name: "Add responses…", exact: true }).isDisabled(),
      true,
    );
    await page.keyboard.press("Control+s");
    await page.keyboard.press("Delete");
    await (step % 2
      ? panel.getByRole("button", { name: "End tour", exact: true }).click()
      : page.keyboard.press("Escape"));
    assert.equal(await panel.count(), 0);
    assert.equal(await more.evaluate((node) => node.getRootNode().activeElement === node), true);
  }
  await start();
  await panel.getByRole("button", { name: "Next", exact: true }).click();
  await panel.getByRole("button", { name: "Back", exact: true }).click();
  await panel.getByText("1 of 4", { exact: true }).waitFor();
  await page.keyboard.press("Escape");
  // Refresh must retain current drafts and focused guide controls.
  await manager
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Unfinished guide notes");
  await start();
  await page.evaluate(() => {
    const a = document.querySelector("app-shell").automation;
    a.renderAutomation(structuredClone(globalThis.__workspaceFixture.automation));
  });
  assert.equal(
    await panel
      .getByRole("button", { name: "Next", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  assert.equal(
    await manager
      .locator(".response-set-current")
      .getByLabel("Notes", { exact: true })
      .inputValue(),
    "Unfinished guide notes",
  );
  await page.keyboard.press("Escape");
  await manager.getByRole("button", { name: "Discard changes", exact: true }).click();
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await start();
    for (let step = 1; step <= 4; step++) {
      await panel.getByText(`${step} of 4`, { exact: true }).waitFor();
      const next = panel.getByRole("button", { name: step === 4 ? "Done" : "Next", exact: true });
      await next.scrollIntoViewIfNeeded();
      assert.equal(
        await next.evaluate((node) => {
          const r = node.getBoundingClientRect();
          return r.top >= 0 && r.bottom <= innerHeight;
        }),
        true,
      );
      await page.screenshot({
        path: resolve(
          root,
          `../../../target/response-set-workspaces/phase6/guide-${width}-step-${step}.png`,
        ),
      });
      await next.click();
    }
  }
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  await page.emulateMedia({ forcedColors: "active" });
  await start();
  for (let step = 1; step <= 4; step++) {
    const next = panel.getByRole("button", { name: step === 4 ? "Done" : "Next", exact: true });
    await next.scrollIntoViewIfNeeded();
    assert.equal(
      await next.evaluate((node) => {
        const r = node.getBoundingClientRect();
        return r.top >= 0 && r.bottom <= innerHeight;
      }),
      true,
    );
    await page.screenshot({
      path: resolve(
        root,
        `../../../target/response-set-workspaces/phase6/guide-zoom-contrast-${step}.png`,
      ),
    });
    await next.click();
  }
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.emulateMedia({ forcedColors: "none", colorScheme: "dark" });
  await start();
  await page.screenshot({
    path: resolve(root, "../../../target/response-set-workspaces/phase6/guide-dark.png"),
  });
  await page.keyboard.press("Escape");
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.automation),
    before,
    "Tour changed saved work, generation, live snapshot or Undo",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.change_response_set ?? 0),
    changesBefore,
  );
  // The tour is nonmodal: shortcuts from visible navigation and the sibling
  // response editor must not bypass the disabled collection Save action.
  await page.setViewportSize({ width: 1440, height: 900 });
  await manager.getByRole("button", { name: "Checkout response", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.loadingSavedResponse,
  );
  const editor = page.locator("automation-workspace .auto-response-editor"),
    notes = manager.locator(".response-set-current").getByLabel("Notes", { exact: true }),
    back = manager.getByRole("button", { name: "← Response sets", exact: true });
  await editor.locator('[name="body"]').fill("Tour body draft");
  await notes.fill("Tour notes draft");
  const savedBeforeShortcuts = await page.evaluate(() =>
      structuredClone(globalThis.__workspaceFixture.automation),
    ),
    writesBeforeShortcuts = await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      return [
        "change_response_set",
        "edit_response_asset",
        "create_response_asset",
        "validate_automation",
        "activate_automation",
      ].map((command) => [command, s.calls[command] ?? 0]);
    });
  await start();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Shift+Tab");
  assert.equal(
    await back.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Keyboard did not reach the visible collection Back control",
  );
  await page.keyboard.press("Control+s");
  assert.equal(await panel.count(), 1);
  assert.equal(await notes.inputValue(), "Tour notes draft");
  assert.equal(
    await back.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Blocked shortcut moved Back focus",
  );
  await editor.locator('[name="body"]').focus();
  await page.keyboard.press("Control+s");
  assert.equal(
    await editor
      .locator('[name="body"]')
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Blocked editor shortcut moved draft focus",
  );
  assert.equal(await editor.locator('[name="body"]').inputValue(), "Tour body draft");
  // Also pin the persistence boundary used by submitted/expanded editors.
  assert.equal(
    await page.evaluate(async () => {
      const a = document.querySelector("app-shell").automation;
      return await a.saveCollection();
    }),
    false,
  );
  assert.equal(
    await page.evaluate(async () =>
      document
        .querySelector("app-shell")
        .automation.saveAutoResponse(new Event("submit", { cancelable: true })),
    ),
    false,
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.automation),
    savedBeforeShortcuts,
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      return [
        "change_response_set",
        "edit_response_asset",
        "create_response_asset",
        "validate_automation",
        "activate_automation",
      ].map((command) => [command, s.calls[command] ?? 0]);
    }),
    writesBeforeShortcuts,
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    true,
  );
  await page.screenshot({
    path: resolve(
      root,
      "../../../target/response-set-workspaces/phase6/guide-shortcuts-preserve-draft.png",
    ),
  });
  await panel.getByRole("button", { name: "End tour", exact: true }).click();
  await back.focus();
  await page.keyboard.press("Control+s");
  await page.waitForFunction(() => {
    const a = document.querySelector("app-shell").automation;
    return !a.responseSets.busy && !a.responseSets.dirty;
  });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.responseSets[0].notes),
    "Tour notes draft",
  );
  assert.equal(await editor.locator('[name="body"]').inputValue(), "Tour body draft");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.change_response_set),
    writesBeforeShortcuts[0][1] + 1,
    "Ending the tour did not reenable collection Ctrl+S",
  );
  // Natural Tab can leave this nonmodal callout for the sibling editor. Its
  // Escape belongs to the active tour and returns to visible help focus.
  await notes.fill("Escape keeps this guide draft");
  const beforeEditorEscape = await page.evaluate(() =>
    structuredClone(globalThis.__workspaceFixture.automation),
  );
  await start();
  await page.keyboard.press("Tab");
  const responseName = editor.getByLabel("Response name", { exact: true });
  assert.equal(
    await responseName.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Tab from first Next did not reach the response editor",
  );
  await page.keyboard.press("Escape");
  assert.equal(await panel.count(), 0, "Escape from the sibling editor did not end the tour");
  assert.equal(await more.evaluate((node) => node.getRootNode().activeElement === node), true);
  assert.equal(await notes.inputValue(), "Escape keeps this guide draft");
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    true,
  );
  assert.deepEqual(
    await page.evaluate(() => globalThis.__workspaceFixture.automation),
    beforeEditorEscape,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/response-set-workspaces/phase6/guide-editor-escape.png"),
  });
  await responseName.focus();
  await page.keyboard.press("Escape");
  assert.equal(
    await responseName.evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Inactive guide changed ordinary editor Escape",
  );
  assert.equal(await editor.isVisible(), true);
  // Intentional navigation still resolves the ordinary whole-collection draft
  // boundary, including Keep editing, Escape and Save changes.
  await notes.fill("Navigation draft after tour");
  await start();
  await back.click();
  const unsaved = page.locator("automation-workspace .unsaved-rule-dialog");
  await unsaved.waitFor({ state: "visible" });
  assert.equal(await panel.count(), 0);
  await page.keyboard.press("Escape");
  assert.equal(await notes.inputValue(), "Navigation draft after tour");
  await start();
  await back.click();
  await unsaved.getByRole("button", { name: "Save changes", exact: true }).click();
  await page.waitForFunction(() => !document.querySelector("app-shell").automation.collectionOpen);
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.responseSets[0].notes),
    "Navigation draft after tour",
  );
  await page.evaluate(() => document.querySelector("app-shell").resetWorkspace());
  await page.waitForFunction(
    () => globalThis.__workspaceFixture.workspace.responseSetGuide === "started",
  );
  await ready();
  await open();
  await seed(true);
  assert.equal(await manager.locator(".response-set-invitation").count(), 0);
  await start();
  await panel.getByRole("button", { name: "Next", exact: true }).click();
  await panel
    .getByText("This set is empty. You can finish the tour without adding a response.", {
      exact: true,
    })
    .waitFor();
  await page.keyboard.press("Escape");
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize({ width: 1440, height: 900 });
}
