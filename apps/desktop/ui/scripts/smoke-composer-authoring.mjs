import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

/** Exercise the compiled authoring controls and actual loopback replay in WebView2. */
export async function smokeComposerAuthoring(
  viewer,
  composer,
  url,
  screenshot,
  delayedReplayReceived,
) {
  const wait = async (expression) =>
    viewer.evaluate(`(async () => {
      const deadline = Date.now() + 15000;
      while (!(${expression}) && Date.now() < deadline)
        await new Promise(resolve => setTimeout(resolve, 25));
      if (!(${expression})) throw new Error(${JSON.stringify("Composer wait failed: ")} + ${JSON.stringify(expression)});
    })()`);
  const key = async (name, code, modifiers = 0) => {
    for (const type of ["keyDown", "keyUp"])
      await viewer.call("Input.dispatchKeyEvent", {
        type,
        key: name,
        code: name,
        windowsVirtualKeyCode: code,
        modifiers,
        ...(name === "Enter" && !modifiers && type === "keyDown"
          ? { text: "\r", unmodifiedText: "\r" }
          : {}),
      });
  };
  const text = '{"message":"café"}\n';
  const headers =
    "Content-Type: application/json\nX-Repeat: first\nx-repeat: second\nX-Empty: \nX-Disabled: keep this value\nContent-Length: 99\nTransfer-Encoding: chunked";
  const authoredUrl = url + "?tag=one&tag=two&flag&empty=&escaped=%2f+%20";
  await viewer.evaluate(`(() => {
    const workspace = ${composer};
    workspace.showComposerPane('request');
    const field = (name, value) => {
      const input = workspace.composerForm.elements.namedItem(name);
      input.value = value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    field('method', 'POST');
    field('url', ${JSON.stringify(authoredUrl)});
    workspace.headerEditor.value = ${JSON.stringify(headers)};
    workspace.headerEditor.toggleMode();
    if (workspace.headerEditor.headerEditorMode === 'text') workspace.headerEditor.toggleMode();
    workspace.headerEditor.querySelector('[aria-label="Enable header 5"]').click();
    workspace.headerEditor.toggleMode();
    workspace.headerEditor.toggleMode();
    workspace.useEmptyBody();
    const view = workspace.composerForm.elements.namedItem('bodyView');
    view.value = 'text'; view.dispatchEvent(new Event('change', { bubbles: true }));
    field('body', ${JSON.stringify(text)});
    view.value = 'hex'; view.dispatchEvent(new Event('change', { bubbles: true }));
    view.value = 'text'; view.dispatchEvent(new Event('change', { bubbles: true }));
    workspace.composerForm.elements.namedItem('nonIdempotent').checked = true;
  })()`);
  assert.equal(await viewer.evaluate(`${composer}.input('body').value`), text);
  assert.equal(await viewer.evaluate(`${composer}.composerBodyError`), "");
  assert.equal(
    await viewer.evaluate(
      `${composer}.headerEditor.querySelector('[aria-label="Enable header 5"]').checked`,
    ),
    false,
  );
  await viewer.evaluate(`${composer}.querySelector('query-editor summary').click()`);
  assert.equal(await viewer.evaluate(`${composer}.input('url').value`), authoredUrl);
  await viewer.evaluate(`(() => {
    const input = ${composer}.querySelector('query-editor [aria-label="Query parameter value 2"]');
    input.value = 'next value'; input.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  const editedUrl = await viewer.evaluate(`${composer}.input('url').value`);
  assert.deepEqual(new URL(editedUrl).searchParams.getAll("tag"), ["one", "next value"]);
  assert.ok(editedUrl.includes("&flag&empty=&escaped=%2f+%20"));
  assert.deepEqual(await viewer.evaluate(`${composer}.headerEditor.getHeaders().slice(1, 4)`), [
    { name: "X-Repeat", value: "first" },
    { name: "x-repeat", value: "second" },
    { name: "X-Empty", value: "" },
  ]);
  const previousId = await viewer.evaluate(`${composer}.composerResult?.id ?? null`);
  await viewer.evaluate(`${composer}.input('body').focus()`);
  await key("Enter", 13, 2);
  await wait(
    `!${composer}.composerBusy && ${composer}.composerResult?.id !== ${JSON.stringify(previousId)}`,
  );
  const result = await viewer.evaluate(`({
    error: ${composer}.composerError,
    status: ${composer}.composerResult?.status,
    body: ${composer}.composerResult?.body,
    matches: ${composer}.resultMatchesDraft,
    id: ${composer}.composerResult?.id,
    elapsed: ${composer}.composerResult?.elapsedMs,
    responseBytes: ${composer}.composerResult?.responseBodyBytes
  })`);
  assert.equal(result.error, "");
  assert.equal(result.status, 200);
  const echoed = JSON.parse(result.body);
  assert.deepEqual(echoed.bytes, Array.from(Buffer.from(text)));
  assert.deepEqual(echoed.repeated, ["first", "second"]);
  assert.equal(echoed.empty, "");
  assert.equal(echoed.disabled, null);
  assert.equal(echoed.url, new URL(editedUrl).pathname + new URL(editedUrl).search);
  assert.equal(result.matches, true);
  assert.ok(Number.isFinite(result.elapsed) && result.elapsed >= 0);
  assert.equal(result.responseBytes, Buffer.byteLength(result.body));
  await viewer.evaluate(`${composer}.showResultMessage('request')`);
  await wait(`${composer}.querySelector('message-inspector[side="request"]')?.detail != null`);
  assert.equal(await viewer.evaluate(`${composer}.resultRequestTitle`), "Prepared request");
  const prepared = await viewer.evaluate(`${composer}.composerResult.detail.requests[0].headers`);
  assert.ok(!prepared.some((header) => header.name.toLowerCase() === "x-disabled"));
  assert.ok(!prepared.some((header) => header.name.toLowerCase() === "transfer-encoding"));
  assert.equal(
    prepared.find((header) => header.name.toLowerCase() === "content-length")?.value,
    String(Buffer.byteLength(text)),
  );
  await viewer.evaluate(`(() => {
    const workspace = ${composer};
    workspace.showComposerPane('request');
    const value = workspace.headerEditor.querySelector('textarea[aria-label="Header value 2"]');
    value.value = 'edited'; value.dispatchEvent(new Event('input', { bubbles: true }));
  })()`);
  assert.equal(await viewer.evaluate(`${composer}.resultMatchesDraft`), false);
  const historyWasOpen = await viewer.evaluate(
    `${composer}.querySelector('.composer-history').open`,
  );
  if (!historyWasOpen) {
    await viewer.evaluate(`${composer}.querySelector('.composer-history > summary').click()`);
  }
  await wait(
    `!${composer}.composerHistoryBusy && ${composer}.querySelector('#composer-history-${result.id}') != null`,
  );
  const historyFocus = await viewer.evaluate(`(() => {
    const workspace = ${composer};
    const row = workspace.querySelector('#composer-history-${result.id}');
    row.scrollIntoView({ block: 'nearest' });
    row.focus();
    const rect = row.getBoundingClientRect();
    const listRect = row.closest('.composer-history-list').getBoundingClientRect();
    return {
      focused: row.getRootNode().activeElement === row,
      enabled: !row.disabled,
      visible: workspace.querySelector('.composer-history').open && rect.width > 0 && rect.height > 0 && rect.bottom > listRect.top && rect.top < listRect.bottom
    };
  })()`);
  assert.deepEqual(
    historyFocus,
    { focused: true, enabled: true, visible: true },
    "History Enter requires an enabled, visible, focused attempt",
  );
  await key("Enter", 13);
  await wait(`${composer}.resultFromHistory`);
  assert.equal(
    await viewer.evaluate(
      `${composer}.querySelector('#composer-history-${result.id}').getAttribute('aria-pressed')`,
    ),
    "true",
  );
  await viewer.evaluate(
    `${composer}.querySelector('.composer-history-row[data-selected] .composer-history-actions').click()`,
  );
  await wait(`${composer}.composerActionsMenu.matches(':popover-open')`);
  await key("Escape", 27);
  assert.equal(
    await viewer.evaluate(`${composer}.composerActionsMenu.matches(':popover-open')`),
    false,
  );
  assert.equal(
    await viewer.evaluate(
      `${composer}.querySelector('.composer-history-row[data-selected] .composer-history-actions').matches(':focus')`,
    ),
    true,
  );
  if (!historyWasOpen) {
    await viewer.evaluate(`${composer}.querySelector('.composer-history > summary').click()`);
  }
  assert.equal(
    await viewer.evaluate(`${composer}.headerEditor.getHeaders()[1].value`),
    "edited",
    "Opening a historical result replaced the draft",
  );
  await viewer.evaluate(`void ${composer}.editHistoryRequest(${result.id})`);
  await wait(`${composer}.replaceComposerDialog.open`);
  await viewer.evaluate(`${composer}.resolveComposerReplacement(true)`);
  await wait(`${composer}.composerText.startsWith('Replay request loaded.')`);
  assert.equal(await viewer.evaluate(`${composer}.headerEditor.getHeaders()[1].value`), "first");
  assert.equal(await viewer.evaluate(`${composer}.input('nonIdempotent').checked`), false);
  assert.equal(await viewer.evaluate(`${composer}.input('url').value`), editedUrl);
  await viewer.evaluate(`(() => {
    const workspace = ${composer};
    workspace.showComposerPane('request');
    const field = workspace.input('url');
    field.value = ${JSON.stringify(url + "?delay=1")};
    field.dispatchEvent(new Event('input', { bubbles: true }));
    workspace.input('nonIdempotent').checked = true;
    workspace.input('body').focus();
  })()`);
  await key("Enter", 13, 2);
  await wait(`${composer}.composerBusy`);
  let timer;
  try {
    await Promise.race([
      delayedReplayReceived,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error("The delayed request never reached the loopback server")),
          15000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
  await viewer.evaluate(`(async () => { await ${composer}.cancelComposer(); })()`);
  await wait(`!${composer}.composerBusy`);
  assert.match(await viewer.evaluate(`${composer}.composerText`), /cancel/i);
  assert.equal(await viewer.evaluate(`${composer}.composerResult.id`), result.id);
  await viewer.call("Emulation.setDeviceMetricsOverride", {
    width: 800,
    height: 600,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await viewer.evaluate(`${composer}.input('body').focus()`);
  await key("Enter", 13, 2);
  await wait(`${composer}.composerBusy`);
  await viewer.evaluate(`${composer}.input('method').focus()`);
  await wait(`!${composer}.composerBusy && ${composer}.composerResult.id !== ${result.id}`);
  assert.equal(await viewer.evaluate(`${composer}.composerPane`), "request");
  assert.equal(await viewer.evaluate(`${composer}.input('method').matches(':focus')`), true);
  await viewer.evaluate(`${composer}.showResultMessage('response')`);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await viewer.call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const bounds = await viewer.evaluate(`(() => {
      const workspace = ${composer};
      const form = workspace.composerForm;
      const fields = form.querySelector('.composer-request-fields');
      fields.scrollTop = 0;
      const send = form.querySelector('button[type="submit"]');
      const rect = send.getBoundingClientRect();
      const fieldRect = fields.getBoundingClientRect();
      const footerRect = form.querySelector('.composer-send').getBoundingClientRect();
      const surfaces = [workspace.querySelector('#composer'), workspace.querySelector('.composer-page'), workspace.querySelector('.composer-history-list')];
      const back = workspace.querySelector('.workspace-header .actions button:last-child').getBoundingClientRect();
      const actions = [...workspace.querySelectorAll('.composer-history-actions')].filter(button => button.getClientRects().length).map(button => button.getBoundingClientRect().right);
      return { overflow: fields.scrollWidth > fields.clientWidth + 2, workspaceOverflow: surfaces.some(surface => surface.scrollWidth > surface.clientWidth + 1), backRight: back.right, actionRight: Math.max(0,...actions), viewportWidth: innerWidth, top: rect.top, bottom: rect.bottom, width: rect.width, height: rect.height, fieldsBottom: fieldRect.bottom, footerTop: footerRect.top, viewport: innerHeight };
    })()`);
    assert.equal(bounds.overflow, false, `Composer overflows horizontally at ${width}px`);
    assert.equal(
      bounds.workspaceOverflow,
      false,
      `Composer workspace/history overflow at ${width}px`,
    );
    assert.ok(
      bounds.backRight <= bounds.viewportWidth && bounds.actionRight <= bounds.viewportWidth,
      `Composer actions extend past the viewport at ${width}px`,
    );
    assert.ok(
      bounds.top >= 0 && bounds.bottom <= bounds.viewport && bounds.width > 0 && bounds.height > 0,
      `Send is unreachable at ${width}px`,
    );
    assert.ok(
      bounds.fieldsBottom <= bounds.footerTop + 1,
      `Send overlaps draft controls at ${width}px`,
    );
    assert.equal(
      await viewer.evaluate(`(() => {
      const workspace = ${composer};
      const input = workspace.input('body');
      input.scrollIntoView({ block: 'nearest' });
      const fieldRect = input.closest('.composer-request-fields').getBoundingClientRect();
      const rect = input.getBoundingClientRect();
      const top = Math.max(fieldRect.top, rect.top), bottom = Math.min(fieldRect.bottom, rect.bottom);
      const hit = input.getRootNode().elementFromPoint((rect.left + rect.right) / 2, (top + bottom) / 2);
      return bottom - top >= 10 && hit === input;
    })()`),
      true,
      `Body remains obscured after scrolling at ${width}px`,
    );
    assert.equal(
      await viewer.evaluate(`(() => {
      const workspace = ${composer};
      const section = workspace.querySelector('#composer');
      const page = workspace.querySelector('.composer-page');
      const back = workspace.querySelector('.workspace-header .actions button:last-child').getBoundingClientRect();
      return section.scrollLeft === 0 && section.scrollWidth <= section.clientWidth + 1 && page.scrollWidth <= page.clientWidth + 1 && back.right <= innerWidth;
    })()`),
      true,
      `Scrolling the body displaced the Composer workspace at ${width}px`,
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", `-authoring-${width}.png`),
        Buffer.from(image.data, "base64"),
      );
    }
  }
  await viewer.call("Emulation.clearDeviceMetricsOverride");
  process.stdout.write(
    "Native query/header editing, keyboard send, history, prepared request, cancellation and compact layout verified.\n",
  );
}
