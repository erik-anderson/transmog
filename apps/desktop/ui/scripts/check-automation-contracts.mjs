import assert from "node:assert/strict";
import { resolve } from "node:path";
import { checkUnifiedRules } from "./check-unified-rules.mjs";

export async function checkAutomationContracts(page, root) {
  await checkUnifiedRules(page, root);
  await page.setViewportSize({ width: 1440, height: 900 });
  const workspace = page.locator("automation-workspace"),
    grid = workspace.getByRole("grid", { name: "Traffic rules" });
  const headers = workspace.locator('header-rules[editor-kind="headers"]');
  const saved = workspace.locator(".auto-response-editor"),
    browser = workspace.locator("user-agent-overrides");
  const idle = () =>
    page.waitForFunction(
      () => !document.querySelector("app-shell").automation.changingAutoResponse,
    );
  const current = (name) =>
    page.evaluate(
      (name) =>
        structuredClone(
          globalThis.__workspaceFixture.automation.rules.find((rule) => rule.displayName === name),
        ),
      name,
    );
  const row = (name) =>
    grid.locator("tr[data-rule-id]").filter({ has: page.getByText(name, { exact: true }) });
  const choose = async (name) => {
    await row(name).click();
  };
  await choose("Outbound headers");
  await headers.getByRole("button", { name: "Discard changes", exact: true }).click();
  await checkRuleSelectionRaces(page);
  await checkHeaderDraftRaces(page);
  const name = headers.getByLabel("Rule name", { exact: true }),
    save = headers.getByRole("button", { name: "Save rule", exact: true });
  assert.equal(await save.isDisabled(), true);
  await name.fill("Changed name");
  assert.equal(await save.isEnabled(), true);
  await name.fill("Outbound headers");
  assert.equal(
    await save.isDisabled(),
    true,
    "Returning to saved content should clear dirty state",
  );
  await name.fill("Unsaved header");
  await row("Outbound headers")
    .getByRole("button", { name: "Disable Outbound headers", exact: true })
    .click();
  await idle();
  assert.equal(await name.inputValue(), "Unsaved header");
  assert.equal((await current("Outbound headers")).enabled, false);
  assert.equal(await save.isEnabled(), true, "Activation discarded the draft");
  await workspace
    .locator(".auto-response-undo")
    .getByRole("button", { name: "Undo", exact: true })
    .click();
  await idle();
  assert.equal((await current("Outbound headers")).enabled, true);
  assert.equal(await name.inputValue(), "Unsaved header");
  await page.evaluate(() => (globalThis.__workspaceFixture.uaRejectValidation = true));
  await row("Outbound headers")
    .getByRole("button", { name: "Disable Outbound headers", exact: true })
    .click();
  await idle();
  assert.equal((await current("Outbound headers")).enabled, true);
  assert.equal(await workspace.locator(".automation-feedback").isVisible(), true);
  assert.match(await workspace.locator(".automation-feedback").innerText(), /Rule change failed/);
  await page.evaluate(() => (globalThis.__workspaceFixture.uaRejectValidation = false));
  await workspace.locator(".automation-feedback").getByRole("button", { name: "Dismiss" }).click();
  await headers.getByRole("button", { name: "Discard changes", exact: true }).click();

  await choose("Offline response");
  const body = saved.locator('[name="body"]'),
    saveResponse = saved.getByRole("button", { name: "Save rule", exact: true });
  assert.equal(await saveResponse.isDisabled(), true);
  await body.fill("Unsaved response");
  assert.equal(await saveResponse.isEnabled(), true);
  await row("Offline response")
    .getByRole("button", { name: "Disable Offline response", exact: true })
    .click();
  await idle();
  assert.equal(await body.inputValue(), "Unsaved response");
  await saved.getByRole("button", { name: "Discard changes", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.loadingSavedResponse,
  );
  assert.equal(await body.inputValue(), "");
  assert.equal(await saveResponse.isDisabled(), true);
  await body.fill("Saved response");
  await saveResponse.click();
  await page.waitForFunction(() => {
    const w = document.querySelector("app-shell").automation;
    return !w.savingAutoResponse && !w.loadingSavedResponse && !w.draftDirty;
  });
  assert.equal(await saveResponse.isDisabled(), true);
  const revision = (await current("Offline response")).revision;
  await body.focus();
  await page.keyboard.press("Control+s");
  assert.equal(
    (await current("Offline response")).revision,
    revision,
    "Saving unchanged content created another revision",
  );

  await choose("Browser identity");
  const uaName = browser.locator('[name="uaName"]'),
    uaSave = browser.getByRole("button", { name: "Save rule", exact: true });
  assert.equal(await uaSave.isDisabled(), true);
  await uaName.fill("Unsaved browser");
  await row("Browser identity")
    .getByRole("button", { name: "Enable Browser identity", exact: true })
    .click();
  await idle();
  assert.equal(await uaName.inputValue(), "Unsaved browser");
  assert.equal(await uaSave.isEnabled(), true);
  await browser.getByRole("button", { name: "Discard changes", exact: true }).click();
  assert.equal(await uaName.inputValue(), "Browser identity");
  assert.equal(await uaSave.isDisabled(), true);
  const inspections = await page.evaluate(
    () => globalThis.__workspaceFixture.calls.inspect_response_asset,
  );
  await choose("Offline response");
  assert.equal(await body.inputValue(), "Saved response");
  assert.equal(
    await page.evaluate(() => globalThis.__workspaceFixture.calls.inspect_response_asset),
    inspections,
    "Resuming a mounted saved-response editor reloaded its draft",
  );

  await choose("Outbound headers");
  const navigation = page.getByRole("button", { name: "Toggle navigation labels", exact: true });
  const originalExpanded = await navigation.getAttribute("aria-expanded");
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    for (const expanded of ["true", "false"]) {
      if ((await navigation.getAttribute("aria-expanded")) !== expanded) await navigation.click();
      const bounds = await save.boundingBox();
      assert.ok(
        bounds && bounds.y >= 0 && bounds.y + bounds.height <= height,
        "Save must remain visible without scrolling",
      );
      assert.equal(
        await save.evaluate((button) => {
          const box = button.getBoundingClientRect(),
            root = button.getRootNode();
          return [box.top + 2, box.bottom - 2].every((y) =>
            button.contains(root.elementFromPoint(box.left + box.width / 2, y)),
          );
        }),
        true,
        `Save is clipped at ${width}×${height} with navigation expanded=${expanded}`,
      );
      await page.screenshot({
        path: resolve(
          root,
          `../../../target/ui-check/automation-contracts-${width}-${expanded === "true" ? "expanded" : "collapsed"}.png`,
        ),
      });
    }
  }
  if ((await navigation.getAttribute("aria-expanded")) !== originalExpanded)
    await navigation.click();
  await page.setViewportSize({ width: 1440, height: 900 });
  await checkUnsavedDialogs(page, workspace, choose, root);
}

async function checkRuleSelectionRaces(page) {
  const result = await page.evaluate(async () => {
    const workspace = document.querySelector("app-shell").automation,
      state = globalThis.__workspaceFixture,
      saved = state.automation.rules.find((rule) => rule.request.responseAsset),
      target = state.automation.rules.find((rule) => rule.displayName === "Outbound headers"),
      delayed = {
        ...structuredClone(saved),
        id: "delayed-selection",
        displayName: "Delayed response",
      };
    state.automation.rules.push(delayed);
    workspace.renderAutomation(structuredClone(state.automation));
    state.deferAssetInspection = true;
    globalThis.__olderRuleSelection = workspace.selectRule(delayed);
    return {
      delayed: delayed.id,
      target: target.id,
      saved: saved.id,
      inbound: state.automation.rules.find((rule) => rule.displayName === "Inbound headers").id,
    };
  });
  try {
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseAssetInspection);
    const grid = page.getByRole("grid", { name: "Traffic rules", exact: true });
    await grid.locator(`tr[data-rule-id="${result.target}"]`).click();
    await page.waitForFunction(
      (id) => document.querySelector("app-shell").automation.catalogSelectedId === id,
      result.target,
    );
    const selection = await page.evaluate(async () => {
      const state = globalThis.__workspaceFixture;
      state.releaseAssetInspection();
      delete state.releaseAssetInspection;
      await globalThis.__olderRuleSelection;
      delete globalThis.__olderRuleSelection;
      const workspace = document.querySelector("app-shell").automation;
      return {
        ids: [...workspace.ruleSelection.ids],
        focused: workspace.ruleSelection.focused,
        active: workspace.getRootNode().activeElement?.closest("[data-rule-id]")?.dataset.ruleId,
      };
    });
    assert.deepEqual(selection.ids, [result.target], "An older load replaced the newer selection");
    assert.equal(selection.focused, result.target, "An older load restored its list focus");
    assert.equal(selection.active, result.target, "An older load moved keyboard focus back");
    const preparedSelection = await page.evaluate(async (ids) => {
      const w = document.querySelector("app-shell").automation,
        headers = w.matchTestHeaders,
        replace = headers.replaceValue;
      let release,
        entered,
        paused = false;
      const gate = new Promise((resolve) => (release = resolve)),
        waiting = new Promise((resolve) => (entered = resolve));
      headers.replaceValue = async (...args) => {
        await replace.apply(headers, args);
        if (!paused) {
          paused = true;
          entered();
          await gate;
        }
      };
      const older = w.selectRule(
        globalThis.__workspaceFixture.automation.rules.find((rule) => rule.id === ids.saved),
      );
      try {
        await waiting;
        await w.selectRule(
          globalThis.__workspaceFixture.automation.rules.find((rule) => rule.id === ids.inbound),
        );
        release();
        await older;
        return [...w.ruleSelection.ids];
      } finally {
        release();
        await older;
        headers.replaceValue = replace;
      }
    }, result);
    assert.deepEqual(
      preparedSelection,
      [result.inbound],
      "An older editor preparation replaced the newer native selection",
    );
    await grid.locator(`tr[data-rule-id="${result.target}"]`).click();
    await checkRuleSelectionSiblings(page, result);
  } finally {
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      state.automation.rules = state.automation.rules.filter(
        (rule) => rule.id !== "delayed-selection",
      );
      document
        .querySelector("app-shell")
        .automation.renderAutomation(structuredClone(state.automation));
    });
  }
}

async function checkRuleSelectionSiblings(page, ids) {
  const grid = page.getByRole("grid", { name: "Traffic rules", exact: true }),
    row = (id) => grid.locator(`tr[data-rule-id="${id}"]`),
    snapshot = () =>
      page.evaluate(() => {
        const w = document.querySelector("app-shell").automation,
          active = w.getRootNode().activeElement;
        return {
          ids: [...w.ruleSelection.ids],
          focused: w.ruleSelection.focused,
          active: active?.closest("[data-rule-id]")?.dataset.ruleId,
          activeLabel: active?.getAttribute("aria-label"),
        };
      });
  for (const newer of ["saved", "search", "all", "keyboard", "menu-cancel"]) {
    await page.evaluate(
      async ({ ids, newer }) => {
        const w = document.querySelector("app-shell").automation,
          state = globalThis.__workspaceFixture;
        await w.selectRule(state.automation.rules.find((rule) => rule.id === ids.saved));
        state.deferAssetInspection = true;
        const rule = state.automation.rules.find((rule) => rule.id === ids.delayed);
        globalThis.__olderRuleSelection =
          newer === "menu-cancel"
            ? w.openRuleMenu(
                rule,
                new MouseEvent("contextmenu", { cancelable: true, clientX: 100, clientY: 100 }),
              )
            : w.selectRule(rule);
      },
      { ids, newer },
    );
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.releaseAssetInspection);
    let target = ids.delayed;
    if (newer === "saved") {
      target = ids.saved;
      await row(target).click();
      await page.waitForFunction(
        () => !document.querySelector("app-shell").automation.loadingSavedResponse,
      );
    } else if (newer === "search") {
      await page.getByLabel("Search rules", { exact: true }).focus();
    } else if (newer === "all") {
      await grid.getByRole("checkbox", { name: "Select all visible rules", exact: true }).check();
    } else if (newer === "menu-cancel") {
      await page.keyboard.press("Escape");
    } else {
      target = await page.evaluate((id) => {
        const order = document.querySelector("app-shell").automation.ruleOrder();
        return order[order.indexOf(id) - 1];
      }, ids.delayed);
      await row(ids.delayed).focus();
      await page.keyboard.press("ArrowUp");
      await page.waitForFunction(
        (id) => document.querySelector("app-shell").automation.ruleSelection.focused === id,
        target,
      );
    }
    await page.evaluate(async () => {
      const state = globalThis.__workspaceFixture;
      state.releaseAssetInspection();
      delete state.releaseAssetInspection;
      await globalThis.__olderRuleSelection;
      delete globalThis.__olderRuleSelection;
    });
    const current = await snapshot();
    if (newer === "menu-cancel") {
      assert.equal(
        await page.evaluate(() =>
          document.querySelector("app-shell").automation.ruleMenu.matches(":popover-open"),
        ),
        false,
        "Escape during selection reopened the rule menu",
      );
    } else if (newer === "all") {
      assert.equal(current.ids.length, 6, "An older load replaced Select All");
      assert.equal(current.activeLabel, "Select all visible rules");
    } else {
      assert.deepEqual(current.ids, [target], `${newer}: an older load replaced selection`);
      assert.equal(current.focused, target, `${newer}: an older load restored list focus`);
      if (newer === "search") assert.equal(current.activeLabel, "Search rules");
      else assert.equal(current.active, target, `${newer}: an older load moved keyboard focus`);
    }
  }

  // A stale native child must not roll its parent back over a newer saved response.
  await page.evaluate((ids) => {
    const w = document.querySelector("app-shell").automation,
      rendered = w.$whenRendered;
    let release,
      paused = false;
    const gate = new Promise((resolve) => (release = resolve));
    w.$whenRendered = async () => {
      await rendered.call(w);
      if (!paused && w.automationSection === "header-rules") {
        paused = true;
        globalThis.__releaseRuleSelection = release;
        await gate;
      }
    };
    globalThis.__olderRuleSelection = w
      .selectRule(
        globalThis.__workspaceFixture.automation.rules.find((rule) => rule.id === ids.inbound),
      )
      .finally(() => {
        w.$whenRendered = rendered;
      });
  }, ids);
  await page.waitForFunction(() => !!globalThis.__releaseRuleSelection);
  await row(ids.saved).click();
  await page.waitForFunction(
    (id) => document.querySelector("app-shell").automation.editingAutoResponseId === id,
    ids.saved,
  );
  await page.evaluate(async () => {
    globalThis.__releaseRuleSelection();
    await globalThis.__olderRuleSelection;
    delete globalThis.__releaseRuleSelection;
    delete globalThis.__olderRuleSelection;
  });
  assert.deepEqual(
    (await snapshot()).ids,
    [ids.saved],
    "A stale child rolled back its parent selection",
  );

  // Both requests share one leave decision; only the latest may apply it.
  const baseline = await page.locator('.auto-response-editor [name="body"]').inputValue();
  await page.locator('.auto-response-editor [name="body"]').fill("Pending response draft");
  await page.waitForFunction(() => document.querySelector("app-shell").automation.draftDirty);
  await page.evaluate((ids) => {
    const w = document.querySelector("app-shell").automation,
      rules = globalThis.__workspaceFixture.automation.rules;
    globalThis.__ruleSelectionRequests = Promise.all([
      w.selectRule(rules.find((rule) => rule.id === ids.target)),
      w.selectRule(rules.find((rule) => rule.id === ids.inbound)),
    ]);
  }, ids);
  await page
    .locator(".unsaved-rule-dialog")
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await page.evaluate(async () => {
    await globalThis.__ruleSelectionRequests;
    delete globalThis.__ruleSelectionRequests;
  });
  assert.deepEqual(
    (await snapshot()).ids,
    [ids.inbound],
    "A shared leave decision applied an older selection",
  );
  await row(ids.saved).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.loadingSavedResponse,
  );
  assert.equal(
    await page.locator('.auto-response-editor [name="body"]').inputValue(),
    baseline,
    "Returning to a mounted editor restored a discarded draft",
  );
  await row(ids.target).click();
  const headers = page.locator('header-rules[editor-kind="headers"]'),
    savedBody = page.locator('.auto-response-editor [name="body"]');
  await headers.getByLabel("Rule name", { exact: true }).fill("Pending header draft");
  await row(ids.saved).click();
  await savedBody.fill("Keep after a canceled destination");
  await row(ids.inbound).click();
  await page
    .locator(".unsaved-rule-dialog")
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await headers
    .locator(".header-unsaved")
    .getByRole("button", { name: "Keep editing", exact: true })
    .click();
  await page.waitForFunction(
    (id) => document.querySelector("app-shell").automation.ruleSelection.ids.has(id),
    ids.saved,
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.draftDirty),
    true,
    "A destination that kept its own draft consumed the source discard",
  );
  assert.equal(await savedBody.inputValue(), "Keep after a canceled destination");
  await page.evaluate(async () => {
    const w = document.querySelector("app-shell").automation;
    await w.revertAutoResponse();
    await w.headerRulesEditor.discard();
  });
  await row(ids.target).click();
  await checkBulkSelectionOwnership(page, ids);
}

async function checkBulkSelectionOwnership(page, ids) {
  const search = page.getByLabel("Search rules", { exact: true }),
    grid = page.getByRole("grid", { name: "Traffic rules", exact: true });
  for (const target of [ids.target, ids.saved]) {
    await search.fill("");
    await grid.locator(`tr[data-rule-id="${target}"]`).click();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.loadingSavedResponse,
    );
    for (const action of ["duplicate", "undo-remove"]) {
      if (action === "undo-remove") {
        await search.fill("");
        await grid.locator(`tr[data-rule-id="${target}"]`).click();
        await page.waitForFunction(
          () => !document.querySelector("app-shell").automation.loadingSavedResponse,
        );
        await page.evaluate(() =>
          document.querySelector("app-shell").automation.mutateSelectedRules("remove"),
        );
      }
      await page.evaluate((action) => {
        const w = document.querySelector("app-shell").automation;
        globalThis.__workspaceFixture.holdCommand = "activate_automation";
        globalThis.__pendingRuleMutation =
          action === "duplicate" ? w.mutateSelectedRules("duplicate") : w.undoRemoveAutoResponse();
      }, action);
      await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
      await search.fill(action === "duplicate" ? "Outbound" : "Inbound");
      const selected = await page.evaluate(async () => {
        const state = globalThis.__workspaceFixture;
        state.finishHeldCommand();
        delete state.finishHeldCommand;
        await globalThis.__pendingRuleMutation;
        delete globalThis.__pendingRuleMutation;
        return [...document.querySelector("app-shell").automation.ruleSelection.ids];
      });
      assert.deepEqual(selected, [], `${action}: a late mutation replaced newer search selection`);
      if (action === "duplicate") {
        await page.evaluate(() =>
          document.querySelector("app-shell").automation.undoRemoveAutoResponse(),
        );
      }
    }
  }
  await search.fill("");
  await grid.locator(`tr[data-rule-id="${ids.target}"]`).click();
}

async function checkHeaderDraftRaces(page) {
  const ids = await page.evaluate(() => {
    const w = document.querySelector("app-shell").automation,
      editor = w.headerRulesEditor,
      rendered = editor.$whenRendered;
    let release,
      paused = false;
    const gate = new Promise((resolve) => (release = resolve));
    editor.$whenRendered = async () => {
      await rendered.call(editor);
      if (!paused && !editor.hrId) {
        paused = true;
        globalThis.__releaseHeaderDraft = release;
        await gate;
      }
    };
    globalThis.__olderHeaderDraft = editor.newRule(undefined, "request").finally(() => {
      editor.$whenRendered = rendered;
    });
    const rules = globalThis.__workspaceFixture.automation.rules;
    return {
      inbound: rules.find((rule) => rule.displayName === "Inbound headers").id,
      outbound: rules.find((rule) => rule.displayName === "Outbound headers").id,
    };
  });
  await page.waitForFunction(() => !!globalThis.__releaseHeaderDraft);
  const grid = page.getByRole("grid", { name: "Traffic rules", exact: true });
  await grid.locator(`tr[data-rule-id="${ids.inbound}"]`).click();
  await page.waitForFunction(
    (id) => document.querySelector("app-shell").automation.headerRulesEditor.hrId === id,
    ids.inbound,
  );
  const phase = await page.evaluate(async () => {
    globalThis.__releaseHeaderDraft();
    await globalThis.__olderHeaderDraft;
    delete globalThis.__releaseHeaderDraft;
    delete globalThis.__olderHeaderDraft;
    return document.querySelector("app-shell").automation.headerRulesEditor.hrPhase;
  });
  assert.equal(
    phase,
    "response",
    "A superseded new request draft changed the selected response rule",
  );

  await page.evaluate(() =>
    document.querySelector("app-shell").automation.headerRulesEditor.newRule(undefined, "request"),
  );
  const name = page
      .locator('header-rules[editor-kind="headers"]')
      .getByLabel("Rule name", { exact: true }),
    original = await name.inputValue();
  await name.fill("Temporary request name");
  await name.fill(original);
  assert.equal(
    await page.evaluate(
      () => document.querySelector("app-shell").automation.headerRulesEditor.hrDirty,
    ),
    false,
    "A pristine request draft used a response baseline",
  );
  await grid.locator(`tr[data-rule-id="${ids.outbound}"]`).click();
}

async function checkUnsavedDialogs(page, workspace, choose, root) {
  for (const item of [
    {
      name: "Outbound headers",
      editor: "headerRulesEditor",
      selector: 'header-rules[editor-kind="headers"]',
      field: '[name="headerRuleName"]',
      dialog: 'header-rules[editor-kind="headers"] .header-unsaved',
      command: "change_header_rule",
      rejection: "headerReject",
    },
    {
      name: "Browser identity",
      editor: "userAgentOverrides",
      selector: "user-agent-overrides",
      field: '[name="uaName"]',
      dialog: ".ua-unsaved",
      command: "validate_automation",
      rejection: "uaRejectValidation",
    },
    {
      name: "Offline response",
      editor: null,
      selector: ".auto-response-editor",
      field: '[name="name"]',
      dialog: ".unsaved-rule-dialog",
      command: "validate_automation",
      rejection: "uaRejectValidation",
    },
  ]) {
    await choose(item.name);
    const field = workspace.locator(item.selector).locator(item.field),
      dialog = workspace.locator(item.dialog);
    await field.fill(item.name + " pending");
    await page.evaluate(({ editor, command, rejection }) => {
      const w = document.querySelector("app-shell").automation,
        state = globalThis.__workspaceFixture;
      state.holdCommand = command;
      state[rejection] = true;
      globalThis.__ruleLeaveResult = null;
      const pending = editor ? w[editor].canLeaveDraft() : w.canLeaveEditor();
      void pending.then((result) => (globalThis.__ruleLeaveResult = result));
    }, item);
    await dialog.getByRole("button", { name: "Save and continue", exact: true }).click();
    await page.waitForFunction(
      () => typeof globalThis.__workspaceFixture.finishHeldCommand === "function",
    );
    for (const label of ["Save and continue", "Discard changes", "Keep editing"])
      assert.equal(
        await dialog.getByRole("button", { name: label, exact: true }).isDisabled(),
        true,
        `${item.name}: pending save permits another leave decision`,
      );
    await page.keyboard.press("Escape");
    assert.equal(await dialog.isVisible(), true, `${item.name}: Escape abandons a pending save`);
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      state.finishHeldCommand();
      delete state.finishHeldCommand;
    });
    await dialog
      .getByRole("alert")
      .filter({ hasText: /[Ff]ailed|not (?:be )?saved/ })
      .waitFor();
    assert.equal(await field.inputValue(), item.name + " pending");
    assert.equal(await dialog.isVisible(), true);
    await page.setViewportSize({ width: 800, height: 600 });
    if (item.editor === "userAgentOverrides") await page.emulateMedia({ colorScheme: "dark" });
    if (item.editor === null) await page.emulateMedia({ forcedColors: "active" });
    for (const button of await dialog.getByRole("button").all())
      assert.equal(
        await button.evaluate((node) => {
          const r = node.getBoundingClientRect();
          return (
            r.x >= 0 &&
            r.y >= 0 &&
            r.right <= innerWidth &&
            r.bottom <= innerHeight &&
            node.contains(
              node.getRootNode().elementFromPoint(r.x + r.width / 2, r.y + r.height / 2),
            )
          );
        }),
        true,
        `${item.name}: failed save hides a recovery action`,
      );
    await page.screenshot({
      path: resolve(
        root,
        `../../../target/ui-check/automation-review-${item.editor ?? "saved-response"}-error.png`,
      ),
    });
    await page.emulateMedia({ colorScheme: null, forcedColors: "none" });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(
      (rejection) => (globalThis.__workspaceFixture[rejection] = false),
      item.rejection,
    );
    await dialog.getByRole("button", { name: "Save and continue", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(() => globalThis.__ruleLeaveResult === true);
    await field.fill(item.name);
    await workspace
      .locator(item.selector)
      .getByRole("button", { name: "Save rule", exact: true })
      .click();
    await page.waitForFunction(() => {
      const w = document.querySelector("app-shell").automation;
      return (
        !w.savingAutoResponse &&
        !w.loadingSavedResponse &&
        !w.headerRulesEditor.hrBusy &&
        !w.userAgentOverrides.uaBusy
      );
    });
    if (item.editor !== "userAgentOverrides") {
      await field.fill("");
      await page.evaluate((editor) => {
        const w = document.querySelector("app-shell").automation;
        globalThis.__ruleLeaveResult = null;
        void (editor ? w[editor].canLeaveDraft() : w.canLeaveEditor()).then(
          (result) => (globalThis.__ruleLeaveResult = result),
        );
      }, item.editor);
      await dialog.getByRole("button", { name: "Save and continue", exact: true }).click();
      await dialog.waitFor({ state: "hidden" });
      await page.waitForFunction(() => globalThis.__ruleLeaveResult === false);
      assert.equal(
        await field.evaluate((node) => node.getRootNode().activeElement === node),
        true,
        `${item.name}: invalid input stays behind the modal`,
      );
      await field.fill(item.name);
    }
  }
}
