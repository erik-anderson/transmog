import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkResponseSetCollectionAdds(page, root) {
  const evidence = resolve(root, "../../../target/response-set-workspaces/phase3");
  await mkdir(evidence, { recursive: true });
  const workspace = page.locator("automation-workspace"),
    sets = workspace.locator("response-set-workspace"),
    current = sets.locator(".response-set-current"),
    editor = workspace.locator(".auto-response-editor");
  const edit = async (name) => {
    await sets.getByRole("button", { name, exact: true }).click();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.loadingSavedResponse,
    );
  };
  const results = [];
  for (const source of ["scratch", "file"]) {
    await page.goto("https://workspace.test/");
    await page.getByRole("link", { name: /Automation/ }).click();
    await page.waitForFunction(
      () => !!document.querySelector("app-shell").automation?.automationStatus,
    );
    await page.evaluate(async () => {
      const state = globalThis.__workspaceFixture,
        w = document.querySelector("app-shell").automation,
        action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
      const rules = ["A", "B"].map((label, index) => ({
        id: "retained-" + label,
        displayName: "Saved " + label,
        enabled: !index,
        revision: 1,
        priority: -1000000 + index,
        matcher: {
          method: "GET",
          url: { kind: "exact", value: "https://adds.invalid/" + label },
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
          responseAsset: "retained-body-" + label + "@1",
          allowNonIdempotentBodyReplacement: false,
        },
        response: { ...action },
      }));
      state.assets = ["A", "B"].map((label) => ({
        id: "retained-body-" + label,
        revision: 1,
        status: 200,
        headers: [],
        bodyBytes: 12,
        sha256: "a".repeat(64),
        mediaType: "text/plain",
        provenance: { kind: "authored" },
        display: "Saved body " + label,
        rawBody: [...new TextEncoder().encode("Saved body " + label)],
        linkedBody: null,
        copiedFrom: null,
      }));
      state.automation = {
        generation: 10,
        autoresponsesEnabled: true,
        rules: [],
        responseSets: [
          {
            id: "retained-set",
            incarnation: "retained-owner",
            name: "Saved collection",
            notes: "Saved notes",
            revision: 3,
            modifiedAt: Date.now(),
            rules,
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
      state.bodyFile = "C:/fixtures/retained addition.txt";
      state.bodyFileText = "File addition body";
      state.bodyFileBytes = 18;
      w.onVariantStatus(
        new CustomEvent("set-status", { detail: structuredClone(state.automation) }),
      );
      await w.openResponseSet(new CustomEvent("set-open", { detail: { id: "retained-set" } }));
    });
    for (const label of ["A", "B"]) {
      await edit("Saved " + label);
      await editor.getByLabel("Response name", { exact: true }).fill("Dirty " + label);
      await editor.locator('[name="body"]').fill("Dirty body " + label);
    }
    await current
      .getByLabel("Response set name", { exact: true })
      .fill("Dirty collection " + source);
    await current.getByLabel("Notes", { exact: true }).fill("Keep both response drafts " + source);
    await sets.getByRole("button", { name: "Add responses…", exact: true }).click();
    await sets
      .locator("#set-add-menu")
      .getByRole("button", {
        name: source === "scratch" ? "Create from scratch…" : "From file…",
        exact: true,
      })
      .click();
    await page.waitForFunction(() => {
      const w = document.querySelector("app-shell").automation;
      return !w.collectionAdding && !w.fileChoiceBusy;
    });
    const owner = await page.evaluate(() => {
      const w = document.querySelector("app-shell").automation;
      return {
        open: w.collectionOpen,
        id: w.responseSets.setId,
        selection: w.responseSets.currentSelection(),
        count: w.responseSets.draftRules.length,
        dirty: w.responseSets.dirty,
        snapshots: [...w.collectionSnapshots.keys()],
      };
    });
    assert.equal(owner.open, true);
    assert.equal(owner.id, "retained-set");
    assert.equal(owner.selection.incarnation, "retained-owner");
    assert.equal(owner.selection.revision, 3);
    assert.equal(owner.count, 3);
    assert.equal(owner.dirty, true);
    assert.ok(owner.snapshots.includes("retained-A"));
    assert.ok(owner.snapshots.includes("retained-B"));
    assert.equal(
      await current.getByLabel("Response set name", { exact: true }).inputValue(),
      "Dirty collection " + source,
    );
    assert.equal(
      await current.getByLabel("Notes", { exact: true }).inputValue(),
      "Keep both response drafts " + source,
    );
    const newName = "New " + source;
    await editor.getByLabel("Response name", { exact: true }).fill(newName);
    await editor
      .getByLabel("Matching address", { exact: true })
      .fill("https://adds.invalid/" + source);
    if (source === "scratch") await editor.locator('[name="body"]').fill("Scratch addition body");
    else assert.equal(await editor.getByLabel("File use", { exact: true }).inputValue(), "copy");
    for (const label of ["A", "B"]) {
      await edit("Dirty " + label);
      assert.equal(
        await editor.getByLabel("Response name", { exact: true }).inputValue(),
        "Dirty " + label,
      );
      assert.equal(await editor.locator('[name="body"]').inputValue(), "Dirty body " + label);
    }
    await edit(newName);
    if (source === "scratch")
      assert.equal(await editor.locator('[name="body"]').inputValue(), "Scratch addition body");
    else assert.equal(await editor.getByLabel("File use", { exact: true }).inputValue(), "copy");
    await sets.getByRole("button", { name: "Save changes", exact: true }).click();
    await page.waitForFunction(() => {
      const w = document.querySelector("app-shell").automation;
      return !w.responseSets.busy && !w.loadingSavedResponse;
    });
    const saved = await page.evaluate(() => {
      const state = globalThis.__workspaceFixture,
        set = state.automation.responseSets[0];
      return {
        name: set.name,
        notes: set.notes,
        applied: set.applied,
        rules: set.rules.map((rule) => ({
          id: rule.id,
          name: rule.displayName,
          included: rule.enabled,
          asset: state.assets.find(
            (asset) => asset.id + "@" + asset.revision === rule.request.responseAsset,
          ),
        })),
        dirty: document.querySelector("app-shell").automation.responseSets.dirty,
      };
    });
    assert.equal(saved.name, "Dirty collection " + source);
    assert.equal(saved.notes, "Keep both response drafts " + source);
    assert.equal(saved.applied, null);
    assert.equal(saved.dirty, false);
    assert.deepEqual(
      saved.rules
        .slice(0, 2)
        .map((rule) => [rule.id, rule.name, rule.asset.display, rule.included]),
      [
        ["retained-A", "Dirty A", "Dirty body A", true],
        ["retained-B", "Dirty B", "Dirty body B", false],
      ],
    );
    assert.equal(saved.rules[2].name, newName);
    assert.equal(
      saved.rules[2].asset.display,
      source === "scratch" ? "Scratch addition body" : "File addition body",
    );
    if (source === "file") {
      assert.equal(saved.rules[2].asset.copiedFrom, "C:/fixtures/retained addition.txt");
      assert.equal(saved.rules[2].asset.linkedBody, null);
    }
    results.push({ source, owner, saved });
  }
  await page.screenshot({ path: resolve(evidence, "round3-collection-adds.png") });
  await writeFile(
    resolve(evidence, "round3-collection-adds-results.json"),
    JSON.stringify({ results }, null, 2),
  );
}
