import assert from "node:assert/strict";
import { resolve } from "node:path";
import { waitForBodyText } from "./check-body-source.mjs";

/** Replay controls keep the editable draft separate from retained send evidence. */
export async function checkComposerIterations(page, root) {
  await page.goto("https://workspace.test/");
  await page.locator('app-shell a[data-view="composer"]').click();
  await page.waitForFunction(
    () => typeof document.querySelector("app-shell").composer?.populateRequest === "function",
  );
  const workspace = page.locator("composer-workspace"),
    form = workspace.locator(".composer-request"),
    url = form.locator('[name="url"]'),
    body = form.locator('[name="body"]'),
    inspector = workspace.locator("message-inspector"),
    evidenceTabs = workspace.locator(".composer-message-tabs"),
    history = workspace.locator(".composer-history"),
    calls = () => page.evaluate(() => globalThis.__workspaceFixture.calls.execute_composer ?? 0),
    settled = () =>
      page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);
  const load = async (source = {}) => {
    await page.evaluate(async (source) => {
      const state = globalThis.__workspaceFixture,
        composer = document.querySelector("app-shell").composer;
      state.deferComposer = false;
      state.composerResultSource = { entryId: "first", traceId: null, originalId: null };
      state.composerSourceOverride = {
        method: "GET",
        url: "https://example.test/iteration?first=%2f&first=%2F&flag#kept",
        headers: [],
        body: "",
        bodyAvailable: true,
        notices: [],
        ...source,
      };
      composer.composerDirty = false;
      await composer.populateRequest({ id: "first" });
    }, source);
  };

  // The keyboard shortcut enters the same native validation/acknowledgment path.
  await load({
    method: "POST",
    headers: [{ name: "Authorization", value: "explicit credential" }],
  });
  let before = await calls();
  await url.press("Control+Enter");
  assert.equal(await calls(), before);
  await form.locator('[name="nonIdempotent"]').check();
  await url.press("Control+Enter");
  assert.equal(await calls(), before);
  await form.locator('[name="credentials"]').check();
  await url.press("Control+Enter");
  await settled();
  assert.equal(await calls(), before + 1);
  const enabled = form.getByLabel("Enable header 1", { exact: true });
  await enabled.uncheck();
  assert.equal(await form.locator('[name="credentials"]').isVisible(), false);
  await enabled.check();
  assert.equal(await form.locator('[name="credentials"]').isChecked(), false);

  // Structured input cannot evade the URL limit or truncate the draft.
  await load({
    headers: [
      { name: "Content-Length", value: "999" },
      { name: "Transfer-Encoding", value: "chunked" },
      { name: "X-Duplicate", value: "one" },
      { name: "X-Duplicate", value: "two" },
    ],
    body: "73 65 6e 74 20 62 79 74 65 73",
  });
  const query = form.locator("query-editor");
  if (!(await query.locator("details").evaluate((details) => details.open)))
    await query.locator("summary").click();
  const queryValue = query.getByLabel("Query parameter value 2", { exact: true });
  await queryValue.fill("x".repeat(9000));
  const tooLong = await url.inputValue();
  assert.ok(tooLong.length > 8192);
  before = await calls();
  await queryValue.press("Control+Enter");
  assert.equal(await calls(), before);
  assert.equal(await url.inputValue(), tooLong);
  assert.match(await url.evaluate((input) => input.validationMessage), /8,192 character limit/);
  await queryValue.fill("changed / value");
  assert.equal(await url.evaluate((input) => input.checkValidity()), true);
  const sentUrl = await url.inputValue();
  await queryValue.press("Control+Enter");
  await settled();
  assert.equal(await calls(), before + 1);
  assert.equal(await form.locator(".composer-headers > .field-help").isVisible(), true);
  await workspace
    .locator(".composer-result-stats")
    .getByText(/125 ms · \d+ B encoded/)
    .waitFor();
  const savedId = await workspace.evaluate((composer) => composer.composerResult.id);
  await url.fill("https://example.test/newer-draft");
  await body.fill("newer body draft");
  await evidenceTabs.getByRole("button", { name: "Prepared request", exact: true }).click();
  await evidenceTabs
    .locator("button[data-selected]")
    .getByText("Prepared request", { exact: true })
    .waitFor();
  assert.equal(
    await evidenceTabs
      .getByRole("button", { name: "Prepared request", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await evidenceTabs
      .getByRole("button", { name: "Response", exact: true })
      .getAttribute("aria-pressed"),
    "false",
  );
  await workspace
    .getByText(/transport-generated headers and protocol framing were not observed/)
    .waitFor();
  await inspector.locator(".headers-table").getByText("content-length", { exact: true }).waitFor();
  assert.match(await inspector.locator(".headers-table").textContent(), /content-length10/);
  assert.doesNotMatch(
    await inspector.locator(".headers-table").textContent(),
    /Transfer-Encoding|999/,
  );
  assert.equal(
    await inspector.locator(".headers-table").getByText("X-Duplicate", { exact: true }).count(),
    2,
  );
  assert.equal(
    await workspace.locator(".composer-result-header strong").textContent(),
    `GET ${sentUrl}`,
  );
  await inspector.getByRole("button", { name: "Body", exact: true }).click();
  await waitForBodyText(inspector, "sent bytes");
  await evidenceTabs.getByRole("button", { name: "Response", exact: true }).click();
  await waitForBodyText(inspector, "<script>");
  assert.equal(await body.inputValue(), "newer body draft");

  // Opening an earlier response leaves the dirty draft and its focus in place.
  assert.equal(await history.evaluate((details) => details.open), false);
  await history.getByText("History", { exact: true }).click();
  const row = history.locator(".composer-history-list > div").first();
  const selectRow = row.locator(".composer-history-select"),
    rowActions = row.locator(".composer-history-actions"),
    actionsMenu = workspace.locator(".composer-actions-menu");
  const historyAction = async (name) => {
    await rowActions.click();
    await actionsMenu.getByRole("menuitem", { name, exact: true }).click();
  };
  await selectRow.click();
  await workspace.getByText("Earlier replay", { exact: true }).waitFor();
  assert.equal(await selectRow.getAttribute("aria-pressed"), "true");
  assert.equal(await row.getAttribute("data-selected"), "");
  const olderId = await workspace.evaluate((composer) => composer.composerHistory[1].id);
  await selectRow.press("ArrowDown");
  await page.waitForFunction(
    (id) => document.querySelector("app-shell").composer.resultHistoryId === id,
    olderId,
  );
  await history.locator(".composer-history-select[aria-pressed='true']").press("Home");
  await page.waitForFunction(
    (id) => document.querySelector("app-shell").composer.resultHistoryId === id,
    savedId,
  );
  assert.equal(await url.inputValue(), "https://example.test/newer-draft");
  assert.equal(await body.inputValue(), "newer body draft");
  assert.equal(await workspace.locator(".unsaved-rule-dialog").isVisible(), false);
  const rowBounds = await row.boundingBox();
  await rowActions.click();
  assert.deepEqual(await row.boundingBox(), rowBounds, "Actions expanded the history layout");
  await actionsMenu.press("Escape");
  assert.equal(await actionsMenu.isVisible(), false);
  assert.equal(
    await rowActions.evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  await rowActions.click();
  await page.keyboard.press("Tab");
  assert.equal(await actionsMenu.isVisible(), false);
  await historyAction("Compare with source");
  const comparison = page.getByRole("dialog", {
    name: "Compare requests and responses",
    exact: true,
  });
  await comparison.waitFor({ state: "visible" });
  assert.deepEqual(
    await page.evaluate(() => {
      const input = globalThis.__workspaceFixture.comparisonInput;
      return [input.left, input.right];
    }),
    [
      { kind: "exchange", id: "first" },
      {
        kind: "exchange",
        id: await workspace.evaluate((composer) => composer.composerResult.detail.id),
      },
    ],
  );
  await comparison.press("Escape");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.composerHistoryRequestUnavailable = true;
  });
  await historyAction("Edit again");
  await history
    .getByText("Replay request could not be loaded: Replay evidence expired", { exact: true })
    .waitFor();
  assert.equal(await workspace.locator(".unsaved-rule-dialog").isVisible(), false);
  assert.equal(await body.inputValue(), "newer body draft");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.composerHistoryRequestUnavailable = false;
  });
  await historyAction("Edit again");
  await workspace
    .locator(".unsaved-rule-dialog")
    .getByRole("button", { name: "Keep editing", exact: true })
    .click();
  assert.equal(await body.inputValue(), "newer body draft");
  await historyAction("Edit again");
  await workspace
    .locator(".unsaved-rule-dialog")
    .getByRole("button", { name: "Replace draft", exact: true })
    .click();
  await page.waitForFunction(
    (sentUrl) =>
      document.querySelector("app-shell").composer.composerForm.elements.namedItem("url").value ===
      sentUrl,
    sentUrl,
  );
  assert.equal(await body.inputValue(), "sent bytes");

  // Delayed source lookups cannot navigate after the user resumes editing or leaves.
  for (const [sourceAction, supersedingAction] of [
    ["history", "focus"],
    ["draft", "focus"],
    ["history", "workspace"],
    ["draft", "workspace"],
  ]) {
    await page.evaluate((sourceAction) => {
      const state = globalThis.__workspaceFixture,
        invoke = window.__TAURI_INTERNALS__.invoke,
        composer = document.querySelector("app-shell").composer;
      state.sourceNavigationPending = false;
      state.sourceNavigationSettled = false;
      let held = false;
      window.__TAURI_INTERNALS__.invoke = async (command, args) => {
        const result = await invoke(command, args);
        if (command === "session_detail" && !held) {
          held = true;
          state.sourceNavigationPending = true;
          await new Promise((resolve) => (state.finishSourceNavigation = resolve));
        }
        return result;
      };
      state.restoreSourceNavigation = () => {
        window.__TAURI_INTERNALS__.invoke = invoke;
      };
      const navigation =
        sourceAction === "history"
          ? composer.showHistorySource("first")
          : composer.showComposerSource();
      void navigation.finally(() => {
        state.sourceNavigationSettled = true;
      });
    }, sourceAction);
    await page.waitForFunction(() => globalThis.__workspaceFixture.sourceNavigationPending);
    if (supersedingAction === "focus") {
      await form.locator('[name="method"]').focus();
    } else {
      await workspace.evaluate((composer) => {
        composer.view = "traffic";
        composer.view = "composer";
      });
    }
    await page.evaluate(() => globalThis.__workspaceFixture.finishSourceNavigation());
    await page.waitForFunction(() => globalThis.__workspaceFixture.sourceNavigationSettled);
    assert.equal(await workspace.evaluate((composer) => composer.view), "composer");
    if (supersedingAction === "focus") {
      assert.equal(
        await form
          .locator('[name="method"]')
          .evaluate((input) => input.getRootNode().activeElement === input),
        true,
      );
    }
    await page.evaluate(() => globalThis.__workspaceFixture.restoreSourceNavigation());
  }
  await page.evaluate((id) => {
    const state = globalThis.__workspaceFixture;
    state.deferComposerHistoryRequest = true;
    state.composerHistoryRequestPending = false;
    state.composerHistoryRequestSettled = false;
    void document
      .querySelector("app-shell")
      .composer.editHistoryRequest(id)
      .finally(() => (state.composerHistoryRequestSettled = true));
  }, savedId);
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryRequestPending);
  await workspace.evaluate((composer) => composer.showComposerPane("response"));
  await evidenceTabs.getByRole("button", { name: "Response", exact: true }).focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposerHistoryRequest());
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryRequestSettled);
  assert.equal(await workspace.evaluate((composer) => composer.composerPane), "response");
  assert.equal(
    await evidenceTabs
      .getByRole("button", { name: "Response", exact: true })
      .evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  assert.match(
    await workspace.locator(".composer-source-popover .full-value").textContent(),
    /Original Traffic request · first/,
  );
  // A clean draft can be restored after a delayed history read without taking
  // focus back from a field the user reached while it was loading.
  await url.focus();
  await page.evaluate((id) => {
    const state = globalThis.__workspaceFixture;
    state.deferComposerHistoryRequest = true;
    state.composerHistoryRequestPending = false;
    state.composerHistoryRequestSettled = false;
    void document
      .querySelector("app-shell")
      .composer.editHistoryRequest(id)
      .finally(() => (state.composerHistoryRequestSettled = true));
  }, savedId);
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryRequestPending);
  await form.locator('[name="method"]').focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposerHistoryRequest());
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryRequestSettled);
  assert.equal(
    await form
      .locator('[name="method"]')
      .evaluate((input) => input.getRootNode().activeElement === input),
    true,
  );
  assert.equal(await body.inputValue(), "sent bytes");

  // Observed evidence explicitly selects its declared stage, even if client differs.
  await page.evaluate(async (id) => {
    const state = globalThis.__workspaceFixture,
      result = state.composerResults[id],
      upstream = structuredClone(result.detail.requests[0]);
    upstream.boundary = "upstream-request";
    upstream.headers = [
      { name: "X-Observed", value: "actual sent stage", sensitive: false, binary: false },
    ];
    result.detail.requests[0].headers = [
      { name: "X-Client-Only", value: "not sent", sensitive: false, binary: false },
    ];
    result.detail.requests.push(upstream);
    result.sentRequest = {
      evidence: "observed",
      requestBoundary: "upstream-request",
      note: "Observed request sent to the origin for this replay.",
    };
    result.detail.storedBodies.push({
      ...structuredClone(
        result.detail.storedBodies.find((body) => body.boundary === "client-request"),
      ),
      boundary: "upstream-request",
    });
    state.composerBodies[result.detail.id]["upstream-request"] =
      state.composerBodies[result.detail.id]["client-request"];
    await document.querySelector("app-shell").composer.viewHistoryResponse(id);
  }, savedId);
  await evidenceTabs.getByRole("button", { name: "Sent request", exact: true }).click();
  await inspector.locator(".headers-table").getByText("X-Observed", { exact: true }).waitFor();
  assert.doesNotMatch(await inspector.locator(".headers-table").textContent(), /X-Client-Only/);
  assert.equal(await inspector.getByLabel("Message stage", { exact: true }).isVisible(), false);

  // Locally handled replay evidence shows its observed client request without
  // claiming that the proxy sent a request upstream.
  await page.evaluate(async (id) => {
    const state = globalThis.__workspaceFixture,
      result = state.composerResults[id];
    state.composerUpstreamResult = result;
    const local = structuredClone(result);
    local.detail.requests = local.detail.requests.filter(
      (head) => head.boundary === "client-request",
    );
    local.sentRequest = {
      evidence: "observed",
      requestBoundary: "client-request",
      note: "Observed client request retained from this send. Live rules handled the request locally; no upstream request was observed.",
    };
    state.composerResults[id] = local;
    await document.querySelector("app-shell").composer.viewHistoryResponse(id);
  }, savedId);
  await evidenceTabs.getByRole("button", { name: "Observed request", exact: true }).click();
  await inspector.locator(".headers-table").getByText("X-Client-Only", { exact: true }).waitFor();
  assert.doesNotMatch(await inspector.locator(".headers-table").textContent(), /X-Observed/);
  await workspace.getByText(/no upstream request was observed/).waitFor();
  assert.equal(
    await evidenceTabs.getByRole("button", { name: "Sent request", exact: true }).count(),
    0,
  );
  await page.evaluate(async (id) => {
    const state = globalThis.__workspaceFixture;
    state.composerResults[id] = state.composerUpstreamResult;
    delete state.composerUpstreamResult;
    await document.querySelector("app-shell").composer.viewHistoryResponse(id);
  }, savedId);

  // A late history response cannot steal the request pane after a newer edit.
  await evidenceTabs.getByRole("button", { name: "Response", exact: true }).click();
  await page.evaluate((id) => {
    const state = globalThis.__workspaceFixture;
    state.deferComposerHistory = true;
    state.composerHistoryPending = false;
    state.composerHistorySettled = false;
    void document
      .querySelector("app-shell")
      .composer.viewHistoryResponse(id)
      .finally(() => (state.composerHistorySettled = true));
  }, savedId);
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryPending);
  await evidenceTabs.getByRole("button", { name: "Sent request", exact: true }).click();
  await workspace.evaluate((composer) => composer.showComposerPane("request"));
  await body.fill("edited during history loading");
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposerHistory());
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistorySettled);
  assert.equal(await workspace.evaluate((composer) => composer.composerPane), "request");
  assert.equal(await body.inputValue(), "edited during history loading");
  assert.equal(await workspace.evaluate((composer) => composer.resultMessage), "request");
  await page.evaluate((id) => {
    const state = globalThis.__workspaceFixture;
    state.composerHistoryPending = false;
    state.composerHistorySettled = false;
    void document
      .querySelector("app-shell")
      .composer.viewHistoryResponse(id)
      .finally(() => (state.composerHistorySettled = true));
  }, savedId);
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistoryPending);
  await form.locator('[name="method"]').focus();
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposerHistory());
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerHistorySettled);
  assert.equal(await workspace.evaluate((composer) => composer.composerPane), "request");
  assert.equal(
    await form
      .locator('[name="method"]')
      .evaluate((input) => input.getRootNode().activeElement === input),
    true,
  );

  // A completed send cannot hide a compact draft field reached while waiting.
  await page.setViewportSize({ width: 800, height: 600 });
  for (const field of ["method", "url"]) {
    await load({ url: `https://example.test/send-focus-${field}` });
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      state.deferComposer = true;
      state.composerPending = false;
    });
    const editVersion = await workspace.evaluate((composer) => composer.editVersion);
    await form.getByRole("button", { name: "Send request", exact: true }).click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
    await form.locator(`[name="${field}"]`).focus();
    await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
    await settled();
    assert.equal(await workspace.evaluate((composer) => composer.composerPane), "request");
    assert.equal(await workspace.evaluate((composer) => composer.editVersion), editVersion);
    assert.equal(
      await form
        .locator(`[name="${field}"]`)
        .evaluate((input) => input.getRootNode().activeElement === input),
      true,
    );
  }
  // Ordinary pointer/keyboard sends still reveal the completed response.
  for (const trigger of ["pointer", "keyboard"]) {
    await load({ url: `https://example.test/send-owned-${trigger}` });
    await page.evaluate(() => {
      const state = globalThis.__workspaceFixture;
      state.deferComposer = true;
      state.composerPending = false;
    });
    if (trigger === "pointer")
      await form.getByRole("button", { name: "Send request", exact: true }).click();
    else await url.press("Control+Enter");
    await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
    await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
    await settled();
    assert.equal(await workspace.evaluate((composer) => composer.composerPane), "response");
  }
  await page.setViewportSize({ width: 1280, height: 800 });

  // Cancellation may settle the send before the cancellation RPC returns.
  await load({ url: "https://example.test/cancel" });
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture,
      invoke = window.__TAURI_INTERNALS__.invoke;
    state.deferComposer = true;
    state.composerPending = false;
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await invoke(command, args);
      if (command === "cancel_composer")
        await new Promise((resolve) => (state.finishComposerCancel = resolve));
      return result;
    };
    state.restoreComposerCancel = () => {
      window.__TAURI_INTERNALS__.invoke = invoke;
    };
  });
  await url.press("Control+Enter");
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  await workspace
    .locator(".composer-send")
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await settled();
  await workspace
    .locator(".composer-feedback")
    .getByText(
      "Request canceled. The server may already have received it. Your draft is unchanged.",
      { exact: true },
    )
    .waitFor();
  assert.equal(await workspace.evaluate((composer) => composer.composerError), "");
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.finishComposerCancel();
    state.restoreComposerCancel();
    state.deferComposer = false;
    state.deferComposerHistory = false;
  });
  await url.press("Control+Enter");
  await settled();
  await workspace
    .locator(".composer-feedback")
    .getByText("Response received: HTTP 201", { exact: true })
    .waitFor();
  // Failed cancellation leaves the send monitored and restores its eventual success.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture,
      invoke = window.__TAURI_INTERNALS__.invoke;
    state.deferComposer = true;
    state.composerPending = false;
    window.__TAURI_INTERNALS__.invoke = (command, args) => {
      if (command === "cancel_composer")
        return Promise.reject(new Error("Fixture cancellation unavailable"));
      return invoke(command, args);
    };
    state.restoreComposerCancel = () => {
      window.__TAURI_INTERNALS__.invoke = invoke;
    };
  });
  await url.press("Control+Enter");
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  await workspace
    .locator(".composer-send")
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await workspace
    .locator(".composer-feedback")
    .getByText("Cancellation could not be requested: Fixture cancellation unavailable", {
      exact: true,
    })
    .waitFor();
  assert.equal(await workspace.evaluate((composer) => composer.composerBusy), true);
  assert.equal(
    await workspace
      .locator(".composer-send")
      .getByRole("button", { name: "Cancel", exact: true })
      .isEnabled(),
    true,
  );
  await evidenceTabs.getByRole("button", { name: "Prepared request", exact: true }).click();
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.finishComposer();
    state.restoreComposerCancel();
    state.deferComposer = false;
  });
  await settled();
  assert.equal(await workspace.evaluate((composer) => composer.composerError), "");
  assert.equal(await workspace.evaluate((composer) => composer.resultMessage), "request");
  await load({
    url: "https://example.test/iteration?first=%2f&first=%2F&flag#kept",
    headers: [
      { name: "Content-Type", value: "application/json" },
      { name: "X-Iteration", value: "complete retained request" },
    ],
    body: "7b 22 72 65 61 64 79 22 3a 74 72 75 65 7d",
  });
  await url.press("Control+Enter");
  await settled();
  await page.emulateMedia({ reducedMotion: "reduce" });
  // Match retained native history: long URLs, large encoded sizes, and a
  // scrolling list must not contribute a minimum width to the whole workspace.
  await workspace.evaluate((composer) => {
    composer.composerHistory = composer.composerHistory.map((row, index) => ({
      ...row,
      label: index === 0 ? row.label + "&long=" + "encoded%20query".repeat(40) : row.label,
      stats: index === 1 ? "2.01 s · 1,048,593 B encoded" : row.stats,
    }));
  });
  const checkResultHeaderGeometry = async () => {
    const overflow = await workspace.evaluate((composer) => {
      const surfaces = [
          composer.querySelector("#composer"),
          composer.querySelector(".composer-page"),
          composer.querySelector(".composer-history-list"),
        ],
        back = composer
          .querySelector(".workspace-header .actions button:last-child")
          .getBoundingClientRect(),
        actions = [...composer.querySelectorAll(".composer-history-actions")].filter(
          (button) => button.getClientRects().length,
        );
      return (
        surfaces.some((surface) => surface.scrollWidth > surface.clientWidth + 1) ||
        back.right > innerWidth ||
        actions.some((button) => button.getBoundingClientRect().right > innerWidth)
      );
    });
    assert.equal(overflow, false, "Composer workspace or history extends past the viewport");
    const bounds = await workspace.locator(".composer-result-header").evaluate((header) => {
      const main = header.querySelector(".composer-result-main").getBoundingClientRect(),
        title = header.querySelector("strong").getBoundingClientRect(),
        summary = header.querySelector(".composer-result-summary").getBoundingClientRect();
      return {
        titleShare: title.width / main.width,
        mainBottom: main.bottom,
        summaryTop: summary.top,
      };
    });
    assert.ok(bounds.titleShare >= 0.5, "The result URL lost most of its row to metadata");
    assert.ok(bounds.summaryTop >= bounds.mainBottom, "Result metadata squeezed into the URL row");
  };
  await page.setViewportSize({ width: 1440, height: 900 });
  await checkResultHeaderGeometry();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-iterations-wide.png"),
  });
  await page.setViewportSize({ width: 960, height: 720 });
  await checkResultHeaderGeometry();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-iterations-compact.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await workspace.evaluate((composer) => composer.showComposerPane("response"));
  await checkResultHeaderGeometry();
  assert.equal(
    await workspace
      .locator(".composer-pane-tabs")
      .getByRole("button", { name: "Result", exact: true })
      .getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await workspace
      .locator(".composer-pane-tabs")
      .getByRole("button", { name: "Draft", exact: true })
      .getAttribute("aria-pressed"),
    "false",
  );
  await evidenceTabs.getByRole("button", { name: "Prepared request", exact: true }).click();
  await evidenceTabs
    .locator("button[data-selected]")
    .getByText("Prepared request", { exact: true })
    .waitFor();
  await inspector
    .locator(".message-tabs button[data-selected]")
    .getByText("Headers", { exact: true })
    .waitFor();
  assert.equal(
    await workspace
      .locator(".composer-response")
      .evaluate((pane) => pane.scrollWidth <= pane.clientWidth + 1),
    true,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-iterations-small.png"),
  });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-iterations-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none", colorScheme: "dark" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-iterations-dark.png"),
  });
  await page.emulateMedia({ colorScheme: "light" });
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.setViewportSize({ width: 1280, height: 800 });
}
