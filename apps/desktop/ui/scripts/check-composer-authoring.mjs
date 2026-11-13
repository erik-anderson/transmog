import assert from "node:assert/strict";
import { checkQueryEditor } from "./check-query-editor.mjs";
import { checkComposerIterations } from "./check-composer-iterations.mjs";
import { resolve } from "node:path";
import { fillHeaderEditor } from "./header-editor-helpers.mjs";

export async function checkComposerAuthoring(page, root) {
  await checkQueryEditor(page, root);
  await page.goto("https://workspace.test/");
  await page.locator('app-shell a[data-view="composer"]').click();
  const workspace = page.locator("composer-workspace"),
    form = workspace.locator(".composer-request"),
    body = form.locator('[name="body"]'),
    bodyView = form.getByLabel("Request body view", { exact: true }),
    headers = form.locator("header-editor"),
    sendButton = form.getByRole("button", { name: "Send request", exact: true }),
    dialog = workspace.getByRole("dialog", { name: "Edit request body", exact: true });
  const load = async (source = {}) => {
    await page.evaluate(async (source) => {
      const w = document.querySelector("app-shell").composer;
      globalThis.__workspaceFixture.composerSourceOverride = {
        method: "GET",
        url: "https://example.test/authored",
        headers: [],
        body: "",
        bodyAvailable: true,
        notices: [],
        ...source,
      };
      w.composerDirty = false;
      await w.populateRequest({ id: "first" });
    }, source);
  };
  const calls = () =>
    page.evaluate(() => globalThis.__workspaceFixture.calls.execute_composer ?? 0);
  const send = async () => {
    const before = await calls();
    await sendButton.click();
    await page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);
    assert.equal(
      await calls(),
      before + 1,
      JSON.stringify(
        await form.evaluate((form) => ({
          error: form.closest("composer-workspace").composerBodyError,
          invalid: Array.from(form.elements)
            .filter((input) => input.validity && !input.validity.valid)
            .map((input) => ({ name: input.name, message: input.validationMessage })),
        })),
      ),
    );
  };
  const sentBytes = () =>
    page.evaluate(() => {
      const request = globalThis.__workspaceFixture.composerInput;
      return request.bodyIsHex
        ? Array.from(request.body.replace(/\s/g, "").match(/../g) ?? [], (hex) => parseInt(hex, 16))
        : Array.from(new TextEncoder().encode(request.body));
    });
  const expand = async () => {
    await form.getByRole("button", { name: "Expand editor", exact: true }).click();
    await dialog.locator(".body-code-host > .monaco-editor").waitFor();
  };
  const editExpanded = (text) =>
    dialog.evaluate((dialog, text) => {
      const editor = dialog.closest("response-body-editor");
      editor.editor.executeEdits("fixture-paste", [
        { range: editor.model.getFullModelRange(), text },
      ]);
    }, text);
  const hex = (text) =>
    Array.from(new TextEncoder().encode(text), (b) => b.toString(16).padStart(2, "0")).join(" ");

  assert.equal(await form.locator('[name="url"]').inputValue(), "");
  assert.equal(
    await form.locator('[name="url"]').getAttribute("placeholder"),
    "https://example.com/",
  );
  await sendButton.click();
  assert.equal(await calls(), 0, "An example URL became a real request destination");
  assert.ok(
    await body.evaluate((el) => el.getBoundingClientRect().height < 140),
    "An empty body occupied the request pane",
  );
  assert.deepEqual(
    await form
      .locator("#composer-methods option")
      .evaluateAll((items) => items.map((item) => item.value)),
    ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"],
  );
  await form.locator('[name="method"]').fill("CUSTOM-METHOD");
  assert.equal(
    await form.locator('[name="method"]').evaluate((input) => input.checkValidity()),
    true,
  );
  assert.equal(await form.locator('[name="nonIdempotent"]').isVisible(), true);

  await load({
    headers: [
      { name: "X-Duplicate", value: "one" },
      { name: "X-Duplicate", value: "" },
    ],
  });
  assert.equal(await headers.getByLabel("Header value 1", { exact: true }).inputValue(), "one");
  assert.equal(await headers.getByLabel("Header value 2", { exact: true }).inputValue(), "");
  const submitted = await calls();
  await workspace.evaluate(async (w) => {
    await Promise.all([
      w.executeComposer(new Event("submit", { cancelable: true })),
      w.executeComposer(new Event("submit", { cancelable: true })),
    ]);
  });
  assert.equal(
    await calls(),
    submitted + 1,
    "Overlapping validation must not send the request twice",
  );
  await headers.getByRole("button", { name: "Edit as text", exact: true }).click();
  const rawHeaders = headers.getByRole("textbox", { name: "Request headers as text", exact: true });
  await rawHeaders.fill("X-Duplicate: one\nX-Duplicate:\nBroken header");
  const beforeInvalid = await calls();
  await sendButton.click();
  assert.equal(await calls(), beforeInvalid, "Malformed headers reached execution");
  assert.match(await headers.locator(".header-editor-error").textContent(), /header|colon|Name/i);
  assert.match(await rawHeaders.inputValue(), /Broken header$/);
  await rawHeaders.fill("X-Duplicate: one\nX-Duplicate:");
  await send();
  assert.deepEqual(await page.evaluate(() => globalThis.__workspaceFixture.composerInput.headers), [
    { name: "X-Duplicate", value: "one" },
    { name: "X-Duplicate", value: "" },
  ]);
  await workspace.getByText("Response for current draft", { exact: true }).waitFor();
  await rawHeaders.fill("Broken header");
  assert.equal(
    await workspace.locator(".composer-feedback").isVisible(),
    false,
    "A previous successful send remained below an invalid newer draft",
  );
  await rawHeaders.fill("X-Duplicate: one\nX-Duplicate:");

  // Captured newlines survive textarea normalization and representation changes.
  const captured = "first\r\nsecond 😀\r\n";
  await load({ body: hex(captured) });
  assert.equal(await bodyView.inputValue(), "text");
  assert.equal(await body.inputValue(), captured.replace(/\r\n/g, "\n"));
  await bodyView.selectOption("hex");
  await bodyView.selectOption("text");
  await send();
  assert.deepEqual(await sentBytes(), Array.from(new TextEncoder().encode(captured)));
  await body.fill("Text 😀 and café");
  await bodyView.selectOption("hex");
  assert.match(await body.inputValue(), /f0 9f 98 80/);
  await bodyView.selectOption("text");
  assert.equal(await body.inputValue(), "Text 😀 and café");
  await send();
  assert.deepEqual(await sentBytes(), Array.from(new TextEncoder().encode("Text 😀 and café")));

  await bodyView.selectOption("hex");
  await body.fill("4");
  await bodyView.selectOption("text");
  assert.equal(await bodyView.inputValue(), "hex");
  assert.equal(await body.inputValue(), "4", "An invalid hex conversion replaced the draft");
  await form.locator(".composer-body-error").waitFor({ state: "visible" });
  const beforeHex = await calls();
  await sendButton.click();
  assert.equal(await calls(), beforeHex, "Incomplete hex reached execution");
  await body.fill("00 ff 80");
  await bodyView.selectOption("text");
  assert.equal(await bodyView.inputValue(), "hex");
  assert.equal(await body.inputValue(), "00 ff 80");
  await send();
  assert.deepEqual(await sentBytes(), [0, 255, 128]);

  // Spaced hex for a supported captured body must remain natively editable.
  await load({ body: "ab".repeat(3 * 1024 * 1024) });
  assert.equal(await bodyView.inputValue(), "hex");
  await body.focus();
  await body.evaluate((input) => input.setSelectionRange(0, 2));
  await page.keyboard.type("cd");
  assert.equal(await body.evaluate((input) => input.value.slice(0, 2)), "cd");

  // Encoded entity bytes stay encoded even if their prefix happens to look textual.
  await load({ headers: [{ name: "Content-Encoding", value: "gzip" }], body: "61 62 63" });
  assert.equal(await bodyView.inputValue(), "hex");
  await bodyView.selectOption("text");
  assert.equal(await bodyView.inputValue(), "hex");
  assert.equal(await body.inputValue(), "61 62 63");
  await send();
  assert.deepEqual(await sentBytes(), [97, 98, 99]);

  // Content codings combine across repeated fields and comma-separated values.
  for (const codings of [["identity", "gzip"], ["gzip", "identity"], ["identity, gzip"]]) {
    await load({
      headers: codings.map((coding, index) => ({
        name: index === 0 ? "Content-Encoding" : "content-encoding",
        value: coding,
      })),
      body: "61 62 63",
    });
    assert.equal(await bodyView.inputValue(), "hex");
    await bodyView.selectOption("text");
    assert.equal(await bodyView.inputValue(), "hex", `${codings} exposed encoded bytes as Text`);
    assert.equal(await body.inputValue(), "61 62 63");
  }
  await load({ headers: [{ name: "Content-Encoding", value: " Identity " }], body: "61 62 63" });
  assert.equal(await bodyView.inputValue(), "text");
  assert.equal(await body.inputValue(), "abc");

  await load({
    headers: [{ name: "Content-Type", value: "application/json" }],
    body: hex('{"items":[1,2],"active":true}'),
  });
  const original = await body.inputValue();
  await expand();
  assert.equal(
    await dialog
      .getByText("Apply to the draft, then save the rule from its properties.", { exact: true })
      .isVisible(),
    false,
  );
  await editExpanded("cancelled edits");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert.equal(await body.inputValue(), original);
  assert.equal(
    await form
      .getByRole("button", { name: "Expand editor", exact: true })
      .evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  await expand();
  await dialog.getByRole("button", { name: "Format", exact: true }).click();
  await page.waitForFunction(
    () => !document.querySelector("app-shell").composer.bodyEditor.bodyFormatting,
  );
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-body-expanded.png"),
  });
  await dialog.getByRole("button", { name: "Apply to draft", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  assert.notEqual(await body.inputValue(), original);
  assert.deepEqual(JSON.parse(await body.inputValue()), JSON.parse(original));
  await expand();
  await editExpanded("obsolete dialog draft");
  await body.evaluate((input) => {
    input.value = "newer inline draft";
    input.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
  });
  await dialog.getByRole("button", { name: "Apply to draft", exact: true }).click();
  await dialog.getByText(/request draft changed while this editor was open/).waitFor();
  assert.equal(await body.inputValue(), "newer inline draft");
  await dialog.press("Escape");
  await dialog.waitFor({ state: "hidden" });

  await load();
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferComposer = true;
    globalThis.__workspaceFixture.composerPending = false;
  });
  await sendButton.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  await body.fill("draft edited while sending");
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
  await page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);
  await workspace
    .getByText("Response for an earlier draft · current request has changed", { exact: true })
    .waitFor();
  assert.equal(await body.inputValue(), "draft edited while sending");
  assert.equal(await workspace.evaluate((w) => w.composerPane), "request");
  assert.equal(
    await workspace.locator(".composer-feedback").isVisible(),
    false,
    "An earlier response left a success message below a newer draft",
  );

  // Keeping a dirty draft must also keep its already pending send observable.
  await load({ url: "https://example.test/kept-draft" });
  await body.fill("kept body");
  await page.evaluate(() => {
    globalThis.__workspaceFixture.composerPending = false;
  });
  await sendButton.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  await workspace.evaluate((w) => {
    void w.populateRequest({ id: "second" });
  });
  const replaceDialog = workspace.locator(".unsaved-rule-dialog");
  await replaceDialog.getByRole("button", { name: "Keep editing", exact: true }).click();
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
  await page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);
  assert.equal(await body.inputValue(), "kept body");
  assert.match(await workspace.evaluate((w) => w.resultRequestLabel), /kept-draft$/);

  // An accepted replacement supersedes both successful and failed late sends.
  for (const fail of [false, true]) {
    await load({ url: "https://example.test/pending" });
    await page.evaluate(() => {
      globalThis.__workspaceFixture.composerPending = false;
    });
    await sendButton.click();
    await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
    await load({ url: "https://example.test/new-draft" });
    const resultBefore = await workspace.evaluate((w) => w.resultRequestLabel);
    await page.evaluate((fail) => {
      globalThis.__workspaceFixture.composerError = fail;
      globalThis.__workspaceFixture.finishComposer();
    }, fail);
    await page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);
    assert.equal(await workspace.evaluate((w) => w.resultRequestLabel), resultBefore);
    assert.equal(await workspace.evaluate((w) => w.composerError), "");
    assert.match(
      await workspace.locator(".composer-feedback").textContent(),
      /Captured request loaded/,
    );
    assert.equal(await form.locator('[name="url"]').inputValue(), "https://example.test/new-draft");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.composerError = false;
    });
  }

  // Completing a captured-body load preserves focus moved during acquisition.
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture,
      original = window.__TAURI_INTERNALS__.invoke;
    state.composerSourceOverride.url = "https://example.test/delayed-source";
    state.composerSourceReady = false;
    window.__TAURI_INTERNALS__.invoke = async (command, args) => {
      const result = await original(command, args);
      if (command === "composer_source") {
        state.composerSourceReady = true;
        await new Promise((resolve) => (state.releaseComposerSource = resolve));
      }
      return result;
    };
    state.restoreComposerInvoke = () => {
      window.__TAURI_INTERNALS__.invoke = original;
    };
    const w = document.querySelector("app-shell").composer;
    w.composerDirty = false;
    void w.populateRequest({ id: "first" });
  });
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerSourceReady);
  await form.locator('[name="method"]').focus();
  await page.evaluate(() => globalThis.__workspaceFixture.releaseComposerSource());
  await page.waitForFunction(() =>
    document
      .querySelector("app-shell")
      .composer.composerForm.elements.namedItem("url")
      .value.endsWith("delayed-source"),
  );
  assert.equal(
    await form
      .locator('[name="method"]')
      .evaluate((input) => input.getRootNode().activeElement === input),
    true,
  );
  await page.evaluate(() => globalThis.__workspaceFixture.restoreComposerInvoke());

  await load({ url: "https://example.test/initial?source=initial" });
  await workspace.locator("query-editor summary").click();
  await workspace.getByRole("textbox", { name: "Query parameter name 1", exact: true }).focus();
  const overlapping = await workspace.evaluate(async (w) => {
    const state = globalThis.__workspaceFixture;
    const editor = w.queryEditor;
    const replace = editor.replaceValue;
    let release, reached;
    const paused = new Promise((resolve) => (reached = resolve));
    const pending = new Promise((resolve) => (release = resolve));
    let held = false;
    editor.replaceValue = function (value) {
      const rendered = replace.call(this, value);
      if (!held) {
        held = true;
        reached();
        return pending.then(() => rendered);
      }
      return rendered;
    };
    state.composerSourceOverride = {
      method: "POST",
      url: "https://example.test/older?source=older",
      headers: [{ name: "X-Source", value: "older" }],
      body: "4f4c44",
      bodyAvailable: true,
      notices: [],
    };
    const older = w.populateRequest({ id: "first" });
    try {
      await paused;
      state.composerSourceOverride = {
        method: "PUT",
        url: "https://example.test/newer?source=newer",
        headers: [{ name: "X-Source", value: "newer" }],
        body: "4e4557",
        bodyAvailable: true,
        notices: [],
      };
      await w.populateRequest({ id: "second" });
      release();
      await older;
      return {
        method: w.composerForm.elements.namedItem("method").value,
        url: w.composerForm.elements.namedItem("url").value,
        headers: w.headerEditor.value,
        body: w.currentBodyBytes(),
      };
    } finally {
      release();
      await older;
      editor.replaceValue = replace;
    }
  });
  assert.deepEqual(
    overlapping,
    {
      method: "PUT",
      url: "https://example.test/newer?source=newer",
      headers: "X-Source: newer",
      body: [78, 69, 87],
    },
    "An older Composer load must not mix its headers or body into the newer request",
  );

  // Editing or changing body representation cancels an in-flight source load.
  // Both success and failure must settle its feedback without touching the draft.
  const deferSource = async (fail, source = {}) => {
    await page.evaluate(
      ({ fail, source }) => {
        const state = globalThis.__workspaceFixture,
          original = window.__TAURI_INTERNALS__.invoke;
        Object.assign(state.composerSourceOverride, source);
        state.composerSourceOverride.url = "https://example.test/obsolete-source";
        state.composerSourceReady = false;
        state.composerSourceSettled = false;
        let held = false;
        window.__TAURI_INTERNALS__.invoke = async (command, args) => {
          const result = await original(command, args);
          if (command === "composer_source" && !held) {
            held = true;
            state.composerSourceReady = true;
            await new Promise((resolve) => (state.releaseComposerSource = resolve));
            if (fail) {
              throw new Error("Obsolete captured request load failed");
            }
          }
          return result;
        };
        state.restoreComposerInvoke = () => {
          window.__TAURI_INTERNALS__.invoke = original;
        };
        void document
          .querySelector("app-shell")
          .composer.populateRequest({ id: "first" })
          .finally(() => (state.composerSourceSettled = true));
      },
      { fail, source },
    );
    await page.waitForFunction(() => globalThis.__workspaceFixture.composerSourceReady);
  };
  const finishSource = async () => {
    await page.evaluate(() => globalThis.__workspaceFixture.releaseComposerSource());
    await page.waitForFunction(() => globalThis.__workspaceFixture.composerSourceSettled);
    await page.evaluate(() => globalThis.__workspaceFixture.restoreComposerInvoke());
  };
  const draftSnapshot = () =>
    form.evaluate((form) => ({
      method: form.elements.namedItem("method").value,
      url: form.elements.namedItem("url").value,
      body: form.elements.namedItem("body").value,
      mode: form.elements.namedItem("bodyView").value,
      headers: form.querySelector("header-editor").captureDraft(),
    }));

  // Acquiring a source still replaces the draft, but a newer compact pane
  // choice retains its focus and stays visible when the source arrives.
  await page.setViewportSize({ width: 800, height: 600 });
  await load({ url: "https://example.test/previous-pane-source" });
  await deferSource(false);
  const responsePaneButton = workspace
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Result", exact: true });
  await responsePaneButton.click();
  await finishSource();
  assert.equal(
    await form.locator('[name="url"]').inputValue(),
    "https://example.test/obsolete-source",
  );
  assert.equal(await workspace.evaluate((w) => w.composerPane), "response");
  assert.equal(
    await responsePaneButton.evaluate((button) => button.getRootNode().activeElement === button),
    true,
  );
  // An ordinary source load still reveals the request when it retains ownership.
  await load({ url: "https://example.test/owned-pane-source" });
  assert.equal(await workspace.evaluate((w) => w.composerPane), "request");
  assert.equal(
    await form
      .locator('[name="url"]')
      .evaluate((input) => input.getRootNode().activeElement === input),
    true,
  );
  await page.setViewportSize({ width: 1280, height: 800 });

  for (const change of ["body", "headers", "method", "view"]) {
    for (const fail of [false, true]) {
      await load({ body: hex("previous draft"), url: "https://example.test/preserved-draft" });
      await deferSource(fail);
      if (change === "body") {
        await body.fill("edited during source loading");
      } else if (change === "headers") {
        await fillHeaderEditor(headers, "X-Edited: during source loading");
      } else if (change === "method") {
        await form.locator('[name="method"]').fill("POST");
      } else {
        await bodyView.selectOption("hex");
      }
      const draft = await draftSnapshot();
      await finishSource();
      assert.deepEqual(await draftSnapshot(), draft, `A stale source changed the ${change} draft`);
      assert.equal(await workspace.evaluate((w) => w.composerError), "");
      assert.match(
        await workspace.locator(".composer-feedback").textContent(),
        /loading stopped\. Your current draft is preserved/,
        `A superseded source ${fail ? "failure" : "success"} left obsolete ${change} feedback`,
      );
    }
  }

  // A successful source replacement may rebuild header rows. Preserve focus
  // moved into those rows while waiting, including a useful empty-list fallback.
  for (const [field, replacementHeaders] of [
    ["Header name 1", [{ name: "X-New", value: "new value" }]],
    ["Header value 1", [{ name: "X-New", value: "new value" }]],
    ["Header value 1", []],
  ]) {
    await load({ headers: [{ name: "X-Old", value: "old value" }] });
    const asRows = headers.getByRole("button", { name: "Edit as rows", exact: true });
    if (await asRows.isVisible()) {
      await asRows.click();
    }
    await form.locator('[name="method"]').focus();
    await deferSource(false, { headers: replacementHeaders });
    await headers.getByLabel(field, { exact: true }).focus();
    await finishSource();
    const focusTarget = replacementHeaders.length
      ? headers.getByLabel(field, { exact: true })
      : headers.getByRole("button", { name: "Add header", exact: true });
    assert.equal(
      await focusTarget.evaluate((input) => input.getRootNode().activeElement === input),
      true,
      `Loading captured headers lost useful focus from ${field}`,
    );
    assert.deepEqual(await headers.evaluate((editor) => editor.getHeaders()), replacementHeaders);
  }

  // Acquiring a source owns progress immediately; it owns the request draft only
  // after committing. Failed or canceled acquisition must not lose its pending send.
  for (const acquisition of ["failed", "canceled", "replaced"]) {
    for (const order of ["send-first", "source-first"]) {
      const url = `https://example.test/pending-${acquisition}-${order}`;
      await load({ url, body: hex("current request body") });
      const previousResponse = await workspace.evaluate((w) => w.resultRequestLabel);
      await page.evaluate(() => {
        globalThis.__workspaceFixture.deferComposer = true;
        globalThis.__workspaceFixture.composerPending = false;
      });
      await sendButton.click();
      await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
      await deferSource(acquisition === "failed");
      if (acquisition === "canceled") {
        await body.fill("newer draft body");
      }
      const finishSend = async () => {
        await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
        await page.waitForFunction(
          () => !document.querySelector("app-shell").composer.composerBusy,
        );
      };
      if (order === "send-first") {
        await finishSend();
        assert.equal(await workspace.evaluate((w) => w.resultRequestLabel), `GET ${url}`);
        assert.match(
          await workspace.locator(".composer-feedback").textContent(),
          /Loading the complete captured request/,
        );
        assert.equal(await workspace.evaluate((w) => w.composerPane), "request");
        await finishSource();
      } else {
        await finishSource();
        const sourceFeedback = await workspace.locator(".composer-feedback").textContent();
        await finishSend();
        assert.equal(await workspace.locator(".composer-feedback").textContent(), sourceFeedback);
      }
      assert.equal(await workspace.evaluate((w) => w.composerPane), "request");
      assert.equal(
        await workspace.evaluate((w) => w.resultRequestLabel),
        acquisition === "replaced" && order === "source-first" ? previousResponse : `GET ${url}`,
      );
      if (acquisition === "failed") {
        assert.equal(await form.locator('[name="url"]').inputValue(), url);
        assert.match(
          await workspace.evaluate((w) => w.composerError),
          /Obsolete captured request load failed/,
        );
        assert.equal(await workspace.evaluate((w) => w.resultMatchesDraft), true);
      } else if (acquisition === "canceled") {
        assert.equal(await body.inputValue(), "newer draft body");
        assert.match(
          await workspace.locator(".composer-feedback").textContent(),
          /loading stopped/,
        );
        assert.equal(await workspace.evaluate((w) => w.resultMatchesDraft), false);
      } else {
        assert.equal(
          await form.locator('[name="url"]').inputValue(),
          "https://example.test/obsolete-source",
        );
        assert.equal(await workspace.evaluate((w) => w.resultMatchesDraft), false);
      }
    }
  }

  // Settling an old load cannot replace a newer request's sending feedback.
  await load({ url: "https://example.test/newer-send" });
  await deferSource(false);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.deferComposer = true;
    globalThis.__workspaceFixture.composerPending = false;
  });
  await sendButton.click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.composerPending);
  await finishSource();
  assert.match(await workspace.locator(".composer-feedback").textContent(), /Sending request/);
  await page.evaluate(() => globalThis.__workspaceFixture.finishComposer());
  await page.waitForFunction(() => !document.querySelector("app-shell").composer.composerBusy);

  // Nor can it replace a newer captured request's completed loading feedback.
  await load({ url: "https://example.test/previous-source" });
  await deferSource(true);
  await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture;
    state.composerSourceOverride.url = "https://example.test/newer-source";
    await document.querySelector("app-shell").composer.populateRequest({ id: "second" });
  });
  await finishSource();
  assert.equal(
    await form.locator('[name="url"]').inputValue(),
    "https://example.test/newer-source",
  );
  assert.match(
    await workspace.locator(".composer-feedback").textContent(),
    /Captured request loaded/,
  );
  assert.equal(await workspace.evaluate((w) => w.composerError), "");

  await page.setViewportSize({ width: 1440, height: 900 });
  await load({
    method: "POST",
    body: hex('{"ready":true}'),
    headers: [
      { name: "Content-Type", value: "application/json" },
      { name: "Accept", value: "application/json" },
      { name: "X-Long-Header", value: "a long value ".repeat(15) },
    ],
  });
  const finalRowsButton = headers.getByRole("button", { name: "Edit as rows", exact: true });
  if (await finalRowsButton.isVisible()) {
    await finalRowsButton.click();
  }
  assert.equal(await form.locator(".composer-headers > .field-help").isVisible(), false);
  const fields = form.locator(".composer-request-fields");
  const checkDraftGeometry = async () => {
    const bounds = await form.evaluate((form) => {
      const fields = form.querySelector(".composer-request-fields"),
        footer = form.querySelector(".composer-send"),
        fieldRect = fields.getBoundingClientRect(),
        footerRect = footer.getBoundingClientRect();
      return {
        overflow: fields.scrollWidth > fields.clientWidth + 1,
        fieldsBottom: fieldRect.bottom,
        footerTop: footerRect.top,
        footerBottom: footerRect.bottom,
        viewport: innerHeight,
      };
    });
    assert.equal(bounds.overflow, false, "Draft fields overflow horizontally");
    assert.ok(bounds.fieldsBottom <= bounds.footerTop + 1, "Send overlaps request fields");
    assert.ok(bounds.footerBottom <= bounds.viewport, "Send leaves the visible window");
    for (const control of [bodyView, body, form.locator('[name="nonIdempotent"]')]) {
      await control.scrollIntoViewIfNeeded();
      assert.equal(
        await control.evaluate((input) => {
          const scroll = input.closest(".composer-request-fields").getBoundingClientRect(),
            rect = input.getBoundingClientRect(),
            top = Math.max(scroll.top, rect.top),
            bottom = Math.min(scroll.bottom, rect.bottom),
            pointY = (top + bottom) / 2,
            pointX = (rect.left + rect.right) / 2,
            hit = input.getRootNode().elementFromPoint(pointX, pointY);
          return (
            bottom - top >= Math.min(10, rect.height) && (hit === input || input.contains(hit))
          );
        }),
        true,
        "A request body control remains obscured after scrolling into view",
      );
    }
    await fields.evaluate((fields) => {
      fields.scrollTop = 0;
    });
  };
  await checkDraftGeometry();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-authoring-wide.png"),
  });
  await page.setViewportSize({ width: 960, height: 720 });
  await checkDraftGeometry();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-authoring-compact.png"),
  });
  await page.setViewportSize({ width: 800, height: 600 });
  await workspace
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Result", exact: true })
    .click();
  assert.equal(await form.isVisible(), false);
  await workspace
    .locator(".composer-pane-tabs")
    .getByRole("button", { name: "Draft", exact: true })
    .click();
  assert.equal(await body.isVisible(), true);
  await checkDraftGeometry();
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-authoring-small.png"),
  });
  await page.emulateMedia({ forcedColors: "active" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/composer-authoring-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none" });
  await page.setViewportSize({ width: 1440, height: 900 });
  const priorZoom = await page.evaluate(() => {
    const value = document.documentElement.style.zoom;
    document.documentElement.style.zoom = "2";
    return value;
  });
  try {
    assert.equal(await form.evaluate((el) => el.scrollWidth <= el.clientWidth + 1), true);
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/composer-authoring-zoom.png"),
    });
    await fields.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/composer-authoring-zoom-body.png"),
    });
  } finally {
    await page.evaluate((zoom) => {
      document.documentElement.style.zoom = zoom;
    }, priorZoom);
  }
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => {
    delete globalThis.__workspaceFixture.composerSourceOverride;
    globalThis.__workspaceFixture.deferComposer = false;
    globalThis.__workspaceFixture.composerError = false;
  });
  await checkComposerIterations(page, root);
}
