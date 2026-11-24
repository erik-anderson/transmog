import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

export async function checkResponseSetIntake(page, root, errors) {
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
  const trafficOpener = async () => {
    await page.getByRole("link", { name: "Traffic", exact: true }).click();
    await traffic.locator('tr[data-session-id="first"]').click();
    await traffic
      .locator(".selection-actions")
      .getByRole("button", { name: "Actions", exact: true })
      .click();
    return traffic
      .locator("#selected-request-actions")
      .getByRole("button", { name: "Add to response set…", exact: true });
  };
  const openTraffic = async ({ keyboard = false } = {}) => {
    const add = await trafficOpener();
    if (keyboard) await add.press("Enter");
    else await add.click();
    await dialog.waitFor({ state: "visible" });
    await idle();
  };
  const requestIntake = (detail) =>
    page.evaluate(
      (detail) =>
        document
          .querySelector("app-shell")
          .onResponseSetIntake(new CustomEvent("response-set-intake-request", { detail })),
      detail,
    );
  assert.equal(await intake.count(), 0, "Intake component mounted at startup");
  assert.equal(
    await page.evaluate(() =>
      performance.getEntriesByType("resource").some(({ name }) => name.includes("/_webui/packs/")),
    ),
    false,
    "Intake fetched a template pack at startup",
  );
  const pack = "**/_webui/packs/*.json";
  const prefetched = page.waitForResponse((response) => response.url().includes("/_webui/packs/"));
  const add = await trafficOpener();
  const queries = () => page.evaluate(() => globalThis.__workspaceFixture.intakeTrafficQueries);
  await add.focus();
  assert.equal((await prefetched).ok(), true);
  assert.equal(await dialog.isVisible(), false, "Prefetch opened the picker");
  assert.equal(await queries(), undefined, "Prefetch queried captured responses");
  await page.goto("https://workspace.test/");
  const releasePack = Promise.withResolvers();
  const errorsBeforeFailure = errors.length;
  let packRequests = 0,
    queriesAtPack;
  const delayPack = async (route) => {
    if (++packRequests === 1) {
      queriesAtPack = await queries();
      await route.fulfill({ status: 503 });
      return;
    }
    if (packRequests === 2) {
      await releasePack.promise;
    }
    await route.fallback();
  };
  await page.route(pack, delayPack);
  try {
    // Activate without focusing or hovering the opener so the injected failure is on demand.
    await (await trafficOpener()).evaluate((button) => button.click());
    await dialog.waitFor({ state: "visible" });
    const retry = dialog.getByRole("button", { name: "Try again", exact: true });
    await retry.waitFor({ state: "visible" });
    assert.deepEqual(queriesAtPack, [{ offset: 0, search: "", selectedIds: ["first"] }]);
    assert.match(await dialog.getByRole("alert").textContent(), /could not be loaded/);
    await Promise.all([page.waitForRequest(pack), retry.press("Enter")]);
    await dialog
      .getByText("Loading response picker…", { exact: true })
      .waitFor({ state: "visible" });
    const response = page.waitForResponse((response) => response.url().includes("/_webui/packs/"));
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    const opener = traffic
      .locator(".selection-actions")
      .getByRole("button", { name: "Actions", exact: true });
    assert.equal(
      await opener.evaluate((button) => button.getRootNode().activeElement === button),
      true,
    );
    releasePack.resolve();
    await response;
    assert.deepEqual(await queries(), queriesAtPack, "Pack retry repeated the source query");
    await page.evaluate(async () => {
      await document.querySelector("app-shell").$whenRendered();
    });
    assert.equal(
      await dialog.locator(".intake-content").count(),
      0,
      "A canceled pack mounted the picker",
    );
  } finally {
    releasePack.resolve();
    await page.unroute(pack, delayPack);
    // Chromium logs the deliberate network failure even when the picker handles it.
    for (let index = errors.length - 1; index >= errorsBeforeFailure; index--) {
      if (
        errors[index] ===
        "Failed to load resource: the server responded with a status of 503 (Service Unavailable)"
      ) {
        errors.splice(index, 1);
      }
    }
  }
  assert.deepEqual(
    errors.slice(errorsBeforeFailure),
    [],
    "Handled pack failure escaped the picker",
  );
  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.failIntakeTrafficQuery = true;
    fixture.sessions.push(
      ...Array.from({ length: 100 }, (_, index) => ({
        ...fixture.sessions[0],
        id: "intake-pagination-" + index,
        requestNumber: String(index + 10),
      })),
    );
  });
  await openTraffic({ keyboard: true });
  await dialog.getByRole("grid", { name: "Responses to collect" }).waitFor({ state: "visible" });
  assert.equal(
    await dialog.getByRole("searchbox").evaluate((box) => box.getRootNode().activeElement === box),
    true,
  );
  const retry = dialog.getByRole("button", { name: "Try again", exact: true });
  await retry.waitFor({ state: "visible" });
  assert.match(await dialog.getByRole("alert").textContent(), /temporarily unavailable/);
  assert.equal(
    await dialog.getByText("No eligible responses match.", { exact: false }).isVisible(),
    false,
  );
  await dialog.getByLabel("Response set", { exact: true }).selectOption("__new");
  const name = dialog.getByLabel("Response set name", { exact: true });
  await name.fill("Draft name while sources are unavailable");
  await retry.press("Enter");
  await idle();
  assert.equal(await name.inputValue(), "Draft name while sources are unavailable");
  await dialog.getByRole("button", { name: "Load more responses", exact: true }).waitFor();
  assert.equal(
    await dialog.locator("#intake-source-first").getByRole("checkbox").isChecked(),
    true,
  );
  const detailCalls = () => page.evaluate(() => globalThis.__workspaceFixture.calls.session_detail);
  const detailCount = await detailCalls();
  const queryCount = (await queries()).length;
  await dialog.getByRole("searchbox").evaluate((search) => {
    const picker = document.querySelector("app-shell").responseSetIntake;
    search.value = "no-matching-traffic";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    // Race the search debounce with pagination from the previous results.
    return picker.loadSources(true);
  });
  await page.waitForFunction(
    () =>
      globalThis.__workspaceFixture.intakeTrafficQueries.at(-1).search === "no-matching-traffic",
  );
  await idle();
  assert.deepEqual((await queries()).slice(queryCount), [
    { offset: 0, search: "no-matching-traffic", selectedIds: ["first"] },
  ]);
  assert.equal(await dialog.locator(".intake-source-list tbody tr").count(), 1);
  assert.equal(
    await dialog.locator("#intake-source-first").getByRole("checkbox").isChecked(),
    true,
  );
  assert.equal(await detailCalls(), detailCount, "Picker search issued per-row detail requests");
  const searchbox = dialog.getByRole("searchbox");
  const pinned = dialog.locator("#intake-source-first").getByRole("checkbox");
  await pinned.press("Space");
  await pinned.waitFor({ state: "detached" });
  assert.equal(await searchbox.evaluate((box) => box.getRootNode().activeElement === box), true);
  await searchbox.fill("");
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });

  await requestIntake({ source: "traffic", ids: ["first"], destinationId: "removed-set" });
  await dialog.waitFor({ state: "visible" });
  await idle();
  assert.match(
    await dialog.getByRole("alert").textContent(),
    /response set is no longer available/,
  );
  assert.equal(await dialog.getByLabel("Response set", { exact: true }).inputValue(), "");
  assert.equal(await pinned.isChecked(), true);
  assert.equal(await dialog.locator(".intake-source-list tbody tr").count(), 101);
  await page.evaluate(() => (globalThis.__workspaceFixture.failIntakeTrafficQuery = true));
  await searchbox.fill("failed-filter");
  await idle();
  assert.match(await dialog.getByRole("alert").textContent(), /temporarily unavailable/);
  assert.equal(await dialog.locator(".intake-source-list tbody tr").count(), 1);
  assert.equal(await pinned.isChecked(), true);
  await searchbox.fill("");
  await idle();
  assert.equal(await dialog.locator(".intake-source-list tbody tr").count(), 101);
  await page.evaluate(() => (globalThis.__workspaceFixture.deferIntakeTrafficQuery = true));
  assert.equal(
    await searchbox.evaluate((input) => {
      const checkbox = input
        .getRootNode()
        .getElementById("intake-source-first")
        .querySelector("input");
      input.value = "still-no-matching-traffic";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      // The same-turn click can run before the rendered disabled attribute changes.
      checkbox.click();
      return checkbox.checked;
    }),
    true,
    "A blocked checkbox click changed the native checked state",
  );
  await page.waitForFunction(
    () => typeof globalThis.__workspaceFixture.releaseIntakeTrafficQuery === "function",
  );
  assert.equal(await pinned.isDisabled(), true, "Rows are selectable while loading");
  await dialog.getByRole("button", { name: "Clear selection", exact: true }).click();
  assert.equal(await searchbox.evaluate((box) => box.getRootNode().activeElement === box), true);
  await page.evaluate(() => globalThis.__workspaceFixture.releaseIntakeTrafficQuery());
  await idle();
  assert.equal(await dialog.locator(".intake-source-list tbody tr").count(), 0);
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });

  await page.evaluate(() => {
    const fixture = globalThis.__workspaceFixture;
    fixture.sessions = fixture.sessions.filter((row) => !row.id.startsWith("intake-pagination-"));
  });
  for (const type of ["keydown", "pointerdown"]) {
    const opened = await page.evaluate(async (type) => {
      const shell = document.querySelector("app-shell"),
        rendered = shell.$whenRendered;
      let release, entered;
      const gate = new Promise((resolve) => (release = resolve)),
        waiting = new Promise((resolve) => (entered = resolve));
      shell.$whenRendered = async () => {
        await rendered.call(shell);
        entered();
        await gate;
      };
      try {
        const request = shell.onResponseSetIntake(
          new CustomEvent("response-set-intake-request", {
            detail: { source: "traffic", ids: ["first"] },
          }),
        );
        if (!(await Promise.race([waiting.then(() => true), request.then(() => false)]))) {
          throw new Error("Intake completed before the render checkpoint");
        }
        document.dispatchEvent(
          type === "keydown"
            ? new KeyboardEvent(type, { key: "Escape", bubbles: true })
            : new PointerEvent(type, { bubbles: true }),
        );
        release();
        await request;
        return shell.responseSetIntake.intakeDialog.open;
      } finally {
        release();
        shell.$whenRendered = rendered;
        shell.responseSetIntake.close();
      }
    }, type);
    assert.equal(opened, false, `${type} did not cancel intake while its workspace mounted`);
  }
  await traffic.locator('tr[data-session-id="first"]').click({ button: "right" });
  await traffic
    .locator("#traffic-entry-menu")
    .getByRole("button", { name: "Details…", exact: true })
    .click();
  const details = traffic.locator(".timing-dialog");
  await details.waitFor({ state: "visible" });
  await details.getByRole("button", { name: "Close", exact: true }).click();
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
  await dialog
    .locator("#intake-source-second")
    .getByRole("checkbox")
    .click({ modifiers: ["Shift"] });
  assert.equal(
    await dialog.getByRole("checkbox", { checked: true }).count(),
    2,
    "A range checkbox click unchecked a row that remained selected",
  );
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
  await requestIntake({
    source: "rules",
    ids: [sourceBefore[0].id, "removed-rule"],
    destinationId: destination,
  });
  await idle();
  const nativeReviews = () =>
    page.evaluate(() => globalThis.__workspaceFixture.calls.review_response_set_intake);
  const originalReviews = await nativeReviews();
  await dialog.getByRole("button", { name: "Review responses", exact: true }).click();
  assert.match(
    await dialog.getByRole("alert").textContent(),
    /selected responses are no longer available/,
  );
  assert.equal(await nativeReviews(), originalReviews, "Review submitted a partial selection");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
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
