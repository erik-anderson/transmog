import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

/** Round-one regression: successful collection must advance the mounted editor baseline. */
export async function checkResponseSetIntakeRefresh(page, root) {
  const evidence = resolve(root, "../../../target/response-set-workspaces/phase3");
  await mkdir(evidence, { recursive: true });
  const workspace = page.locator("automation-workspace"),
    sets = workspace.locator("response-set-workspace"),
    editor = workspace.locator(".auto-response-editor"),
    picker = page.locator("response-set-intake"),
    dialog = picker.locator(".response-set-intake-dialog"),
    unsaved = workspace.locator(".unsaved-rule-dialog");
  const idle = () =>
    page.waitForFunction(() => {
      const picker = document.querySelector("app-shell").responseSetIntake;
      return !picker.intakeLoading && !picker.intakeReviewBusy && !picker.intakeSaving;
    });
  const setup = async (label) => {
    await page.goto("https://workspace.test/");
    await page.getByRole("link", { name: /Automation/ }).click();
    await page.waitForFunction(
      () => !!document.querySelector("app-shell").automation?.automationStatus,
    );
    await page.evaluate(async (label) => {
      const state = globalThis.__workspaceFixture,
        w = document.querySelector("app-shell").automation;
      const action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
      const rule = (id, url, asset) => ({
        id,
        displayName: id === "refresh-owned" ? "Selected destination response" : "Source to collect",
        enabled: true,
        revision: 1,
        priority: -1000000,
        matcher: {
          method: "GET",
          url: { kind: "exact", value: url },
          requestHeaders: [],
          responseHeaders: [],
          scheme: null,
          host: null,
          port: null,
          pathPrefix: null,
          query: null,
          responseStatus: null,
          responseStatusClass: null,
        },
        request: {
          ...action,
          responseAsset: asset + "@1",
          allowNonIdempotentBodyReplacement: false,
        },
        response: { ...action },
      });
      const owned = rule(
          "refresh-owned",
          "https://refresh.invalid/destination",
          "refresh-owned-body",
        ),
        source = rule("refresh-source", "https://refresh.invalid/source", "refresh-source-body");
      state.assets = [
        ["refresh-owned-body", "destination body"],
        ["refresh-source-body", "source body"],
      ].map(([id, display]) => ({
        id,
        revision: 1,
        status: 200,
        headers: [],
        bodyBytes: display.length,
        sha256: "a".repeat(64),
        mediaType: "text/plain",
        provenance: { kind: "authored" },
        display,
        rawBody: [...new TextEncoder().encode(display)],
        linkedBody: null,
        copiedFrom: null,
      }));
      state.automation = {
        generation: 10,
        autoresponsesEnabled: true,
        rules: [source],
        responseSets: [
          {
            id: "refresh-target",
            incarnation: "refresh-owner",
            name: "Refresh " + label,
            notes: "Saved notes",
            revision: 1,
            modifiedAt: Date.now(),
            rules: [owned],
            versions: [],
            applied: null,
            undoApply: null,
            paused: false,
            previewPage: null,
            previewPageRevision: 0,
          },
        ],
        diagnostics: [],
        usage: [],
        candidateCount: 0,
        historyCount: 0,
      };
      w.onVariantStatus(
        new CustomEvent("set-status", { detail: structuredClone(state.automation) }),
      );
      await w.openResponseSet(new CustomEvent("set-open", { detail: { id: "refresh-target" } }));
      await w.editCollectionResponse(
        new CustomEvent("set-edit", { detail: { ruleId: "refresh-owned" } }),
      );
    }, label);
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.loadingSavedResponse,
    );
    assert.equal(await editor.locator('[name="body"]').inputValue(), "destination body");
  };
  const review = async () => {
    await sets.getByRole("button", { name: "Add responses…", exact: true }).click();
    await sets
      .locator("#set-add-menu")
      .getByRole("button", { name: "From existing rules…", exact: true })
      .click();
    await dialog.waitFor({ state: "visible" });
    await idle();
    await dialog.getByRole("checkbox", { name: "Select Source to collect", exact: true }).check();
    await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  };
  const results = [];
  for (const choice of ["clean", "save", "discard"]) {
    await setup(choice);
    if (choice !== "clean") {
      await editor.locator('[name="body"]').fill("Prompt draft body");
      await sets
        .locator(".response-set-current")
        .getByLabel("Notes", { exact: true })
        .fill("Prompt draft notes");
    }
    await review();
    if (choice !== "clean") {
      await unsaved.waitFor({ state: "visible" });
      await unsaved
        .getByRole("button", {
          name: choice === "save" ? "Save changes" : "Discard changes",
          exact: true,
        })
        .click();
    }
    await idle();
    await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
    await idle();
    const refreshed = await page.evaluate(() => {
      const w = document.querySelector("app-shell").automation,
        set = globalThis.__workspaceFixture.automation.responseSets[0];
      return {
        expected: {
          experimentId: set.id,
          incarnation: set.incarnation,
          variantId: "",
          revision: set.revision,
        },
        actual: w.responseSets.currentSelection(),
        draftIds: w.responseSets.draftRules.map((rule) => rule.id),
        savedIds: set.rules.map((rule) => rule.id),
        rows: w.responseSets.ruleRows.length,
        selected: w.responseSets.selectedRuleId,
        editorId: w.editingAutoResponseId,
        editorSelection: w.editingVariant,
        dirty: w.responseSets.dirty,
        focusStayed:
          document.querySelector("app-shell").responseSetIntake.closeButton ===
          w.getRootNode().activeElement,
      };
    });
    assert.deepEqual(
      refreshed.actual,
      refreshed.expected,
      choice + " kept a stale pinned revision/incarnation",
    );
    assert.deepEqual(
      refreshed.draftIds,
      refreshed.savedIds,
      choice + " did not render appended responses",
    );
    assert.equal(refreshed.rows, 2);
    assert.equal(refreshed.selected, "refresh-owned");
    assert.equal(refreshed.editorId, "refresh-owned");
    assert.deepEqual(refreshed.editorSelection, refreshed.expected);
    assert.equal(refreshed.dirty, false);
    assert.equal(refreshed.focusStayed, true, "Completion moved focus into the response editor");
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    assert.equal(
      await editor.locator('[name="body"]').inputValue(),
      choice === "save" ? "Prompt draft body" : "destination body",
    );
    const body = "Saved after " + choice;
    await editor.locator('[name="body"]').fill(body);
    await editor.locator('[name="body"]').press("Control+s");
    await page.waitForFunction(() => {
      const w = document.querySelector("app-shell").automation;
      return !w.responseSets.busy && !w.responseSets.dirty && !w.loadingSavedResponse;
    });
    const saved = await page.evaluate(() => {
      const state = globalThis.__workspaceFixture,
        set = state.automation.responseSets[0],
        rule = set.rules.find((rule) => rule.id === "refresh-owned"),
        asset = state.assets.find(
          (asset) => asset.id + "@" + asset.revision === rule.request.responseAsset,
        );
      return {
        count: set.rules.length,
        body: asset.display,
        selected: document.querySelector("app-shell").automation.responseSets.selectedRuleId,
      };
    });
    assert.equal(saved.count, 2, "The next save lost the collected response");
    assert.equal(saved.body, body);
    assert.equal(saved.selected, "refresh-owned");
    results.push({ choice, refreshed, saved });
  }
  // Closing a pending collection allows newer edits; its late reply cannot replace them.
  await setup("newer draft");
  await review();
  await idle();
  await page.evaluate(() => (globalThis.__workspaceFixture.holdCommand = "collect_response_set"));
  await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await editor.locator('[name="body"]').fill("Newer local body");
  await sets
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Newer local notes");
  await editor.locator('[name="body"]').focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await idle();
  assert.equal(await editor.locator('[name="body"]').inputValue(), "Newer local body");
  assert.equal(
    await sets.locator(".response-set-current").getByLabel("Notes", { exact: true }).inputValue(),
    "Newer local notes",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    true,
  );
  assert.equal(
    await page.evaluate(
      () => document.querySelector("app-shell").automation.responseSets.currentSelection().revision,
    ),
    1,
  );
  assert.equal(
    await page.evaluate(
      () => globalThis.__workspaceFixture.automation.responseSets[0].rules.length,
    ),
    2,
  );
  assert.equal(
    await editor
      .locator('[name="body"]')
      .evaluate((node) => node.getRootNode().activeElement === node),
    true,
    "Late collection completion stole editor focus",
  );
  await page.screenshot({ path: resolve(evidence, "round1-newer-draft-preserved.png") });
  await writeFile(
    resolve(evidence, "round1-refresh-results.json"),
    JSON.stringify({ results, newerDraftPreserved: true }, null, 2),
  );
}
