import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkResponseSetIntake(page, root) {
  await page.goto("https://workspace.test/");
  const traffic = page.locator("traffic-workspace"),
    workspace = page.locator("automation-workspace"),
    sets = workspace.locator("response-set-workspace"),
    intake = page.locator("response-set-intake"),
    dialog = intake.locator(".response-set-intake-dialog");
  const evidence = resolve(root, "../../../target/response-set-workspaces/phase3");
  await mkdir(evidence, { recursive: true });
  const state = () =>
    page.evaluate(() => structuredClone(globalThis.__workspaceFixture.automation));
  const idle = () =>
    page.waitForFunction(() => {
      const picker = document.querySelector("app-shell").responseSetIntake;
      return !picker.intakeLoading && !picker.intakeReviewBusy && !picker.intakeSaving;
    });
  const acknowledge = async () => {
    const checkbox = dialog.getByRole("checkbox", {
      name: "Keep all responses with these overlapping matches",
      exact: true,
    });
    if (await checkbox.isVisible()) await checkbox.check();
  };
  const openTraffic = async () => {
    await page.getByRole("link", { name: "Traffic", exact: true }).click();
    await traffic.locator('tr[data-session-id="first"]').click();
    await traffic
      .locator(".selection-actions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    await traffic
      .locator("#selected-request-actions")
      .getByRole("button", { name: "Add to response set…", exact: true })
      .click();
    await dialog.waitFor({ state: "visible" });
    await idle();
  };
  await traffic.locator('tr[data-session-id="first"]').click();
  await traffic.locator('tr[data-session-id="second"]').click({ modifiers: ["Control"] });
  const selectedBefore = await page.evaluate(() => [
    ...document.querySelector("app-shell").traffic.trafficSelection.ids,
  ]);
  await traffic
    .locator(".traffic-selection-bar")
    .getByRole("button", { name: "Add to response set…", exact: true })
    .focus();
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "visible" });
  await idle();
  assert.equal(
    await dialog.getByRole("checkbox", { checked: true }).count(),
    2,
    "Traffic selection was not explicit in the picker",
  );
  await dialog.locator("#intake-source-first strong").click();
  await dialog.locator("#intake-source-second strong").click({ modifiers: ["Shift"] });
  assert.equal(
    await page.evaluate(() => String(getSelection())),
    "",
    "Shift range selection highlighted source text",
  );
  assert.equal(await dialog.getByRole("checkbox", { checked: true }).count(), 2);
  await dialog.getByRole("button", { name: "Clear selection", exact: true }).click();
  await dialog.locator("#intake-source-first").focus();
  await page.keyboard.press("Space");
  await page.keyboard.press("Shift+ArrowDown");
  assert.equal(
    await dialog.getByRole("checkbox", { checked: true }).count(),
    2,
    "Keyboard range selection did not collect both responses",
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.screenshot({ path: resolve(evidence, "source-picker-1440.png") });
  await page.setViewportSize({ width: 1280, height: 800 });
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").activeView),
    "traffic",
  );
  await dialog.getByLabel("Response set", { exact: true }).selectOption("__new");
  await dialog.getByLabel("Response set name", { exact: true }).fill("Collected checkout work");
  await dialog
    .getByLabel("Notes", { exact: true })
    .fill("Captured checkout responses and follow-up rules.");
  const originalLive = JSON.stringify((await state())?.rules ?? []);
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await idle();
  assert.equal(await dialog.locator(".intake-review ol > li").count(), 2);
  await acknowledge();
  await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
  await idle();
  assert.match(
    await dialog.textContent(),
    /Added 2 responses to Collected checkout work\. Live traffic is unchanged\./,
  );
  let saved = await state(),
    destination = saved.responseSets[0].id;
  assert.equal(saved.responseSets[0].rules.length, 2);
  assert.equal(JSON.stringify(saved.rules), originalLive);
  assert.equal(saved.responseSets[0].applied, null);
  assert.equal(saved.responseSets[0].versions.length, 0);
  assert.deepEqual(
    await page.evaluate(() => [
      ...document.querySelector("app-shell").traffic.trafficSelection.ids,
    ]),
    selectedBefore,
  );
  await dialog.getByRole("button", { name: "Open response set", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("app-shell").automation.collectionOpen);
  // Ordinary sources preserve names, complete matchers, linked/copied references and inclusion.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture,
      action = { headers: [], replaceBody: null, discardBody: false, abortReason: null };
    state.assets ??= [];
    for (const [index, name] of [
      [1, "Checkout response from a linked file with a deliberately long descriptive name"],
      [2, "Excluded account response"],
    ]) {
      const id = "intake-asset-" + index;
      state.assets.push({
        id,
        revision: 1,
        status: 200,
        bodyBytes: 5,
        sha256: "f".repeat(64),
        mediaType: "application/octet-stream",
        headers: [],
        provenance: { kind: "imported" },
        display: "body" + index,
        rawBody: [0, 255, 65, 13, 10],
        linkedBody:
          index === 1
            ? {
                path: "C:/fixtures/a long folder/checkout response.bin",
                originalContentCodings: [],
                applyOriginalCompression: false,
              }
            : null,
        copiedFrom: index === 2 ? "C:/fixtures/copied response.bin" : null,
      });
      state.automation.rules.push({
        id: "intake-source-" + index,
        displayName: name,
        enabled: index === 1,
        revision: 7,
        priority: 10 * index,
        matcher: {
          method: "POST",
          url: {
            kind: "exact",
            value: "https://example.test/" + (index === 1 ? "checkout" : "account"),
          },
          requestHeaders: [
            { name: "X-Original", condition: { kind: "prefix", value: [107, 101, 101, 112] } },
          ],
          responseHeaders: [],
          scheme: null,
          host: null,
          port: null,
          pathPrefix: null,
          query: null,
          responseStatus: null,
          responseStatusClass: null,
        },
        request: { ...action, responseAsset: id + "@1", allowNonIdempotentBodyReplacement: false },
        response: { ...action },
      });
    }
    state.automation.generation++;
    document
      .querySelector("app-shell")
      .automation.onVariantStatus(
        new CustomEvent("set-status", { detail: structuredClone(state.automation) }),
      );
  });
  const sourceBefore = (await state()).rules;
  await sets.getByRole("button", { name: "Add responses…", exact: true }).click();
  await sets
    .locator("#set-add-menu")
    .getByRole("button", { name: "From existing rules…", exact: true })
    .click();
  await dialog.waitFor({ state: "visible" });
  await idle();
  assert.equal(
    await dialog.locator(".intake-source-list tbody tr").count(),
    2,
    "Owned set content appeared as ordinary sources",
  );
  await dialog
    .getByRole("checkbox", { name: "Select " + sourceBefore[1].displayName, exact: true })
    .check();
  await dialog
    .getByRole("checkbox", { name: "Select " + sourceBefore[0].displayName, exact: true })
    .check();
  assert.equal(await dialog.getByRole("radio", { name: /Copy rules/ }).isChecked(), true);
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await idle();
  assert.match(await dialog.locator(".intake-review").textContent(), /Linked file: C:\/fixtures/);
  assert.match(
    await dialog.locator(".intake-review").textContent(),
    /Add to Collected checkout work · Copy rules/,
  );
  assert.match(await dialog.locator(".intake-review").textContent(), /Excluded/);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    const bounds = await dialog.boundingBox();
    assert.ok(
      bounds.x >= 0 &&
        bounds.y >= 0 &&
        bounds.x + bounds.width <= width + 1 &&
        bounds.y + bounds.height <= height + 1,
    );
    assert.equal(await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 2), true);
    await mkdir(resolve(root, "../../../target/response-set-workspaces/phase3"), {
      recursive: true,
    });
    await page.screenshot({
      path: resolve(root, `../../../target/response-set-workspaces/phase3/intake-${width}.png`),
    });
  }
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/response-set-workspaces/phase3/intake-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.evaluate(() => (document.documentElement.style.zoom = "1.5"));
  await page.screenshot({
    path: resolve(root, "../../../target/response-set-workspaces/phase3/intake-zoom.png"),
  });
  await page.evaluate(() => (document.documentElement.style.zoom = ""));
  await acknowledge();
  await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
  await idle();
  saved = await state();
  assert.deepEqual(saved.rules, sourceBefore);
  assert.equal(saved.responseSets[0].rules.length, 4);
  assert.deepEqual(
    saved.responseSets[0].rules
      .slice(2)
      .map((rule) => [rule.displayName, rule.enabled, rule.matcher, rule.request.responseAsset]),
    sourceBefore.map((rule) => [
      rule.displayName,
      rule.enabled,
      rule.matcher,
      rule.request.responseAsset,
    ]),
  );
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.setViewportSize({ width: 1280, height: 800 });
  // Destination drafts are resolved explicitly; Discard does not run on cancellation or failure.
  await sets.getByRole("button", { name: "← Response sets", exact: true }).click();
  await sets.getByRole("button", { name: "Collected checkout work", exact: true }).click();
  await sets
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Keep this destination draft");
  const openRules = async () => {
    await sets.getByRole("button", { name: "Add responses…", exact: true }).click();
    await sets
      .locator("#set-add-menu")
      .getByRole("button", { name: "From existing rules…", exact: true })
      .click();
    await dialog.waitFor({ state: "visible" });
    await idle();
    await dialog
      .getByRole("checkbox", { name: "Select " + sourceBefore[0].displayName, exact: true })
      .check();
    await dialog.getByRole("radio", { name: /Move rules/ }).check();
    await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  };
  await openRules();
  const unsaved = workspace.locator(".unsaved-rule-dialog");
  await unsaved.waitFor({ state: "visible" });
  await unsaved.getByRole("button", { name: "Keep editing", exact: true }).click();
  await idle();
  assert.equal(
    await sets.locator(".response-set-current").getByLabel("Notes", { exact: true }).inputValue(),
    "Keep this destination draft",
  );
  await page.keyboard.press("Escape");
  await openRules();
  await unsaved.getByRole("button", { name: "Discard changes", exact: true }).click();
  await idle();
  assert.match(await dialog.locator(".intake-warning").textContent(), /1 selected rule is active/);
  await page.keyboard.press("Escape");
  assert.equal(
    await sets.locator(".response-set-current").getByLabel("Notes", { exact: true }).inputValue(),
    "Keep this destination draft",
  );
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    true,
  );
  await openRules();
  await unsaved.getByRole("button", { name: "Discard changes", exact: true }).click();
  await idle();
  await acknowledge();
  await page.screenshot({ path: resolve(evidence, "move-active-overlap.png") });
  await page.evaluate(() => (globalThis.__workspaceFixture.intakeReject = true));
  await dialog.getByRole("button", { name: "Move 1 rule", exact: true }).click();
  await idle();
  assert.match(await dialog.locator(".field-error").textContent(), /save failed/);
  assert.deepEqual((await state()).rules, sourceBefore);
  assert.equal(
    await sets.locator(".response-set-current").getByLabel("Notes", { exact: true }).inputValue(),
    "Keep this destination draft",
  );
  await page.evaluate(() => (globalThis.__workspaceFixture.intakeReject = false));
  await dialog.getByRole("button", { name: "Move 1 rule", exact: true }).click();
  await idle();
  saved = await state();
  assert.equal(saved.rules.length, 1);
  assert.equal(saved.rules[0].id, sourceBefore[1].id);
  assert.equal(saved.responseSets[0].applied, null);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").automation.responseSets.dirty),
    false,
  );
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  // A mounted destination draft is protected even while Traffic is visible.
  await sets
    .locator(".response-set-current")
    .getByLabel("Notes", { exact: true })
    .fill("Hidden destination draft");
  await openTraffic();
  await dialog.getByLabel("Response set", { exact: true }).selectOption(destination);
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await unsaved.waitFor({ state: "visible" });
  const shortcutsBefore = await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    return {
      selection: [...document.querySelector("app-shell").traffic.trafficSelection.ids],
      exports: state.calls.review_traffic_export ?? 0,
      opens: state.calls.pick_trace_path ?? 0,
      removes: state.calls.remove_traffic_entries ?? 0,
    };
  });
  await page.keyboard.press("Control+s");
  await page.keyboard.press("Control+o");
  await page.keyboard.press("Delete");
  await page.keyboard.press("Escape");
  await idle();
  assert.equal(await unsaved.isVisible(), false);
  assert.equal(await dialog.isVisible(), true);
  assert.deepEqual(
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      return {
        selection: [...document.querySelector("app-shell").traffic.trafficSelection.ids],
        exports: state.calls.review_traffic_export ?? 0,
        opens: state.calls.pick_trace_path ?? 0,
        removes: state.calls.remove_traffic_entries ?? 0,
      };
    }),
    shortcutsBefore,
    "The hidden-destination prompt leaked Traffic/global shortcuts",
  );
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await unsaved.waitFor({ state: "visible" });
  await unsaved.getByRole("button", { name: "Keep editing", exact: true }).click();
  await idle();
  assert.equal(
    await sets.locator(".response-set-current").getByLabel("Notes", { exact: true }).inputValue(),
    "Hidden destination draft",
  );
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: /Automation/ }).click();
  await sets.getByRole("button", { name: "Discard changes", exact: true }).click();
  // A committed late result stays available, without reopening dialogs or navigating.
  await openTraffic();
  await dialog.getByLabel("Response set", { exact: true }).selectOption(destination);
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await idle();
  await acknowledge();
  await page.evaluate(() => (globalThis.__workspaceFixture.holdCommand = "collect_response_set"));
  await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
  await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishHeldCommand);
  await page.keyboard.press("Escape");
  await page.evaluate(() => globalThis.__workspaceFixture.finishHeldCommand());
  await idle();
  assert.equal(await dialog.isVisible(), false);
  assert.equal(
    await page.evaluate(() => document.querySelector("app-shell").activeView),
    "traffic",
  );
  assert.equal(await intake.locator(".intake-result").isVisible(), true);
  assert.match(await intake.locator(".intake-result").textContent(), /Added 1 response/);
  // Generation changes after review fail in place, keeping selected input available.
  await openTraffic();
  await dialog.getByLabel("Response set", { exact: true }).selectOption(destination);
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  await idle();
  await acknowledge();
  await page.evaluate(() => globalThis.__workspaceFixture.automation.generation++);
  await dialog.getByRole("button", { name: "Add responses", exact: true }).click();
  await idle();
  assert.match(await dialog.locator(".field-error").textContent(), /changed/);
  assert.equal(await dialog.locator(".intake-review ol > li").count(), 1);
  await dialog.getByRole("button", { name: "Back", exact: true }).click();
  assert.equal(await dialog.getByRole("checkbox", { checked: true }).count(), 1);
  await page.keyboard.press("Escape");
}
