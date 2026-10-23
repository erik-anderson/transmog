import { newTrafficRule } from "./check-unified-rules.mjs";
import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkHeaderRules(page, root) {
  await page.addInitScript(() => sessionStorage.setItem("startup-update-tested", "true"));
  await page.goto("https://workspace.test/");
  await page.waitForFunction(() => document.querySelector("app-shell").traffic.queryLoaded);
  await page.evaluate(async () => {
    const shell = document.querySelector("app-shell");
    await shell.activateView("automation");
    const s = globalThis.__workspaceFixture;
    s.automation = {
      ...s.automation,
      generation: 1,
      rules: [],
      autoresponsesEnabled: false,
      experiments: [],
    };
    await shell.automation.refreshAutomation();
    shell.automation.showAutomationSection("header-rules");
  });
  const pane = page.locator('header-rules[editor-kind="headers"]'),
    editor = pane.locator("form"),
    ops = pane.locator(".header-operations li");
  const settle = () =>
    page.waitForFunction(
      () => !document.querySelector("app-shell").automation.querySelector("header-rules").hrBusy,
    );
  const current = () =>
    page.evaluate(() => structuredClone(globalThis.__workspaceFixture.automation));
  // Author, test, save and reopen conditional overrides for both directions.
  for (const phase of ["request", "response"]) {
    await newTrafficRule(page, `Change ${phase} headers…`);
    await editor
      .getByLabel("Rule name", { exact: true })
      .fill(`Conditional ${phase} header override`);
    await ops
      .first()
      .getByLabel("Header operation", { exact: true })
      .selectOption({ label: "Override if present" });
    await ops.first().getByLabel("Operation header name", { exact: true }).fill("X-Existing");
    await ops.first().getByLabel("Operation header value", { exact: true }).fill("replacement");
    await editor.getByRole("button", { name: "Add operation", exact: true }).click();
    await ops
      .nth(1)
      .getByLabel("Header operation", { exact: true })
      .selectOption({ label: "Override if present" });
    await ops.nth(1).getByLabel("Operation header name", { exact: true }).fill("X-Missing");
    await ops.nth(1).getByLabel("Operation header value", { exact: true }).fill("absent");
    const tester = pane.locator(".header-draft-test");
    await tester.locator("summary").click();
    await tester
      .getByLabel(`Sample ${phase} headers`, { exact: true })
      .fill("X-Existing: first\nx-existing: second\nX-Keep: untouched");
    await tester.getByRole("button", { name: "Test draft", exact: true }).click();
    await page.waitForFunction(
      () => !document.querySelector("app-shell").automation.headerRulesEditor.hrTestBusy,
    );
    const after = tester.locator(".header-sample-results > div").last();
    assert.deepEqual(await after.locator("dt").allTextContents(), ["X-Existing", "X-Keep"]);
    assert.deepEqual(await after.locator("dd").allTextContents(), ["replacement", "untouched"]);
    assert.match(await tester.textContent(), /Override if present X-Missing · No change/);
    await tester.locator("summary").click();
    await editor.getByRole("button", { name: "Save rule", exact: true }).click();
    await settle();
    assert.deepEqual(
      (await current()).rules.at(-1)[phase].headers.map((op) => op.operation),
      ["override-if-present", "override-if-present"],
    );
    await newTrafficRule(page, `Change ${phase} headers…`);
    await page
      .locator("automation-workspace")
      .getByRole("button", { name: new RegExp(`^Conditional ${phase} header override`) })
      .click();
    assert.equal(
      await ops.first().getByLabel("Header operation", { exact: true }).inputValue(),
      "override-if-present",
    );
    await editor.getByRole("button", { name: "Remove rule", exact: true }).click();
    await settle();
  }
  await newTrafficRule(page, "Change response headers…");
  await editor
    .getByLabel("Rule name", { exact: true })
    .fill("Remove CSP and patch policy with duplicate fields and preserved byte values");
  await editor.getByLabel("Matching address", { exact: true }).fill("https://example.test/policy");
  await ops
    .nth(0)
    .getByLabel("Operation header name", { exact: true })
    .fill("Content-Security-Policy");
  await editor.getByRole("button", { name: "Add operation", exact: true }).click();
  await ops.nth(1).getByLabel("Operation header name", { exact: true }).fill("Cache-Control");
  await ops.nth(1).getByLabel("Operation header value", { exact: true }).fill("no-cache");
  await editor.getByRole("button", { name: "Add operation", exact: true }).click();
  await ops.nth(2).getByLabel("Header operation", { exact: true }).selectOption("append");
  await ops.nth(2).getByLabel("Operation header name", { exact: true }).fill("X-Bytes");
  await ops.nth(2).getByLabel("Operation value format", { exact: true }).selectOption("hex");
  await ops.nth(2).getByLabel("Operation header value", { exact: true }).fill("80 ff");
  const tester = pane.locator(".header-draft-test");
  await tester.locator("summary").click();
  await tester.getByLabel("Sample URL", { exact: true }).fill("https://example.test/policy");
  await tester
    .getByLabel("Sample response headers", { exact: true })
    .fill(
      "Content-Security-Policy: default-src 'none'\nCache-Control: max-age=60\ncache-control: private\nSet-Cookie: first=1\nSet-Cookie: second=2",
    );
  await tester.getByRole("button", { name: "Test draft", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").automation.querySelector("header-rules").hrTestBusy,
  );
  assert.match(await tester.textContent(), /Draft matches/);
  assert.deepEqual(
    await page.evaluate(
      () => globalThis.__workspaceFixture.lastHeaderTest.rule.response.headers.at(-1).field.value,
    ),
    [128, 255],
  );
  assert.equal(
    await tester.locator(".header-sample-results").getByText("first=1", { exact: true }).count(),
    2,
  );
  assert.match(await tester.textContent(), /Hex: 80 ff/);
  assert.equal((await current()).rules.length, 0, "Testing activated the rule");
  // Response predicates are retained and cannot silently move into an ignored request phase.
  await editor.getByText("Header conditions", { exact: true }).click();
  await editor.getByLabel("Response status condition", { exact: true }).fill("200");
  await editor.getByRole("button", { name: "Add response-header condition", exact: true }).click();
  await editor.getByLabel("Condition header name", { exact: true }).fill("Content-Type");
  await editor.getByLabel("Header condition", { exact: true }).selectOption("prefix");
  await editor.getByLabel("Condition value", { exact: true }).fill("text/html");
  await editor.getByLabel("Header rule phase", { exact: true }).selectOption("request");
  assert.equal(
    await editor.getByLabel("Header rule phase", { exact: true }).inputValue(),
    "response",
  );
  assert.match(
    await pane.locator(".header-rule-workspace > footer").textContent(),
    /Remove response-only/,
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.headerReject = true));
  await editor.getByRole("button", { name: "Save rule", exact: true }).click();
  await settle();
  assert.match(await pane.textContent(), /not saved/);
  assert.equal((await current()).rules.length, 0);
  assert.equal(
    await editor.getByLabel("Rule name", { exact: true }).inputValue(),
    "Remove CSP and patch policy with duplicate fields and preserved byte values",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.headerReject = false));
  await editor.getByRole("button", { name: "Save rule", exact: true }).click();
  await settle();
  let state = await current();
  const ruleId = state.rules[0].id;
  assert.equal(state.rules[0].enabled, false);
  assert.equal(state.autoresponsesEnabled, false);
  await editor.getByRole("button", { name: "Save and enable", exact: true }).click();
  await settle();
  state = await current();
  assert.equal(state.rules[0].enabled, true);
  assert.equal(state.autoresponsesEnabled, false);
  // Stale tests cannot replace newer draft feedback.
  await page.evaluate(() => (globalThis.__workspaceFixture.holdHeaderTest = true));
  await tester.getByRole("button", { name: "Test draft", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishHeaderTest);
  await ops.nth(1).getByLabel("Operation header value", { exact: true }).fill("private");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeaderTest());
  assert.equal(
    await page.evaluate(
      () =>
        document.querySelector("app-shell").automation.querySelector("header-rules").hrTestSummary,
    ),
    "",
  );
  // Unrelated refreshes preserve drafts; a changed target revision is rejected.
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture;
    s.automation.rules[0].revision++;
    s.automation.generation++;
    await document.querySelector("app-shell").automation.refreshAutomation();
  });
  assert.equal(
    await ops.nth(1).getByLabel("Operation header value", { exact: true }).inputValue(),
    "private",
  );
  await editor.getByRole("button", { name: "Save rule", exact: true }).click();
  await settle();
  assert.match(await pane.textContent(), /changed while you were editing/);
  await editor.getByRole("button", { name: "Discard changes", exact: true }).click();
  // Save in another workspace must not steal focus when it completes.
  await editor.getByLabel("Rule name", { exact: true }).fill("Changed before delayed save");
  await page.evaluate(() => (globalThis.__workspaceFixture.holdHeaderSave = true));
  await editor.getByRole("button", { name: "Save rule", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.finishHeaderSave);
  await page.evaluate(() => document.querySelector("app-shell").activateView("traffic"));
  const search = page.locator('traffic-workspace input[name="search"]');
  await search.focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeaderSave());
  await settle();
  assert.equal(await search.evaluate((e) => e === e.getRootNode().activeElement), true);
  await page.evaluate(() => document.querySelector("app-shell").activateView("automation"));
  // Keyboard unsaved protection and ordinary workspace changes keep the draft.
  await editor.getByLabel("Rule name", { exact: true }).fill("Pending name");
  await newTrafficRule(page, "Change response headers…");
  const unsaved = pane.locator(".header-unsaved");
  await unsaved.waitFor();
  await page.keyboard.press("Escape");
  assert.equal(await editor.getByLabel("Rule name", { exact: true }).inputValue(), "Pending name");
  await editor.getByRole("button", { name: "Discard changes", exact: true }).click();
  await editor.getByRole("button", { name: "Remove rule", exact: true }).click();
  await settle();
  assert.equal((await current()).rules.length, 0);
  await newTrafficRule(page, "Change response headers…");
  await editor.getByLabel("Rule name", { exact: true }).fill("Preserve this draft during Undo");
  await pane.getByRole("button", { name: "Undo remove", exact: true }).click();
  await unsaved.waitFor();
  await page.keyboard.press("Escape");
  assert.equal((await current()).rules.length, 0);
  assert.equal(
    await editor.getByLabel("Rule name", { exact: true }).inputValue(),
    "Preserve this draft during Undo",
  );
  await editor.getByRole("button", { name: "Discard changes", exact: true }).click();
  await pane.getByRole("button", { name: "Undo remove", exact: true }).click();
  await settle();
  assert.equal((await current()).rules[0].id, ruleId);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await editor.getByRole("button", { name: "Save rule", exact: true }).scrollIntoViewIfNeeded();
    const b = await editor.getByRole("button", { name: "Save rule", exact: true }).boundingBox();
    assert.ok(b.y >= 0 && b.y + b.height <= height);
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/header-rules-${width}.png`),
    });
  }
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  await page.emulateMedia({ forcedColors: "active" });
  await editor.getByRole("button", { name: "Save rule", exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/header-rules-zoom-contrast.png"),
  });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(async () => {
    const s = globalThis.__workspaceFixture,
      t = document.querySelector("app-shell").traffic;
    s.sessions[0].headerEffects = [
      {
        sequence: 2,
        hookId: "automation/recorded@7/request",
        name: "Recorded request header rule",
        phase: "Request",
        changed: true,
        headers: { added: ["X-Sample"], removed: [], changed: [] },
      },
      {
        sequence: 5,
        hookId: "script/recorded@3/response",
        name: "Recorded response script",
        phase: "Response",
        changed: false,
        headers: { added: [], removed: [], changed: [] },
      },
    ];
    await document.querySelector("app-shell").activateView("traffic");
    await t.revealSession("first");
    await t.showTimings();
  });
  const evidence = page.locator(".recorded-header-edits");
  await evidence.locator("summary").click();
  assert.match(await evidence.textContent(), /recorded@7/);
  assert.match(await evidence.textContent(), /No canonical change/);
  assert.match(await evidence.textContent(), /X-Sample/);
  assert.equal(await evidence.locator("li").count(), 2);
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/header-rule-observed-evidence.png"),
  });
  await page.keyboard.press("Escape");
  // Equal-position IDs use the engine's ASCII tie-break, independent of UI locale.
  assert.deepEqual(
    await page.evaluate(() => {
      const h = document.querySelector("app-shell").automation.querySelector("header-rules"),
        status = structuredClone(h.headerState),
        base = status.rules[0];
      h.headerState = {
        ...status,
        rules: ["a_b", "a-b"].map((id) => ({ ...base, id, priority: 0, enabled: false })),
      };
      h.headerStateChanged();
      return h.hrRows.map((r) => r.id);
    }),
    ["a-b", "a_b"],
  );
}
