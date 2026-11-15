import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkDebuggerValues(page, workspace, root) {
  const inspector = workspace.getByRole("complementary", { name: "Call stack and variables" });
  const local = inspector
    .locator("debug-value")
    .filter({ has: page.getByRole("tree", { name: "local", exact: true }) });
  const prompt = workspace.getByRole("textbox", { name: "Immediate expression", exact: true });
  const row = (name) =>
    local.locator("tr").filter({ has: page.locator("th").getByText(name, { exact: true }) });
  const longValue = "first line\nsecond line\n" + "long value ".repeat(80);
  const ready = () =>
    page.waitForFunction(() => {
      const element = document
        .querySelector("app-shell")
        .shadowRoot.querySelector("debugger-workspace");
      const local = element.debuggerScopeValues.find((value) => value.label === "local");
      return (
        local &&
        !local.expired &&
        !local.loading &&
        !local.restoring &&
        JSON.parse(local.epoch).pause === globalThis.__workspaceFixture.workspaceDebug.epoch
      );
    });
  await page.evaluate((longValue) => {
    const debug = globalThis.__workspaceFixture.workspaceDebug;
    const property = (name, kind, display, handle = null) => ({
      name,
      value: { kind, display, handle, truncated: false },
    });
    debug.propertyInventory = {
      1: [
        property("value", "number", "42"),
        property("sample", "object", "[Object]", 2),
        property("message", "string", longValue),
        property("secret", "unavailable", "Accessor; not evaluated"),
      ],
      2: [property("answer", "number", "42")],
    };
    debug.pause();
  }, longValue);
  try {
    await ready();
    await row("value").getByText("42", { exact: true }).waitFor();
    await local.locator(".heading").focus();
    await page.keyboard.press("ArrowDown");
    assert.equal(await row("value").evaluate((e) => e.getRootNode().activeElement === e), true);
    await page.keyboard.press("F2");
    const input = local.getByRole("textbox", { name: "New value for value", exact: true });
    assert.equal(await input.inputValue(), "42");
    await input.fill("invalid(");
    await input.press("Enter");
    await local.getByRole("alert").getByText("Invalid value expression", { exact: true }).waitFor();
    assert.equal(
      await input.inputValue(),
      "invalid(",
      "Failed edits preserve the replacement draft",
    );
    await input.press("Escape");
    assert.equal(await input.count(), 0);
    assert.equal(await prompt.isVisible(), true, "Canceling a value edit leaves Console open");
    await row("value").press("F2");
    await input.fill("99");
    await input.press("Enter");
    await row("value").getByText("99", { exact: true }).waitFor();
    await inspector.locator(".watch").getByText("99", { exact: true }).waitFor();
    assert.equal(await row("value").getAttribute("data-changed"), "");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdValueEdit = true;
    });
    await row("value").press("F2");
    await input.fill("100");
    await input.press("Enter");
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishValueEdit,
    );
    await row("message").locator(".property-preview").dblclick();
    assert.equal(await input.inputValue(), "100", "A pending save keeps its original editor");
    assert.equal(
      await local.getByRole("textbox", { name: "New value for message", exact: true }).count(),
      0,
    );
    const outer = inspector.locator(".call-stack button").filter({ hasText: "outer" });
    await outer.click();
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishValueEdit());
    await ready();
    await row("value").getByText("21", { exact: true }).waitFor();
    assert.equal(
      await outer.evaluate((element) => element.getRootNode().activeElement === element),
      true,
      "A late edit reply must not steal focus from the newly selected frame",
    );
    await inspector.locator(".call-stack button").filter({ hasText: "run" }).click();
    await ready();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdValueReply = true;
    });
    await row("value").press("F2");
    await input.fill("100");
    await input.press("Enter");
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishValueReply,
    );
    await outer.click();
    await ready();
    await inspector.locator(".call-stack button").filter({ hasText: "run" }).click();
    await ready();
    await row("value").press("F2");
    await input.fill("102");
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishValueReply());
    await page.waitForFunction(
      () =>
        !document.querySelector("app-shell").shadowRoot.querySelector("debugger-workspace")
          .valueEditOperation,
    );
    await ready();
    assert.equal(
      await input.inputValue(),
      "102",
      "An old reply cannot clear a newer draft after returning to the same frame",
    );
    await input.press("Escape");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdValueEdit = true;
    });
    await row("value").press("F2");
    await input.fill("101");
    await input.press("Enter");
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishValueEdit,
    );
    await workspace.evaluate((element) => {
      element.pendingValueMove = element.place("bottom");
    });
    await workspace.getByText("Applying value before moving debugger…", { exact: true }).waitFor();
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishValueEdit());
    await workspace.evaluate((element) => element.pendingValueMove);
    await ready();
    await row("value").getByText("101", { exact: true }).waitFor();
    assert.equal(
      await input.count(),
      0,
      "A confirmed edit is transferred as a result, not an uncommitted replacement draft",
    );

    const sample = row("sample");
    await sample.focus();
    await sample.press("ArrowRight");
    await row("answer").waitFor();
    await sample.press("ArrowLeft");
    assert.equal(await row("answer").count(), 0);
    await row("value").press("F2");
    await input.fill("101");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdPropertyHandle = 2;
    });
    await sample.press("ArrowRight");
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishPropertyRead,
    );
    const loading = local.getByRole("status").filter({ hasText: "Loading…" });
    await loading.waitFor();
    assert.equal(
      await local.getByRole("button", { name: "Apply value", exact: true }).isDisabled(),
      true,
      "A value edit cannot submit while its tree is loading",
    );
    assert.equal(await loading.getAttribute("aria-level"), null);
    assert.equal(await loading.getAttribute("tabindex"), null);
    assert.equal(
      await loading.evaluate((element) =>
        element.previousElementSibling?.textContent.includes("sample"),
      ),
      true,
      "Loading belongs immediately beneath the expanding property, before its next sibling",
    );
    assert.equal(await local.getByText("No properties", { exact: true }).count(), 0);
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/debugger-branch-loading.png"),
    });
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead();
    });
    await row("answer").waitFor();
    await input.press("Escape");
    const closure = inspector
      .locator("debug-value")
      .filter({ has: page.getByRole("tree", { name: "closure · outer", exact: true }) });
    if (
      (await closure.getByRole("button", { name: /closure/u }).getAttribute("aria-expanded")) !==
      "true"
    )
      await closure.getByRole("button", { name: /closure/u }).click();
    await closure.getByText("42", { exact: true }).waitFor();
    await closure.locator("tr").first().press("Shift+F10");
    assert.equal(
      await page.getByRole("menuitem", { name: "Add to Watch", exact: true }).count(),
      0,
      "A shadowed outer binding cannot be represented by an unqualified watch path",
    );
    assert.equal(
      await page.getByRole("menuitem", { name: "Copy property path", exact: true }).count(),
      0,
    );
    await page.getByRole("menuitem", { name: "View value", exact: true }).press("Escape");
    await local.evaluate((element) => {
      const tree = element.shadowRoot.querySelector("tbody");
      element.retainedRows = [...tree.querySelectorAll("tr")];
      element.removedRows = [];
      element.rowObserver = new MutationObserver((records) => {
        for (const record of records) {
          for (const node of record.removedNodes) {
            if (element.retainedRows.includes(node) || node.contains?.(tree))
              element.removedRows.push(node);
          }
        }
      });
      element.rowObserver.observe(element.shadowRoot, { subtree: true, childList: true });
    });
    await workspace.locator(".debug-inline-value").filter({ hasText: "value = 101" }).waitFor();
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.holdPropertyHandle = 2;
      // Identical parent previews must not conceal changes to their children.
      debug.propertyInventory[2][0].value.display = "43";
    });
    await row("answer").focus();
    await page.keyboard.press("F10");
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishPropertyRead,
    );
    await row("answer").getByText("42", { exact: true }).waitFor();
    assert.equal(await local.getByText("Expired", { exact: true }).isVisible(), true);
    assert.equal(
      await workspace.locator(".debug-inline-value").count(),
      0,
      "Expired scope snapshots must not appear as current values beside the new execution line",
    );
    assert.equal(
      await row("answer").evaluate((element) => element.getRootNode().activeElement === element),
      true,
      "A stepping refresh keeps keyboard focus on the inspected leaf",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/debugger-step-refresh.png"),
    });
    assert.equal(
      await workspace.evaluate((element) => {
        const closure = element.debuggerScopeValues.find(
          (value) => value.label === "closure · outer",
        );
        return new Map(element.snapshotView().inspection.states).get(closure.presentationKey)?.open;
      }),
      true,
      "A handoff preserves expansion of a staged scope that has not started restoration",
    );
    // The next scope has not begun restoration while Local's child read is held.
    await page.keyboard.press("F10");
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead();
    });
    await ready();
    await row("answer").getByText("43", { exact: true }).waitFor();
    await workspace.locator(".debug-inline-value").filter({ hasText: "value = 101" }).waitFor();
    assert.equal(await row("answer").getAttribute("data-changed"), "");
    assert.equal(
      await local.evaluate((element) => {
        element.rowObserver.disconnect();
        return element.removedRows.length;
      }),
      0,
      "Stepping must never remove and reinsert the expanded rows, even while reads are delayed",
    );
    assert.equal(
      await sample
        .getByRole("button", { name: "sample", exact: true })
        .getAttribute("aria-expanded"),
      "true",
    );
    assert.equal(
      await closure.getByRole("button", { name: /closure/u }).getAttribute("aria-expanded"),
      "true",
      "Stepping again before a later scope starts restoring must preserve its expansion intent",
    );

    await row("value").press("F2");
    await input.fill("777");
    const editsBeforeRefresh = await page.evaluate(
      () =>
        globalThis.__workspaceFixture.workspaceDebug.actions.filter(
          (action) => action.kind === "set-value",
        ).length,
    );
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdPropertyHandle = 2;
    });
    await workspace.evaluate((element) => {
      element.pendingScopeRefresh = element.refreshInspection();
    });
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishPropertyRead,
    );
    assert.equal(await input.inputValue(), "777");
    await input.press("Enter");
    assert.equal(
      await page.evaluate(
        () =>
          globalThis.__workspaceFixture.workspaceDebug.actions.filter(
            (action) => action.kind === "set-value",
          ).length,
      ),
      editsBeforeRefresh,
      "Enter in a retained expired editor must not edit the staged replacement",
    );
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead();
    });
    await workspace.evaluate((element) => element.pendingScopeRefresh);
    await ready();
    await input.press("Escape");

    await prompt.fill("value = 123");
    await prompt.press("Enter");
    await row("value").getByText("123", { exact: true }).waitFor();
    await inspector.locator(".watch").getByText("123", { exact: true }).waitFor();
    await row("answer").waitFor();

    await workspace.evaluate((element) => element.addWatchExpression("sample"));
    const sampleWatch = inspector
      .locator(".watch debug-value")
      .filter({ has: page.getByRole("tree", { name: "sample", exact: true }) });
    await sampleWatch.getByRole("button", { name: "sample", exact: true }).click();
    await sampleWatch.getByText("answer", { exact: true }).waitFor();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdPropertyHandle = 2;
    });
    await inspector.getByRole("button", { name: "Refresh watches", exact: true }).click();
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishPropertyRead,
    );
    await workspace.evaluate((element) => element.inspectionChanged());
    assert.equal(await sampleWatch.getByText("Expired", { exact: true }).isVisible(), true);
    assert.equal(
      await workspace.evaluate(
        (element) => new Map(element.snapshotView().inspection.states).get("watch:sample")?.open,
      ),
      true,
      "A handoff preserves the latest Watch expansion while its matching replacement is loading",
    );
    await sampleWatch.locator(".heading").press("ArrowLeft");
    await sampleWatch.locator(".heading").press("F10");
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead();
    });
    await ready();
    await sampleWatch.getByText("Expired", { exact: true }).waitFor({ state: "detached" });
    assert.equal(
      await sampleWatch
        .getByRole("button", { name: "sample", exact: true })
        .getAttribute("aria-expanded"),
      "true",
      "Stepping during a manual refresh preserves the Watch's latest expanded state",
    );
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdScopesFrame = 1;
    });
    await inspector.locator(".call-stack button").filter({ hasText: "outer" }).click();
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.finishScopes);
    await sampleWatch.getByText("Expired", { exact: true }).waitFor({ state: "detached" });
    assert.equal(
      await sampleWatch
        .getByRole("button", { name: "sample", exact: true })
        .getAttribute("aria-expanded"),
      "false",
      "A previously unvisited frame has independent Watch expansion state",
    );
    await inspector.locator(".call-stack button").filter({ hasText: "run" }).click();
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdScopesFrame;
      debug.finishScopes();
    });
    await ready();
    await sampleWatch.getByText("Expired", { exact: true }).waitFor({ state: "detached" });
    assert.equal(
      await sampleWatch
        .getByRole("button", { name: "sample", exact: true })
        .getAttribute("aria-expanded"),
      "true",
      "A Watch refreshed ahead of delayed Scopes must not overwrite the previous frame's expansion memory",
    );
    await inspector.getByRole("button", { name: "Remove watch sample", exact: true }).click();

    await workspace.evaluate((element) => element.selectFrame(1));
    await ready();
    const outerScroll = await workspace.locator(".variables").evaluate((element) => {
      element.scrollTop = 10;
      return element.scrollTop;
    });
    await workspace.evaluate((element) => element.selectFrame(0));
    await ready();
    const runScroll = await workspace.locator(".variables").evaluate((element) => {
      element.scrollTop = 80;
      return element.scrollTop;
    });
    assert.ok(runScroll > outerScroll, "The frames have distinct reading positions");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.holdScopesFrame = 1;
    });
    await workspace.evaluate((element) => element.selectFrame(1));
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.workspaceDebug.finishScopes);
    assert.equal(
      await workspace.evaluate((element) => element.snapshotView().inspection.scroll),
      outerScroll,
      "A pending frame handoff must save that frame's reading position, not the retained tree's",
    );
    const retainedScroll = await workspace.locator(".variables").evaluate((element, scroll) => {
      element.scrollTop = scroll / 2;
      return element.scrollTop;
    }, runScroll);
    assert.notEqual(retainedScroll, runScroll);
    await workspace.evaluate((element) => element.selectFrame(0));
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdScopesFrame;
      debug.finishScopes();
    });
    await ready();
    assert.equal(
      await workspace.locator(".variables").evaluate((element) => element.scrollTop),
      retainedScroll,
      "Scrolling the retained tree updates its own frame's reading position",
    );
    await workspace.evaluate((element) => element.selectFrame(1));
    await ready();
    assert.equal(
      await workspace.locator(".variables").evaluate((element) => element.scrollTop),
      outerScroll,
      "Leaving a pending frame must not overwrite its saved reading position",
    );
    await workspace.evaluate((element) => element.selectFrame(0));
    await ready();

    await local.evaluate((element) => {
      globalThis.__debuggerLocalBeforeBlock = element;
    });
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.scopeInventory = [
        { kind: "block", name: "", handle: 7 },
        { kind: "local", name: "", handle: 1 },
        { kind: "closure", name: "outer", handle: 3 },
        { kind: "global", name: "", handle: 5 },
      ];
      debug.holdPropertyHandle = 2;
      debug.pause();
    });
    await page.waitForFunction(
      () => !!globalThis.__workspaceFixture.workspaceDebug.finishPropertyRead,
    );
    const savedLocal = await workspace.evaluate((element) => {
      const local = element.debuggerScopeValues.find((value) => value.label === "local");
      return new Map(element.snapshotView().inspection.states).get(local.presentationKey);
    });
    assert.equal(
      savedLocal?.open,
      true,
      "A handoff must not save a staged Block as the previous Local scope",
    );
    assert.ok(savedLocal.paths.some((path) => JSON.stringify(path) === JSON.stringify(["sample"])));
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead();
    });
    await ready();
    await row("answer").waitFor();
    assert.equal(
      await local.evaluate((element) => element === globalThis.__debuggerLocalBeforeBlock),
      true,
      "Entering a block preserves the Local scope and its expanded descendants when its index changes",
    );
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.scopeInventory;
      debug.pause();
    });
    await ready();
    await row("answer").waitFor();

    await row("message").focus();
    await row("message").press("Shift+F10");
    await page.getByRole("menuitem", { name: "View value", exact: true }).click();
    const detail = local.getByRole("dialog", { name: "Inspect value", exact: true });
    assert.equal(
      await detail.getByRole("textbox", { name: "Value text", exact: true }).inputValue(),
      longValue,
    );
    await detail.getByRole("textbox").press("Escape");
    await detail.waitFor({ state: "hidden" });
    assert.equal(await row("message").evaluate((e) => e.getRootNode().activeElement === e), true);
    await row("secret").locator(".property-preview").dblclick();
    await detail
      .getByText("This property is read-only. Accessors are not evaluated.", { exact: true })
      .waitFor();
    await detail.getByRole("button", { name: "Close", exact: true }).click();
    assert.equal(await local.locator(".value-edit").count(), 0);

    await page.setViewportSize({ width: 1024, height: 768 });
    await row("value").press("F2");
    await input.fill("124");
    await sample.evaluate((element) => {
      globalThis.__debuggerRowBeforeInsertion = element;
    });
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.propertyInventory[1].unshift({
        name: "inserted",
        value: { kind: "number", display: "5", handle: null, truncated: false },
      });
    });
    await prompt.fill("0");
    await prompt.press("Enter");
    await row("inserted").waitFor();
    assert.equal(
      await sample.evaluate((element) => element === globalThis.__debuggerRowBeforeInsertion),
      true,
      "Inserting a sibling preserves the object row by property path rather than native index",
    );
    await input.waitFor();
    assert.equal(
      await input.inputValue(),
      "124",
      "Refresh keeps the draft attached to its property name after reordering",
    );
    assert.equal(
      await prompt.evaluate((element) => element.getRootNode().activeElement === element),
      true,
      "Restoring an edit draft must not steal Console focus",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/debugger-value-edit-1024.png"),
    });
    await page.emulateMedia({ forcedColors: "active" });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/debugger-value-edit-forced-colors.png"),
    });
    await page.emulateMedia({ forcedColors: "none" });
    assert.equal(await input.evaluate((e) => e.getBoundingClientRect().right <= innerWidth), true);
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.recreateSurface = true;
    });
    await workspace.evaluate((element) => element.place("window"));
    await ready();
    await input.waitFor();
    assert.equal(
      await input.inputValue(),
      "124",
      "An uncommitted value draft survives moving the debugger surface",
    );
    await input.press("Escape");
    await page.setViewportSize({ width: 1440, height: 900 });
    await row("answer").focus();
    await row("answer").press("Shift+F10");
    await page.getByRole("menuitem", { name: "Add to Watch", exact: true }).click();
    await inspector
      .getByRole("button", { name: "Edit watch sample.answer", exact: true })
      .waitFor();
    await inspector
      .getByRole("button", { name: "Remove watch sample.answer", exact: true })
      .click();
    const script = page.locator("script-editor");
    const draft = await script.evaluate((element) => element.sourceEditor.getValue());
    await workspace.getByRole("button", { name: "Edit script", exact: true }).click();
    await script
      .getByText(
        "Opened the current draft. It differs from the debugger source snapshot; the running build is unchanged.",
        { exact: true },
      )
      .waitFor();
    assert.equal(
      await script.evaluate((element) => element.sourceEditor.getValue()),
      draft,
      "Opening the authoring draft cannot replace it with the executing snapshot",
    );
    await page.locator("app-shell a[data-view='traffic']").click();
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.propertyInventory[2].push(
        ...[11, 22].map((value) => ({
          name: "[Symbol key]",
          value: { kind: "number", display: String(value), handle: null, truncated: false },
        })),
      );
      debug.propertyInventory[1].push({
        name: "proxy",
        writable: true,
        value: {
          kind: "unavailable",
          display: "Proxy: not inspected",
          handle: null,
          truncated: false,
        },
      });
      debug.pause();
    });
    await ready();
    const symbols = row("[Symbol key]");
    await symbols.nth(1).waitFor();
    await symbols.nth(1).press("F2");
    const symbolInput = symbols.nth(1).getByRole("textbox");
    await symbolInput.fill("33");
    await closure.getByRole("button", { name: /closure/u }).click();
    assert.equal(
      await symbolInput.inputValue(),
      "33",
      "Duplicate display names retain the exact editing row during updates",
    );
    assert.equal(await symbols.first().getByRole("textbox").count(), 0);
    await prompt.fill("0");
    await prompt.press("Enter");
    await symbols.nth(1).getByText("22", { exact: true }).waitFor();
    assert.equal(
      await symbols.getByRole("textbox").count(),
      0,
      "Refreshing cannot attach a Symbol draft to another native key",
    );
    await row("proxy").press("F2");
    const proxyInput = local.getByRole("textbox", { name: "New value for proxy", exact: true });
    assert.equal(
      await proxyInput.inputValue(),
      "",
      "Display-only previews are not offered as JavaScript expressions",
    );
    await proxyInput.press("Escape");
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      debug.scopeInventory = [
        { kind: "block", name: "", handle: 7 },
        { kind: "local", name: "run", handle: 1 },
        { kind: "global", name: "", handle: 5 },
      ];
      debug.pause();
    });
    await inspector
      .getByRole("tree", { name: "local · run", exact: true })
      .getByText("123", { exact: true })
      .waitFor();
    assert.equal(
      await inspector
        .getByRole("tree", { name: "block", exact: true })
        .getByRole("button", { name: "block", exact: true })
        .getAttribute("aria-expanded"),
      "false",
      "Opening a named Local scope must not accidentally expand the preceding block scope",
    );
  } catch (error) {
    console.error("Debugger value interaction failed:", error);
    throw error;
  } finally {
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.holdPropertyHandle;
      debug.finishPropertyRead?.();
      delete debug.holdScopesFrame;
      debug.finishScopes?.();
    });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishValueEdit?.());
    await page.evaluate(() => globalThis.__workspaceFixture.workspaceDebug.finishValueReply?.());
    await workspace.evaluate((element) => element.valueEditOperation);
    await page.evaluate(() => {
      const debug = globalThis.__workspaceFixture.workspaceDebug;
      delete debug.propertyInventory;
      delete debug.scopeInventory;
      delete debug.value;
      debug.pause();
    });
    await workspace.evaluate((element) => element.selectFrame(0));
    await local.getByText("42", { exact: true }).waitFor();
  }
}
