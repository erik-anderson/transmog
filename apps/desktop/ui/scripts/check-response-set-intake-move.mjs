import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

/** Move owns the selected source identity, so its property draft needs its own boundary. */
export async function checkResponseSetIntakeMove(page, root) {
  const evidence = resolve(root, "../../../target/response-set-workspaces/phase3");
  await mkdir(evidence, { recursive: true });
  const workspace = page.locator("automation-workspace"),
    editor = workspace.locator(".auto-response-editor"),
    picker = page.locator("response-set-intake"),
    dialog = picker.locator(".response-set-intake-dialog"),
    unsaved = workspace.locator(".unsaved-rule-dialog");
  const body = editor.locator('[name="body"]'),
    name = editor.locator('[name="name"]');
  const idle = () =>
    page.waitForFunction(() => {
      const p = document.querySelector("app-shell").responseSetIntake;
      return !p.intakeLoading && !p.intakeReviewBusy && !p.intakeSaving;
    });
  const fixture = () =>
    page.evaluate(() => structuredClone(globalThis.__workspaceFixture.automation));
  const draft = () =>
    page.evaluate(() => {
      const w = document.querySelector("app-shell").automation;
      return {
        id: w.editingAutoResponseId,
        dirty: w.draftDirty,
        hidden: w.editorHidden,
        body: w.autoResponseBody.value,
        name: w.autoResponseForm.elements.namedItem("name").value,
      };
    });
  const setup = async (advanced = false) => {
    await page.goto("https://workspace.test/");
    await page.getByRole("link", { name: /Automation/ }).click();
    await page.waitForFunction(
      () => !!document.querySelector("app-shell").automation?.automationStatus,
    );
    await page.evaluate(async (advanced) => {
      const state = globalThis.__workspaceFixture,
        w = document.querySelector("app-shell").automation,
        action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
      const rule = (id, name, asset, priority) => ({
        id,
        displayName: name,
        enabled: true,
        revision: 1,
        priority,
        matcher: {
          method: "GET",
          url: { kind: "exact", value: "https://move.invalid/" + id },
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
      const source = rule("move-source", "Source rule", "move-source-body", -1000000),
        other = rule("move-other", "Other rule", "move-other-body", -999999);
      if (advanced) {
        const bytes = (text) => [...new TextEncoder().encode(text)];
        source.enabled = false;
        source.matcher = {
          ...source.matcher,
          url: { kind: "exact", value: "https://move.invalid/move-source?phase=one" },
          examples: [
            {
              method: "GET",
              url: "https://move.invalid/move-source?phase=one",
              expected: true,
              headers: [{ name: "X-Author", value: "saved" }],
            },
          ],
          scheme: "https",
          host: "move.invalid",
          port: 443,
          pathPrefix: "/move",
          query: "phase=one",
          requestHeaders: [
            { name: "X-Author", condition: { kind: "exact", value: bytes("saved") } },
            { name: "X-Tenant", condition: { kind: "prefix", value: bytes("team-") } },
            { name: "X-Suffix", condition: { kind: "suffix", value: bytes("-tail") } },
            { name: "X-Required", condition: { kind: "exists" } },
            { name: "X-Pattern", condition: { kind: "regex", value: "^allowed-[0-9]+$" } },
          ],
          responseHeaders: [
            { name: "X-Origin", condition: { kind: "prefix", value: bytes("upstream-") } },
          ],
          responseStatus: 201,
          responseStatusClass: 2,
        };
        source.request = {
          ...source.request,
          headers: [
            { operation: "append", field: { name: bytes("X-Preserved"), value: bytes("request") } },
          ],
          replaceBody: bytes("request bytes"),
          allowNonIdempotentBodyReplacement: true,
          userAgent: {
            template: "chrome-windows",
            versionOverride: "123",
            clientHints: { "Sec-CH-UA-Platform": "Windows" },
            stripClientHints: false,
          },
        };
        source.response = {
          headers: [{ operation: "set", field: { name: bytes("X-Result"), value: bytes("kept") } }],
          replaceBody: bytes("response bytes"),
          discardBody: false,
          abortReason: null,
          textTransform: {
            find: "before",
            replacement: "after",
            occurrences: "first",
            expectedMatches: 1,
            mediaType: "text/plain",
            encoding: "utf-8",
            maxBytes: 4096,
          },
        };
      }
      state.assets = [
        ["move-source-body", "saved source body"],
        ["move-other-body", "other body"],
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
        rules: [source, other],
        responseSets: [
          {
            id: "move-target",
            incarnation: "move-owner",
            name: "Existing destination",
            notes: "Saved notes",
            revision: 1,
            modifiedAt: Date.now(),
            rules: [],
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
      await w.editAutoResponse(source, true);
    }, advanced);
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.loadingSavedResponse,
    );
    await name.fill("Edited source name");
    await body.fill("edited source body");
    assert.equal((await draft()).dirty, true);
  };
  const open = async (destination = "existing", mode = "move") => {
    await workspace
      .locator(".rule-selection-toolbar")
      .getByRole("button", { name: "More", exact: true })
      .click();
    await workspace
      .locator("#rule-actions-menu")
      .getByRole("button", { name: "Add to response set…", exact: true })
      .click();
    await dialog.waitFor({ state: "visible" });
    await idle();
    await dialog
      .getByLabel("Response set", { exact: true })
      .selectOption(destination === "new" ? "__new" : "move-target");
    if (destination === "new")
      await dialog.getByLabel("Response set name", { exact: true }).fill("New destination");
    if (mode === "move") await dialog.getByRole("radio", { name: /Move rules/ }).check();
  };
  const review = () =>
    dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  const choose = async (choice) => {
    await unsaved.waitFor({ state: "visible" });
    await unsaved
      .getByRole("button", {
        name:
          choice === "save"
            ? "Save and continue"
            : choice === "discard"
              ? "Discard changes"
              : "Keep editing",
        exact: true,
      })
      .click();
    await idle();
  };
  const submit = async () => {
    const ack = dialog.getByRole("checkbox", {
      name: "Keep all responses with these overlapping matches",
      exact: true,
    });
    if (await ack.isVisible()) await ack.check();
    await dialog.getByRole("button", { name: "Move 1 rule", exact: true }).click();
    await idle();
  };
  const savedBody = async (rule) =>
    page.evaluate(
      (reference) =>
        globalThis.__workspaceFixture.assets.find(
          (asset) => asset.id + "@" + asset.revision === reference,
        )?.display,
      rule.request.responseAsset,
    );
  const saveDraft = async () => {
    await body.press("Control+s");
    try {
      await page.waitForFunction(() => {
        const w = document.querySelector("app-shell").automation;
        return !w.savingAutoResponse && !w.loadingSavedResponse && !w.draftDirty;
      });
    } catch (error) {
      console.error(
        "Source draft save state",
        await page.evaluate(() => {
          const w = document.querySelector("app-shell").automation;
          return {
            id: w.editingAutoResponseId,
            source: w.autoResponseSourceState,
            error: w.autoResponseError,
            saving: w.savingAutoResponse,
            loading: w.loadingSavedResponse,
            dirty: w.draftDirty,
            valid: w.autoResponseForm.checkValidity(),
            invalid: [...w.autoResponseForm.elements]
              .filter((field) => field.validity && !field.validity.valid)
              .map((field) => ({
                name: field.name,
                value: field.value,
                message: field.validationMessage,
              })),
            openDialogs: [
              ...document.querySelector("app-shell").getRootNode().querySelectorAll("dialog[open]"),
            ].map((node) => node.outerHTML.slice(0, 180)),
          };
        }),
      );
      throw error;
    }
  };
  const results = [];
  for (const destination of ["existing", "new"])
    for (const choice of ["save", "discard", "keep"]) {
      await setup();
      await open(destination);
      await review();
      await choose(choice);
      if (choice === "keep") {
        assert.equal(await dialog.locator(".intake-review").count(), 0);
        assert.deepEqual(await draft(), {
          id: "move-source",
          dirty: true,
          hidden: false,
          body: "edited source body",
          name: "Edited source name",
        });
        assert.equal((await fixture()).rules.length, 2);
        await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
        await saveDraft();
        assert.equal(
          await savedBody((await fixture()).rules.find((rule) => rule.id === "move-source")),
          "edited source body",
        );
      } else {
        assert.equal(await dialog.locator(".intake-review ol > li").count(), 1);
        if (choice === "save") {
          assert.match(await dialog.locator(".intake-review").textContent(), /Edited source name/);
          assert.equal(
            await savedBody((await fixture()).rules.find((rule) => rule.id === "move-source")),
            "edited source body",
          );
        } else
          assert.equal((await draft()).dirty, true, "Discard was applied before Move succeeded");
        await submit();
        const state = await fixture(),
          target = state.responseSets.find(
            (set) =>
              set.name === (destination === "new" ? "New destination" : "Existing destination"),
          );
        assert.equal(
          state.rules.some((rule) => rule.id === "move-source"),
          false,
        );
        assert.equal(target.rules.length, 1);
        assert.equal(
          target.rules[0].displayName,
          choice === "save" ? "Edited source name" : "Source rule",
        );
        assert.equal(
          await savedBody(target.rules[0]),
          choice === "save" ? "edited source body" : "saved source body",
        );
        assert.equal(target.applied, null);
        assert.equal((await draft()).hidden, true);
        assert.equal((await draft()).id, null);
      }
      results.push({ destination, choice, passed: true });
    }
  // Copy does not consume the original draft; moving a different source does not consume it either.
  for (const mode of ["copy", "unrelated move"]) {
    await setup();
    await open("new", mode === "copy" ? "copy" : "move");
    if (mode !== "copy") {
      await dialog.getByRole("checkbox", { name: "Select Source rule", exact: true }).uncheck();
      await dialog.getByRole("checkbox", { name: "Select Other rule", exact: true }).check();
    }
    await review();
    await idle();
    assert.equal(await unsaved.isVisible(), false);
    if (mode === "copy") {
      await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
      await idle();
    } else await submit();
    assert.deepEqual(await draft(), {
      id: "move-source",
      dirty: true,
      hidden: false,
      body: "edited source body",
      name: "Edited source name",
    });
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await saveDraft();
    assert.equal(
      await savedBody((await fixture()).rules.find((rule) => rule.id === "move-source")),
      "edited source body",
    );
    results.push({ mode, draftUntouched: true });
  }
  // Native checkbox Space is additive and reversible while row keyboard ranges still work.
  await setup();
  await open();
  const first = dialog.getByRole("checkbox", { name: "Select Source rule", exact: true }),
    second = dialog.getByRole("checkbox", { name: "Select Other rule", exact: true });
  await second.check();
  await first.focus();
  await first.press("Space");
  assert.equal(await first.isChecked(), false);
  assert.equal(await second.isChecked(), true);
  await first.press("Space");
  assert.equal(await first.isChecked(), true);
  assert.equal(await second.isChecked(), true);
  await second.focus();
  await second.press("Space");
  assert.equal(await first.isChecked(), true);
  assert.equal(await second.isChecked(), false);
  await dialog.locator("#intake-source-move-source").focus();
  await page.keyboard.press("Space");
  await page.keyboard.press("Shift+ArrowDown");
  assert.equal(await first.isChecked(), true);
  assert.equal(await second.isChecked(), true);
  await page.keyboard.press("Escape");
  // Failed Save stays in the prompt; failed/canceled Move keeps the deferred Discard draft.
  await setup();
  await open();
  await review();
  await page.evaluate(() => (globalThis.__workspaceFixture.assetError = true));
  await unsaved.getByRole("button", { name: "Save and continue", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.resolvingUnsaved,
  );
  assert.equal(await unsaved.isVisible(), true);
  assert.match(await unsaved.textContent(), /asset write failed/);
  assert.equal((await draft()).dirty, true);
  await unsaved.getByRole("button", { name: "Keep editing", exact: true }).click();
  await idle();
  await page.keyboard.press("Escape");
  await setup();
  await open();
  await review();
  await choose("discard");
  await page.keyboard.press("Escape");
  assert.equal((await draft()).dirty, true);
  await saveDraft();
  assert.equal(
    await savedBody((await fixture()).rules.find((rule) => rule.id === "move-source")),
    "edited source body",
  );
  await setup();
  await open();
  await review();
  await choose("discard");
  await page.evaluate(() => (globalThis.__workspaceFixture.intakeReject = true));
  await submit();
  assert.match(await dialog.locator(".field-error").textContent(), /save failed/);
  assert.equal((await fixture()).rules.length, 2);
  assert.equal((await draft()).dirty, true);
  assert.equal(await body.inputValue(), "edited source body");
  await page.evaluate(() => (globalThis.__workspaceFixture.intakeReject = false));
  await submit();
  assert.equal((await draft()).hidden, true);
  // New edits during review invalidate permission to remove the source, even with equal dirty state.
  await setup();
  await open();
  await review();
  await choose("discard");
  const calls = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.collect_response_set ?? 0,
  );
  await page.evaluate(() => {
    const w = document.querySelector("app-shell").automation;
    w.autoResponseBody.value = "newer review body";
    w.autoResponseBody.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
  });
  await submit();
  assert.match(
    await dialog.locator(".field-error").textContent(),
    /source rule changed after review/,
  );
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.collect_response_set ?? 0),
    calls,
  );
  assert.equal(await body.inputValue(), "newer review body");
  assert.equal((await fixture()).rules.length, 2);
  // Backend generation changes reject a previously reviewed Move, retaining the source draft.
  await setup();
  await open("new");
  await review();
  await choose("discard");
  await page.evaluate(() => globalThis.__workspaceFixture.automation.generation++);
  await submit();
  assert.match(await dialog.locator(".field-error").textContent(), /changed/);
  assert.equal((await draft()).dirty, true);
  assert.equal((await fixture()).rules.length, 2);
  // A closed pending Move cannot erase later edits or leave Save pointing to a removed identity.
  for (const fails of [false, true]) {
    await setup();
    await open("new");
    await review();
    await choose("discard");
    await page.evaluate(() => (globalThis.__workspaceFixture.holdCommand = "collect_response_set"));
    await dialog.getByRole("button", { name: "Move 1 rule", exact: true }).click();
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
    await page.keyboard.press("Escape");
    await body.fill("newer pending body");
    await name.fill("Newer pending rule");
    await page.evaluate((fails) => {
      globalThis.__workspaceFixture.intakeReject = fails;
      globalThis.__workspaceFixture.finishHeldCommand();
    }, fails);
    await idle();
    assert.equal(await body.inputValue(), "newer pending body");
    assert.equal((await draft()).dirty, true);
    assert.equal((await draft()).id, fails ? "move-source" : null);
    assert.equal((await draft()).hidden, false);
    await page.evaluate(() => (globalThis.__workspaceFixture.intakeReject = false));
    // A failed operation opens the existing operation-error notice; close it before saving.
    const failure = page.getByRole("alertdialog", {
      name: "Responses could not be added",
      exact: true,
    });
    if (await failure.isVisible())
      await failure.getByRole("button", { name: "Close", exact: true }).click();
    await saveDraft();
    const state = await fixture(),
      rule = state.rules.find((rule) => rule.displayName === "Newer pending rule");
    assert.ok(rule);
    assert.equal(await savedBody(rule), "newer pending body");
    assert.equal(rule.id === "move-source", fails);
    results.push({ pendingFailure: fails, newerDraftSaved: true });
  }
  // A detached draft keeps restrictions and actions that guided fields do not expose.
  await setup(true);
  const original = (await fixture()).rules[0];
  await open("new");
  await review();
  await choose("discard");
  await page.evaluate(() => (globalThis.__workspaceFixture.holdCommand = "collect_response_set"));
  await dialog.getByRole("button", { name: "Move 1 rule", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await body.fill("new advanced body");
  await name.fill("Detached full rule");
  await editor.locator('[name="method"]').selectOption("POST");
  await editor
    .getByLabel("Matching address", { exact: true })
    .fill("https://move.invalid/move-edited?phase=one");
  await editor.getByText("Request header conditions", { exact: true }).click();
  await editor.locator('[name="requestHeaders"]').fill("X-Author: newer");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await idle();
  assert.equal((await draft()).id, null);
  await saveDraft();
  const preserved = (await fixture()).rules.find(
      (rule) => rule.displayName === "Detached full rule",
    ),
    expectedMatcher = {
      ...original.matcher,
      method: "POST",
      url: { kind: "exact", value: "https://move.invalid/move-edited?phase=one" },
      requestHeaders: [
        {
          name: "X-Author",
          condition: { kind: "exact", value: [...new TextEncoder().encode("newer")] },
        },
        ...original.matcher.requestHeaders.filter((header) => header.condition.kind !== "exact"),
      ],
    };
  assert.ok(preserved);
  assert.notEqual(preserved.id, original.id);
  assert.equal(preserved.revision, 1);
  assert.equal(preserved.enabled, false);
  assert.equal(await savedBody(preserved), "new advanced body");
  assert.deepEqual(preserved.matcher, expectedMatcher);
  assert.deepEqual(preserved.response, original.response);
  assert.deepEqual(preserved.request, {
    ...original.request,
    responseAsset: preserved.request.responseAsset,
  });
  results.push({ detachedFullFidelity: true, rule: preserved });
  await page.screenshot({ path: resolve(evidence, "round2-newer-source-saved.png") });
  await writeFile(
    resolve(evidence, "round2-move-results.json"),
    JSON.stringify(
      {
        results,
        checkboxSpace: true,
        rowKeyboardRange: true,
        failedSavePreserved: true,
        canceledDiscardPreserved: true,
        failedMovePreserved: true,
        reviewSignatureGuard: true,
        generationRace: true,
        detachedFullFidelity: true,
      },
      null,
      2,
    ),
  );
}
