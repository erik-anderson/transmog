import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkResponseSetLifecycle(page, root) {
  const evidence = resolve(root, "../../../target/response-set-workspaces/phase4");
  await mkdir(evidence, { recursive: true });
  page.setDefaultTimeout(15000);
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").workspaceLoaded);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell"),
      state = globalThis.__workspaceFixture,
      action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
    const rule = {
      id: "lifecycle-response",
      displayName: "Checkout succeeds",
      enabled: true,
      revision: 1,
      priority: -1000000,
      matcher: {
        method: "GET",
        url: { kind: "exact", value: "https://example.test/checkout" },
        requestHeaders: [],
        responseHeaders: [],
      },
      request: { ...action, responseAsset: "lifecycle-asset@1" },
      response: action,
    };
    const other = {
      ...structuredClone(rule),
      id: "ordinary-response",
      displayName: "Unrelated response",
      priority: -999999,
    };
    const set = {
      id: "lifecycle-set",
      incarnation: "lifecycle-owner",
      name: "Checkout validation",
      notes: "A saved debugging task",
      revision: 1,
      modifiedAt: Date.now(),
      rules: [rule],
      versions: [],
      applied: null,
      undoApply: null,
      paused: false,
      previewPage: {
        capture: "fixture-capture",
        entryId: "first",
        requestNumber: "1",
        traceId: null,
        url: "https://example.test/checkout",
        method: "GET",
        startedAt: 1800000000000,
        source: "Current capture",
      },
      previewPageRevision: 1,
    };
    state.assets = [
      {
        id: "lifecycle-asset",
        revision: 1,
        status: 201,
        headers: [
          {
            name: Array.from(new TextEncoder().encode("Content-Type")),
            value: Array.from(new TextEncoder().encode("application/json")),
          },
        ],
        display: '{"checkout":"success"}',
        bodyBytes: 22,
        sha256: "0".repeat(64),
        mediaType: "application/json",
        provenance: { kind: "manual" },
      },
    ];
    state.automation = {
      generation: 20,
      rules: [other],
      autoresponsesEnabled: false,
      diagnostics: [],
      responseSets: [
        set,
        {
          ...structuredClone(set),
          id: "unrelated-set",
          incarnation: "unrelated-owner",
          name: "Other task",
          rules: [],
          previewPage: null,
        },
      ],
    };
    await shell.activateView("automation");
    shell.automation.renderAutomation(structuredClone(state.automation));
    await shell.automation.showAutomationSection("experiments");
    await shell.automation.openResponseSet(
      new CustomEvent("set-open", { detail: { id: set.id } }),
      true,
    );
  });
  const manager = page.locator("response-set-workspace"),
    lifecycle = manager.locator("response-set-lifecycle"),
    versions = lifecycle.locator(".set-versions-dialog"),
    confirmation = lifecycle.locator('[aria-labelledby="set-lifecycle-confirm-title"]'),
    overlaps = lifecycle.locator('[aria-labelledby="set-overlap-title"]');
  const status = () =>
    page.evaluate(() => structuredClone(globalThis.__workspaceFixture.automation));
  const settled = () =>
    page.waitForFunction(
      () => !document.querySelector("app-shell").automation.responseSets.lifecycle.pending,
    );
  const more = async (label) => {
    await lifecycle.getByRole("button", { name: "More…", exact: true }).click();
    await lifecycle
      .locator("#set-more-menu")
      .getByRole("button", { name: label, exact: true })
      .click();
  };
  const stateText = () => lifecycle.locator(".set-live-controls > span").textContent();
  const baseline = await status();
  assert.equal(await stateText(), "Not applied to live traffic");
  assert.equal(
    await lifecycle.getByText("Saved responses are paused globally.", { exact: true }).isVisible(),
    true,
  );
  // Initial application, named targets for Undo, and global pause remain independent.
  await lifecycle.getByRole("button", { name: "Use on live traffic", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].applied.revision, 1);
  assert.equal((await status()).autoresponsesEnabled, false);
  assert.match(
    await lifecycle.locator(".set-live-undo").textContent(),
    /no previous applied responses/,
  );
  await lifecycle.getByRole("button", { name: "Undo live change", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].applied, null);
  assert.deepEqual((await status()).rules, baseline.rules);
  await page.evaluate(
    () =>
      (globalThis.__workspaceFixture.variantOverlaps = [
        "Unrelated response matches the same request.",
      ]),
  );
  await lifecycle.getByRole("button", { name: "Use on live traffic", exact: true }).click();
  await overlaps.waitFor();
  assert.equal(
    await overlaps.getByRole("button", { name: "Use on live traffic", exact: true }).isDisabled(),
    true,
  );
  await page.keyboard.press("Escape");
  await settled();
  assert.equal((await status()).responseSets[0].applied, null);
  await lifecycle.getByRole("button", { name: "Use on live traffic", exact: true }).click();
  await overlaps.waitFor();
  await overlaps.getByRole("checkbox").check();
  await overlaps.getByRole("button", { name: "Use on live traffic", exact: true }).click();
  await settled();
  assert.equal((await status()).autoresponsesEnabled, false);
  await lifecycle.getByRole("button", { name: "Pause", exact: true }).click();
  await settled();
  assert.equal(await stateText(), "Paused");
  await lifecycle.getByRole("button", { name: "Resume", exact: true }).click();
  await settled();
  assert.equal(await stateText(), "Active on live traffic");
  // Named versions freeze saved current, excluding drafts, and can be inspected.
  await manager
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Unsaved notes");
  assert.equal(
    await lifecycle.getByRole("button", { name: "Pause", exact: true }).isEnabled(),
    true,
  );
  await more("Versions…");
  await versions.getByLabel("Version name", { exact: true }).fill("Known good checkout");
  await versions.getByRole("button", { name: "Save named version…", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].versions[0].name, "Known good checkout");
  assert.equal((await status()).responseSets[0].notes, "A saved debugging task");
  await versions.getByRole("button", { name: "View version", exact: true }).click();
  const view = lifecycle.locator('[aria-labelledby="set-version-view-title"]');
  await view.waitFor();
  await view.getByText("HTTP 201 · application/json · 22 saved bytes", { exact: true }).waitFor();
  assert.equal(
    await view.locator(".version-response-body").textContent(),
    '{"checkout":"success"}',
  );
  assert.equal(
    await view.locator(".version-response-headers").textContent(),
    "Content-Type: application/json",
  );
  assert.equal(
    await view.getByRole("button", { name: "Checkout succeeds", exact: true }).isVisible(),
    true,
  );
  await page.screenshot({ path: resolve(evidence, "version-readable-1280.png") });
  await page.setViewportSize({ width: 800, height: 600 });
  await page.screenshot({ path: resolve(evidence, "version-readable-800.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.keyboard.press("Escape");
  assert.equal(
    await versions
      .getByRole("button", { name: "View version", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await versions.getByRole("button", { name: "Close", exact: true }).click();
  assert.equal(
    await manager
      .locator(".response-set-current")
      .getByLabel("Notes", { exact: true })
      .inputValue(),
    "Unsaved notes",
  );
  assert.equal(
    await lifecycle.locator(".set-live-undo").isVisible(),
    false,
    "A version save left stale Undo visible",
  );
  await manager.getByRole("button", { name: "Save changes", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.responseSets.busy,
  );
  assert.equal(await stateText(), "Active · Saved changes not applied");
  assert.equal(
    await lifecycle.getByRole("button", { name: "Apply changes", exact: true }).isVisible(),
    true,
  );
  await lifecycle.getByRole("button", { name: "Pause", exact: true }).click();
  await settled();
  assert.equal(await stateText(), "Paused · Saved changes not applied");
  assert.equal(
    await lifecycle.getByRole("button", { name: "Apply and resume", exact: true }).isVisible(),
    true,
  );
  await lifecycle.getByRole("button", { name: "Apply and resume", exact: true }).click();
  await overlaps.waitFor();
  await overlaps.getByRole("checkbox").check();
  await overlaps.getByRole("button", { name: "Apply and resume", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].paused, false);
  assert.equal((await status()).autoresponsesEnabled, false);
  assert.match(
    await lifecycle.locator(".set-live-undo").textContent(),
    /Restore saved revision 1.*Checkout succeeds.*paused/s,
  );
  await lifecycle.getByRole("button", { name: "Undo live change", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].paused, true);
  assert.equal((await status()).responseSets[0].applied.revision, 1);
  // Version restore resolves a dirty boundary and leaves live responses untouched.
  await manager
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Keep this unfinished draft");
  await more("Versions…");
  await versions.getByRole("button", { name: "Restore to current set…", exact: true }).click();
  await confirmation.getByRole("button", { name: "Restore to current set", exact: true }).click();
  const unsaved = page.locator("automation-workspace .unsaved-rule-dialog");
  await unsaved.waitFor();
  await unsaved.getByRole("button", { name: "Keep editing", exact: true }).click();
  await settled();
  assert.equal(await confirmation.isVisible(), true);
  assert.equal(
    await manager
      .locator(".response-set-current")
      .getByLabel("Notes", { exact: true })
      .inputValue(),
    "Keep this unfinished draft",
  );
  const applied = structuredClone((await status()).responseSets[0].applied);
  await confirmation.getByRole("button", { name: "Restore to current set", exact: true }).click();
  await unsaved.waitFor();
  await unsaved.getByRole("button", { name: "Discard changes", exact: true }).click();
  await settled();
  assert.deepEqual((await status()).responseSets[0].applied, applied);
  assert.equal(
    await manager
      .locator(".response-set-current")
      .getByLabel("Notes", { exact: true })
      .inputValue(),
    "Unsaved notes",
  );
  if (await confirmation.isVisible())
    await confirmation.getByRole("button", { name: "Close", exact: true }).click();
  // Closing a submitted restore does not replace a newer visible draft when it completes.
  await versions.getByRole("button", { name: "Restore to current set…", exact: true }).click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "change_response_set";
    globalThis.__workspaceFixture.finishHeldCommand = null;
  });
  await confirmation.getByRole("button", { name: "Restore to current set", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
  await versions.getByRole("button", { name: "Close", exact: true }).click();
  await manager
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Typed after closing pending restore");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await settled();
  assert.equal(
    await manager
      .locator(".response-set-current")
      .getByLabel("Notes", { exact: true })
      .inputValue(),
    "Typed after closing pending restore",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    true,
  );
  assert.deepEqual((await status()).responseSets[0].applied, applied);
  await manager.getByRole("button", { name: "Discard changes", exact: true }).click();
  await more("Versions…");
  // A closed read-only inspector rejects late response data and leaves focus visible.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "inspect_response_asset";
    globalThis.__workspaceFixture.finishHeldCommand = null;
  });
  await versions.getByRole("button", { name: "View version", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await versions.getByRole("button", { name: "Close", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForTimeout(30);
  assert.equal(await view.isVisible(), false);
  assert.equal(
    await lifecycle
      .getByRole("button", { name: "More…", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
  );
  await more("Versions…");
  // Removing a version detaches its invoking row. Keep focus within the surviving modal.
  const versionName = versions.getByLabel("Version name", { exact: true }),
    assertVersionFocus = async () => {
      assert.equal(await versions.isVisible(), true);
      assert.equal(
        await versionName.evaluate((node) => node.getRootNode().activeElement === node),
        true,
        "Deleting a version lost focus outside the still-open Versions modal",
      );
    };
  await versions.getByRole("button", { name: "Delete version…", exact: true }).click();
  await confirmation.getByRole("button", { name: "Delete version", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].versions.length, 0);
  assert.deepEqual((await status()).responseSets[0].applied, applied);
  assert.equal(await confirmation.isVisible(), false);
  await assertVersionFocus();
  await page.screenshot({ path: resolve(evidence, "delete-last-version-focus.png") });
  for (const name of ["Milestone A", "Milestone B", "Milestone C"]) {
    await versionName.fill(name);
    await versions.getByRole("button", { name: "Save named version…", exact: true }).click();
    await settled();
  }
  const versionRow = (name) =>
    versions.locator(".set-versions-list article").filter({ hasText: name });
  await versionRow("Milestone A")
    .getByRole("button", { name: "Delete version…", exact: true })
    .click();
  await confirmation.getByRole("button", { name: "Delete version", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].versions.length, 2);
  await assertVersionFocus();
  // A submitted delete must not close the dialog or move focus away from newer input.
  await versionRow("Milestone B")
    .getByRole("button", { name: "Delete version…", exact: true })
    .click();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "change_response_set";
    globalThis.__workspaceFixture.finishHeldCommand = null;
  });
  await confirmation.getByRole("button", { name: "Delete version", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await confirmation.getByRole("button", { name: "Cancel", exact: true }).focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await settled();
  assert.equal(await confirmation.isVisible(), true);
  assert.equal(
    await confirmation
      .getByRole("button", { name: "Close", exact: true })
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Delayed deletion displaced newer dialog focus",
  );
  assert.equal((await status()).responseSets[0].versions.length, 1);
  await confirmation.getByRole("button", { name: "Close", exact: true }).click();
  await assertVersionFocus();
  await versionRow("Milestone C")
    .getByRole("button", { name: "Delete version…", exact: true })
    .click();
  await confirmation.getByRole("button", { name: "Delete version", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets[0].versions.length, 0);
  await assertVersionFocus();
  await versions.getByRole("button", { name: "Close", exact: true }).click();
  // Opening the explicit live URL uses the native command without altering live state.
  await lifecycle.getByRole("button", { name: "Try changes…", exact: true }).click();
  await lifecycle.getByRole("button", { name: "Open live page…", exact: true }).click();
  await settled();
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.livePageArgs), {
    id: "lifecycle-set",
    incarnation: "lifecycle-owner",
    url: "https://example.test/checkout",
  });
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({ path: resolve(evidence, `lifecycle-${width}.png`) });
    assert.equal(
      await lifecycle.getByRole("button", { name: "More…", exact: true }).isVisible(),
      true,
    );
  }
  // A delayed overlap review cannot take focus after the user moves it.
  await page.evaluate(() => {
    globalThis.__workspaceFixture.holdCommand = "response_set_overlaps";
    globalThis.__workspaceFixture.finishHeldCommand = null;
  });
  await lifecycle.getByRole("button", { name: "Apply and resume", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  const notes = manager.locator(".response-set-current").getByLabel("Notes", { exact: true });
  await notes.focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await settled();
  assert.equal(await overlaps.isVisible(), false);
  assert.equal(await notes.evaluate((node) => node.getRootNode().activeElement === node), true);
  // Failure keeps the selected set and retryable deletion dialog; successful deletion preserves unrelated content.
  await more("Delete response set…");
  await page.evaluate(() => (globalThis.__workspaceFixture.setReject = true));
  await confirmation.getByRole("button", { name: "Delete response set", exact: true }).click();
  await settled();
  assert.match(await confirmation.textContent(), /Fixture response set save failed/);
  assert.equal((await status()).responseSets.length, 2);
  await page.evaluate(() => (globalThis.__workspaceFixture.setReject = false));
  await confirmation.getByRole("button", { name: "Delete response set", exact: true }).click();
  await settled();
  assert.equal((await status()).responseSets.length, 1);
  assert.equal((await status()).responseSets[0].id, "unrelated-set");
  assert.deepEqual((await status()).rules, baseline.rules);
  assert.equal((await status()).autoresponsesEnabled, false);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.collectionOpen),
    false,
  );
}
