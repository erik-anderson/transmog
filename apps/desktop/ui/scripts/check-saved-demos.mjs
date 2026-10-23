import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkSavedDemos(page, root) {
  const output = resolve(root, "../../../target/response-set-workspaces/phase5");
  await mkdir(output, { recursive: true });
  page.setDefaultTimeout(15000);
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").workspaceLoaded);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      state = globalThis.__workspaceFixture;
    state.demoSessions = structuredClone(state.sessions);
    state.sessions[0] = {
      ...state.sessions[0],
      contentType: "text/html",
      url: "https://example.test/captured/" + "long-page-context/".repeat(12),
      method: "GET",
    };
    const owner = {
      id: "demo-owner",
      incarnation: "demo-owner-created-once",
      name: "Checkout demonstration task",
      notes: "A durable demonstration",
      revision: 1,
      modifiedAt: Date.now(),
      rules: [],
      versions: [],
      applied: null,
      undoApply: null,
      paused: false,
      previewPage: null,
      previewPageRevision: 0,
    };
    state.automation = {
      generation: 20,
      rules: [],
      autoresponsesEnabled: false,
      diagnostics: [],
      responseSets: [owner],
    };
    await shell.activateView("automation");
    shell.automation.renderAutomation(structuredClone(state.automation));
    await shell.automation.showAutomationSection("experiments");
    await shell.automation.openResponseSet(
      new CustomEvent("set-open", { detail: { id: owner.id } }),
      true,
    );
  });
  const manager = page.locator("response-set-workspace"),
    lifecycle = manager.locator("response-set-lifecycle"),
    demos = page.locator("response-set-demos"),
    save = demos.locator('[aria-labelledby="response-demo-save-title"]'),
    list = demos.locator('[aria-labelledby="response-demos-title"]'),
    del = demos.locator('[aria-labelledby="response-demo-delete-title"]'),
    warning = page.locator("captured-preview .captured-page-warning"),
    report = page.locator("captured-preview .captured-preview-report");
  const tryAction = async (text) => {
    await lifecycle.getByRole("button", { name: "Try changes…", exact: true }).click();
    await lifecycle
      .locator("#set-try-menu")
      .getByRole("button", { name: text, exact: true })
      .click();
  };
  const savedList = async () => {
    await lifecycle.getByRole("button", { name: "More…", exact: true }).click();
    await lifecycle
      .locator("#set-more-menu")
      .getByRole("button", { name: "Saved demos…", exact: true })
      .click();
    await list.waitFor();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
    );
  };
  const ready = () =>
    page.waitForFunction(
      () =>
        !document.querySelector("app-shell").automation.responseSetDemos.pagesLoading &&
        !document.querySelector("app-shell").automation.responseSetDemos.busy,
    );
  const prepare = async (name) => {
    await ready();
    await save.getByLabel("Demo name", { exact: true }).fill(name);
    await save.getByRole("radio").first().check();
    await save.getByRole("button", { name: "Review demo", exact: true }).click();
    await ready();
  };
  await savedList();
  assert.equal(await list.getByText("No saved demos.", { exact: false }).isVisible(), true);
  await list.getByRole("button", { name: "Close", exact: true }).click();
  await tryAction("Save demo…");
  await ready();
  assert.equal(
    await save.getByRole("radio", { checked: true }).count(),
    0,
    "A captured page was silently selected",
  );
  assert.equal(
    await save.getByRole("button", { name: "Review demo", exact: true }).isDisabled(),
    true,
  );
  const name = "Checkout <saved> demo " + "long name ".repeat(9);
  await prepare(name);
  assert.equal(
    await save.getByText("Some resources were not captured.", { exact: false }).isVisible(),
    true,
  );
  assert.equal(
    await save.getByText("304 has no representation body", { exact: true }).isVisible(),
    true,
  );
  assert.equal(
    await save.getByText("Response bytes were evicted", { exact: true }).isVisible(),
    true,
  );
  assert.equal(
    await save.getByText("Resource exceeds the 32 MiB preview limit", { exact: true }).isVisible(),
    true,
  );
  assert.equal(
    await save
      .getByRole("button", { name: "Save demo", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Review completion lost keyboard focus",
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({ path: resolve(output, `demo-review-${width}.png`) });
    assert.equal(await save.evaluate((node) => node.scrollWidth <= node.clientWidth + 2), true);
    await save.getByRole("button", { name: "Save demo", exact: true }).scrollIntoViewIfNeeded();
    assert.equal(
      await save.getByRole("button", { name: "Save demo", exact: true }).isVisible(),
      true,
    );
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({ path: resolve(output, "demo-review-forced-colors.png") });
  await page.emulateMedia({ forcedColors: "none" });
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  await page.screenshot({ path: resolve(output, "demo-review-zoom.png") });
  await page.evaluate(() => {
    document.documentElement.style.zoom = "";
    globalThis.__workspaceFixture.demoByteVersion = "edited after review";
  });
  const token = await page.evaluate(
    () => document.querySelector("app-shell").automation.responseSetDemos.review.token,
  );
  await save.getByRole("button", { name: "Save demo", exact: true }).click();
  await save.waitFor({ state: "hidden" });
  assert.deepEqual(
    await page.evaluate(() => ({
      count: globalThis.__workspaceFixture.demoPrepareCount,
      token: globalThis.__workspaceFixture.demoCommitArgs.token,
      bytes: globalThis.__workspaceFixture.demoSavedByteVersion,
      page: globalThis.__workspaceFixture.automation.responseSets[0].previewPage,
    })),
    { count: 1, token, bytes: "original", page: null },
    "Save silently refroze or changed page association",
  );
  await savedList();
  await list.getByRole("button", { name: "Open demo…", exact: true }).click();
  await warning.waitFor();
  assert.match(await warning.textContent(), /Frozen captured page and response changes/);
  assert.equal(
    await warning.getByLabel("Enable scripts for this preview", { exact: true }).isChecked(),
    false,
  );
  assert.equal(await warning.getByLabel("Resource source", { exact: true }).isDisabled(), true);
  assert.equal(await warning.getByLabel("Responses", { exact: true }).count(), 0);
  await warning.getByRole("button", { name: "Open preview", exact: true }).click();
  await warning.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.openSavedDemoArgs.enableScripts),
    false,
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.previewArgs), undefined);
  await page.evaluate(() => document.querySelector("app-shell").traffic.showPreviewReport());
  await report.waitFor();
  assert.match(await report.textContent(), /Saved demo revision 1/);
  assert.equal(
    await report.getByRole("button", { name: "Open updated preview…", exact: true }).isDisabled(),
    true,
  );
  assert.equal(
    await report.getByRole("button", { name: "Open baseline…", exact: true }).isDisabled(),
    true,
  );
  await report.getByRole("button", { name: "Close", exact: true }).click();
  await savedList();
  await list.getByRole("button", { name: "Open demo…", exact: true }).click();
  await warning.getByLabel("Enable scripts for this preview", { exact: true }).check();
  await warning.getByRole("button", { name: "Open preview", exact: true }).click();
  await warning.waitFor({ state: "hidden" });
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.openSavedDemoArgs.enableScripts),
    true,
  );
  // Replacement failures retain the exact reviewed token for retry.
  await savedList();
  await list.getByRole("button", { name: "Replace…", exact: true }).click();
  await prepare("Replacement demo");
  const replaceToken = await page.evaluate(
    () => document.querySelector("app-shell").automation.responseSetDemos.review.token,
  );
  await page.evaluate(
    () =>
      (globalThis.__workspaceFixture.demoCommitError =
        "Disk unavailable: retry this exact snapshot"),
  );
  await save.getByRole("button", { name: "Replace demo", exact: true }).click();
  await ready();
  assert.match(await save.locator(".field-error").textContent(), /Disk unavailable/);
  assert.equal(
    await page.evaluate(
      () => document.querySelector("app-shell").automation.responseSetDemos.review.token,
    ),
    replaceToken,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.demoCommitError = null));
  await save.getByRole("button", { name: "Replace demo", exact: true }).click();
  await save.waitFor({ state: "hidden" });
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
  );
  assert.equal(
    await list.getByRole("heading", { name: "Replacement demo", exact: true }).isVisible(),
    true,
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.savedDemos[0].revision), 2);
  await page.evaluate(
    () =>
      (globalThis.__workspaceFixture.focusDemo = structuredClone(
        globalThis.__workspaceFixture.savedDemos[0],
      )),
  );
  // CAS rejects an intervening replacement; a refreshed list is the recovery path.
  await list.getByRole("button", { name: "Replace…", exact: true }).click();
  await prepare("Stale replacement");
  await page.evaluate(() => globalThis.__workspaceFixture.savedDemos[0].revision++);
  await save.getByRole("button", { name: "Replace demo", exact: true }).click();
  await ready();
  assert.match(await save.locator(".field-error").textContent(), /changed or was deleted/);
  await save.getByRole("button", { name: "Cancel", exact: true }).click();
  await list.getByRole("button", { name: "Refresh", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
  );
  await list.getByRole("button", { name: "Delete…", exact: true }).click();
  await page.keyboard.press("Escape");
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.savedDemos.length), 1);
  await list.getByRole("button", { name: "Delete…", exact: true }).click();
  await del.getByRole("button", { name: "Delete demo", exact: true }).click();
  await del.waitFor({ state: "hidden" });
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
  );
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.savedDemos.length), 0);
  assert.equal(
    await list
      .getByRole("button", { name: "Close", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Deleting the last row lost focus inside the modal",
  );
  await list.getByRole("button", { name: "Close", exact: true }).click();
  // Repair/Delete stays usable for unavailable snapshots, and late list refresh respects moved focus.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.savedDemos = [
      {
        ...structuredClone(state.focusDemo),
        availabilityError: "Stored demo metadata is damaged. Restore, replace, or delete it.",
      },
    ];
  });
  await savedList();
  assert.equal(
    await list.getByRole("button", { name: "Open demo…", exact: true }).isDisabled(),
    true,
  );
  assert.match(await list.locator(".field-error").first().textContent(), /damaged/);
  await list.getByRole("button", { name: "Delete…", exact: true }).click();
  await page.evaluate(() => (globalThis.__workspaceFixture.deferDemoList = true));
  await del.getByRole("button", { name: "Delete demo", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoList);
  await list.getByRole("button", { name: "Save demo…", exact: true }).focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishDemoList());
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
  );
  assert.equal(
    await list
      .getByRole("button", { name: "Save demo…", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "A delayed deletion refresh moved focus",
  );
  await list.getByRole("button", { name: "Close", exact: true }).click();
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.savedDemos = [structuredClone(state.focusDemo)];
    state.finishDemoDelete = null;
    state.deferDemoDelete = true;
  });
  await savedList();
  await list.getByRole("button", { name: "Delete…", exact: true }).click();
  await del.getByRole("button", { name: "Delete demo", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoDelete);
  await del.getByRole("button", { name: "Cancel", exact: true }).focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishDemoDelete());
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSetDemos.deletePending,
  );
  assert.equal(await del.isVisible(), true);
  assert.equal(
    await del
      .getByRole("button", { name: "Close", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Delayed deletion took moved confirmation focus",
  );
  await del.getByRole("button", { name: "Close", exact: true }).click();
  await list.getByRole("button", { name: "Close", exact: true }).click();
  // Cancellation rejects a late preparation; moved focus survives an accepted preparation.
  await tryAction("Save demo…");
  await ready();
  await save.getByLabel("Demo name", { exact: true }).fill("Canceled");
  await save.getByRole("radio").first().check();
  await page.evaluate(() => (globalThis.__workspaceFixture.deferDemoPrepare = true));
  await save.getByRole("button", { name: "Review demo", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoPrepare);
  await save.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.finishDemoPrepare());
  await page.waitForFunction(() => globalThis.__workspaceFixture.demoCanceledTokens?.length > 1);
  assert.equal(await save.isVisible(), false);
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.savedDemos.length), 0);
  await tryAction("Save demo…");
  await ready();
  await save.getByLabel("Demo name", { exact: true }).fill("Delayed review");
  await save.getByRole("radio").first().check();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.finishDemoPrepare = null;
    globalThis.__workspaceFixture.deferDemoPrepare = true;
  });
  await save.getByRole("button", { name: "Review demo", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoPrepare);
  await save.getByRole("button", { name: "Cancel", exact: true }).focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishDemoPrepare());
  await ready();
  assert.equal(
    await save
      .getByRole("button", { name: "Cancel", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await page.keyboard.press("Escape");
  // An explicitly selected older HTML page is included beyond the first 100 without being selected in the picker.
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      state = globalThis.__workspaceFixture,
      base = state.sessions[0];
    state.sessions = Array.from({ length: 105 }, (_, index) => ({
      ...structuredClone(base),
      id: "demo-page-" + index,
      requestNumber: String(index + 1),
      url: "https://example.test/pages/" + index,
      path: "/pages/" + index,
      contentType: "text/html",
      startedAt: 1800000000000 + index,
    }));
    await shell.activateView("traffic");
    await shell.traffic.refreshSessions(undefined, true);
    await shell.traffic.revealSession("demo-page-104");
    await shell.activateView("automation");
  });
  await tryAction("Save demo…");
  await ready();
  assert.equal(await save.getByRole("radio", { checked: true }).count(), 0);
  const oldPage = save
    .locator(".response-demo-pages label")
    .filter({ hasText: "https://example.test/pages/104" });
  await oldPage.getByRole("radio").check();
  await save.getByLabel("Demo name", { exact: true }).fill("Explicit older HTML");
  await save.getByRole("button", { name: "Review demo", exact: true }).click();
  await ready();
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.demoSaveInput.entryId),
    "demo-page-104",
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.automation.responseSets[0].previewPage),
    null,
  );
  await page.keyboard.press("Escape");
  await page.evaluate(() => (globalThis.__workspaceFixture.sessions = []));
  await tryAction("Save demo…");
  await ready();
  assert.equal(
    await save
      .getByText("No complete captured HTML pages are available.", { exact: false })
      .isVisible(),
    true,
  );
  assert.equal(
    await save.getByRole("button", { name: "Review demo", exact: true }).isDisabled(),
    true,
  );
  await page.keyboard.press("Escape");
  await checkSavedDemoOwnerSwitch(page, output);
}

async function checkSavedDemoOwnerSwitch(page, output) {
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").workspaceLoaded);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      state = globalThis.__workspaceFixture;
    const owner = (id) => ({
      id,
      incarnation: id + "-created-once",
      name: id === "demo-a" ? "Owner A" : "Owner B",
      notes: "",
      revision: 1,
      modifiedAt: Date.now(),
      rules: [],
      versions: [],
      applied: null,
      undoApply: null,
      paused: false,
      previewPage: null,
      previewPageRevision: 0,
    });
    const owners = [owner("demo-a"), owner("demo-b")];
    state.automation = {
      generation: 20,
      rules: [],
      autoresponsesEnabled: false,
      diagnostics: [],
      responseSets: owners,
    };
    state.savedDemos = owners.map((set) => ({
      id: set.id + "-saved",
      revision: 1,
      ownerId: set.id,
      ownerIncarnation: set.incarnation,
      name: set.name + " demo",
      createdAt: Date.now(),
      modifiedAt: Date.now(),
      selection: { experimentId: set.id, incarnation: set.incarnation, variantId: "", revision: 1 },
      entryId: "first",
      url: "https://example.test/" + set.id,
      scope: "original-trace",
      available: 1,
      skipped: 0,
      bytes: 100,
    }));
    await shell.activateView("automation");
    shell.automation.renderAutomation(structuredClone(state.automation));
    await shell.automation.showAutomationSection("experiments");
    shell.automation.responseSetDemos.addEventListener(
      "saved-demo-preview",
      () => (state.ownerSwitchPreviewEvents = (state.ownerSwitchPreviewEvents ?? 0) + 1),
    );
  });
  const demos = page.locator("response-set-demos"),
    list = demos.locator('[aria-labelledby="response-demos-title"]'),
    lifecycle = page.locator("response-set-lifecycle");
  const ready = () =>
    page.waitForFunction(
      () => !document.querySelector("app-shell").automation.responseSetDemos.loading,
    );
  const show = async (id) => {
    await page.evaluate(
      (id) =>
        document
          .querySelector("app-shell")
          .automation.openResponseSet(new CustomEvent("set-open", { detail: { id } }), true),
      id,
    );
    await lifecycle.getByRole("button", { name: "More…", exact: true }).click();
    await lifecycle
      .locator("#set-more-menu")
      .getByRole("button", { name: "Saved demos…", exact: true })
      .click();
    await list.waitFor();
  };
  const rejectOldActions = async () => {
    await page.evaluate(async () => {
      const demos = document.querySelector("app-shell").automation.responseSetDemos;
      demos.openDemo("demo-a-saved");
      demos.askDelete("demo-a-saved");
      demos.startSave("demo-a-saved");
      await demos.confirmDelete();
    });
    assert.equal(
      await demos.locator('[aria-labelledby="response-demo-delete-title"]').isVisible(),
      false,
    );
    assert.equal(
      await demos.locator('[aria-labelledby="response-demo-save-title"]').isVisible(),
      false,
    );
    assert.deepEqual(
      await page.evaluate(() => ({
        preview: globalThis.__workspaceFixture.ownerSwitchPreviewEvents ?? 0,
        deleted: globalThis.__workspaceFixture.demoDeleteArgs ?? null,
        ids: globalThis.__workspaceFixture.savedDemos.map((demo) => demo.id),
      })),
      { preview: 0, deleted: null, ids: ["demo-a-saved", "demo-b-saved"] },
      "A stale owner action opened or deleted another set’s demo",
    );
  };
  for (const failure of [false, true]) {
    await show("demo-a");
    await ready();
    assert.equal(
      await list.getByRole("heading", { name: "Owner A demo", exact: true }).isVisible(),
      true,
    );
    await list.getByRole("button", { name: "Close", exact: true }).click();
    await page.evaluate((failure) => {
      const state = globalThis.__workspaceFixture;
      state.deferDemoList = true;
      state.finishDemoList = null;
      state.demoListError = failure ? "Owner B storage unavailable" : null;
    }, failure);
    await show("demo-b");
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoList);
    assert.equal(
      await list.getByRole("heading", { name: "Saved demos · Owner B", exact: true }).isVisible(),
      true,
    );
    assert.equal(
      await list.locator("article").count(),
      0,
      "Owner A rows remained visible during Owner B load",
    );
    await rejectOldActions();
    if (!failure) await page.screenshot({ path: resolve(output, "demo-owner-switch-loading.png") });
    await page.evaluate(() => globalThis.__workspaceFixture.finishDemoList());
    await ready();
    if (failure) {
      assert.match(await list.locator(".field-error").textContent(), /Owner B storage unavailable/);
      assert.equal(await list.locator("article").count(), 0);
      assert.equal(
        await list.getByText("No saved demos.", { exact: false }).isVisible(),
        false,
        "Failed storage was misreported as an empty library",
      );
      await rejectOldActions();
      await page.screenshot({ path: resolve(output, "demo-owner-switch-failed.png") });
      await page.evaluate(() => (globalThis.__workspaceFixture.demoListError = null));
      await list.getByRole("button", { name: "Refresh", exact: true }).click();
      await ready();
    }
    assert.equal(
      await list.getByRole("heading", { name: "Owner B demo", exact: true }).isVisible(),
      true,
    );
    assert.equal(await list.getByRole("heading", { name: "Owner A demo", exact: true }).count(), 0);
    // A normal refresh keeps the current owner's rows but suspends actions until the new result arrives.
    await page.evaluate(() => {
      globalThis.__workspaceFixture.deferDemoList = true;
      globalThis.__workspaceFixture.finishDemoList = null;
    });
    await list.getByRole("button", { name: "Refresh", exact: true }).click();
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishDemoList);
    assert.equal(
      await list.getByRole("heading", { name: "Owner B demo", exact: true }).isVisible(),
      true,
    );
    for (const name of ["Open demo…", "Replace…", "Delete…"])
      assert.equal(await list.getByRole("button", { name, exact: true }).isDisabled(), true);
    await page.evaluate(() => globalThis.__workspaceFixture.finishDemoList());
    await ready();
    assert.equal(
      await list.getByRole("button", { name: "Delete…", exact: true }).isEnabled(),
      true,
    );
    await page.evaluate(
      () => (globalThis.__workspaceFixture.demoListError = "Refresh unavailable"),
    );
    await list.getByRole("button", { name: "Refresh", exact: true }).click();
    await ready();
    assert.equal(
      await list.getByRole("heading", { name: "Owner B demo", exact: true }).isVisible(),
      true,
    );
    for (const name of ["Open demo…", "Replace…", "Delete…"])
      assert.equal(await list.getByRole("button", { name, exact: true }).isDisabled(), true);
    await page.evaluate(async () => {
      const demos = document.querySelector("app-shell").automation.responseSetDemos;
      demos.openDemo("demo-b-saved");
      demos.askDelete("demo-b-saved");
      demos.startSave("demo-b-saved");
      await demos.confirmDelete();
    });
    await rejectOldActions();
    await page.evaluate(() => (globalThis.__workspaceFixture.demoListError = null));
    await list.getByRole("button", { name: "Close", exact: true }).click();
  }
}
