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
    await sample.press("ArrowRight");
    await row("answer").waitFor();
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
    await prompt.press("F10");
    await ready();
    await row("answer").waitFor();
    assert.equal(
      await sample
        .getByRole("button", { name: "sample", exact: true })
        .getAttribute("aria-expanded"),
      "true",
    );
    assert.equal(
      await closure.getByRole("button", { name: /closure/u }).getAttribute("aria-expanded"),
      "true",
    );

    await prompt.fill("value = 123");
    await prompt.press("Enter");
    await row("value").getByText("123", { exact: true }).waitFor();
    await inspector.locator(".watch").getByText("123", { exact: true }).waitFor();
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
    await page.evaluate(() => {
      globalThis.__workspaceFixture.workspaceDebug.propertyInventory[1].unshift({
        name: "inserted",
        value: { kind: "number", display: "5", handle: null, truncated: false },
      });
    });
    await prompt.fill("0");
    await prompt.press("Enter");
    await row("inserted").waitFor();
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
