import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

// The caller owns the isolated native executable, profile, CDP endpoint and process.
export async function smokeResponseSetIntake({ main, waitFor, directory }) {
  const output = resolve(directory, "intake");
  await mkdir(output, { recursive: true });
  const shell = "document.querySelector('app-shell')",
    traffic = shell + ".traffic",
    a = shell + ".automation",
    sets = a + ".responseSets",
    picker = shell + ".responseSetIntake",
    dialog = picker + ".intakeDialog";
  const invoke = (command, args = {}) =>
    main.evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`,
    );
  const button = (container, text) =>
    `[...${container}.querySelectorAll('button')].find(node=>node.textContent.trim()===${JSON.stringify(text)}&&node.getClientRects().length)`;
  const pointer = async (expression, modifiers = 0) => {
    await main.evaluate(`${expression}.scrollIntoView({block:'nearest'})`);
    const point = await main.evaluate(
      `(()=>{const r=${expression}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`,
    );
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"])
      await main.call("Input.dispatchMouseEvent", {
        type,
        button: "left",
        buttons: type === "mousePressed" ? 1 : 0,
        clickCount: 1,
        modifiers,
        ...point,
      });
  };
  const key = async (key, code, vk, modifiers = 0) => {
    for (const type of ["keyDown", "keyUp"])
      await main.call("Input.dispatchKeyEvent", {
        type,
        key,
        code,
        windowsVirtualKeyCode: vk,
        modifiers,
        ...(key === "Enter" && type === "keyDown" ? { text: "\r" } : {}),
      });
  };
  const input = async (expression, text) => {
    await pointer(expression);
    await key("a", "KeyA", 65, 2);
    await main.call("Input.insertText", { text });
  };
  const idle = () =>
    waitFor(`!${picker}.intakeLoading&&!${picker}.intakeReviewBusy&&!${picker}.intakeSaving`);
  const acknowledge = async () => {
    if (await main.evaluate(`${picker}.intakeReview.warnings.length>0`))
      await pointer(dialog + ".querySelector('.intake-acknowledge input')");
  };
  await waitFor(`${shell}?.workspaceLoaded`);
  const before = await invoke("automation_status");
  const harPath = resolve(output, "captured-binary.har"),
    bodies = [Buffer.from([0, 255, 13, 10, 65]), Buffer.from('{"captured":"complete"}')];
  const entries = bodies.map((body, index) => ({
    startedDateTime: "2026-10-18T18:00:00Z",
    time: 1,
    request: {
      method: "GET",
      url: "https://native-intake.invalid/captured/" + index,
      httpVersion: "HTTP/1.1",
      headers: [],
      queryString: [],
      cookies: [],
      headersSize: -1,
      bodySize: 0,
    },
    response: {
      status: 200,
      statusText: "OK",
      httpVersion: "HTTP/1.1",
      headers: [
        {
          name: "Content-Type",
          value: index === 0 ? "application/octet-stream" : "application/json",
        },
        { name: "X-Preserved", value: "one" },
        { name: "X-Preserved", value: "two" },
      ],
      cookies: [],
      content: {
        size: body.length,
        mimeType: index === 0 ? "application/octet-stream" : "application/json",
        text: body.toString("base64"),
        encoding: "base64",
      },
      redirectURL: "",
      headersSize: -1,
      bodySize: body.length,
    },
    cache: {},
    timings: { send: 0, wait: 0, receive: 1 },
  }));
  await writeFile(
    harPath,
    JSON.stringify({
      log: {
        version: "1.2",
        creator: { name: "Native intake verification", version: "1" },
        entries,
      },
    }),
  );
  await main.evaluate(
    `(async()=>{await ${shell}.activateView('traffic');await ${traffic}.importPaths([${JSON.stringify(harPath)}]);})()`,
  );
  const captured = await invoke("query_sessions", {
    query: {
      search: "native-intake.invalid/captured",
      limit: 10,
      sort: { column: "request-number", direction: "ascending" },
    },
  });
  assert.equal(captured.sessions.length, 2);
  for (const [index, row] of captured.sessions.entries())
    await pointer(traffic + `.querySelector('tr[data-session-id="${row.id}"]')`, index ? 2 : 0);
  await pointer(
    button(traffic + ".querySelector('.traffic-selection-bar')", "Add to response set…"),
  );
  await waitFor(`${dialog}.open`);
  await idle();
  assert.equal(await main.evaluate(`${picker}.intakeSelectedCount`), 2);
  await main.evaluate(`${picker}.destinationSelect.focus()`);
  await key("End", "End", 35);
  await key("Tab", "Tab", 9);
  assert.equal(await main.evaluate(`${picker}.intakeDestination`), "__new");
  await input(picker + ".intakeNameInput", "Native collected debugging work");
  await input(picker + ".intakeNotesInput", "Binary Traffic and ordinary saved responses.");
  await pointer(button(dialog, "Review responses"));
  await idle();
  assert.match(
    await main.evaluate(picker + ".intakeReviewSummary"),
    /Native collected debugging work/,
  );
  await acknowledge();
  await pointer(button(dialog, "Add responses"));
  await idle();
  const collected = await invoke("automation_status"),
    owner = collected.responseSets.find((set) => set.name === "Native collected debugging work");
  assert.ok(owner);
  assert.equal(owner.rules.length, 2);
  assert.deepEqual(collected.rules, before.rules);
  assert.equal(owner.applied, null);
  const frozen = await Promise.all(
    owner.rules.map((rule) =>
      invoke("inspect_response_asset", { reference: rule.request.responseAsset }),
    ),
  );
  assert.deepEqual(
    frozen.map((asset) => asset.bytesBase64),
    bodies.map((body) => body.toString("base64")),
  );
  await pointer(button(dialog, "Open response set"));
  await waitFor(`${a}.collectionOpen&&${sets}.setId===${JSON.stringify(owner.id)}`);
  await pointer(button(sets, owner.rules[1].displayName));
  await waitFor(`!${a}.loadingSavedResponse`);
  const selectedResponse = owner.rules[1].id;
  const assertMounted = async (status) => {
    const set = status.responseSets.find((set) => set.id === owner.id);
    assert.deepEqual(
      await main.evaluate(`${sets}.draftRules.map(rule=>rule.id)`),
      set.rules.map((rule) => rule.id),
    );
    assert.deepEqual(await main.evaluate(`${sets}.currentSelection()`), {
      experimentId: set.id,
      incarnation: set.incarnation,
      variantId: "",
      revision: set.revision,
    });
    assert.equal(await main.evaluate(`${sets}.selectedRuleId`), selectedResponse);
    assert.equal(await main.evaluate(`${a}.editingAutoResponseId`), selectedResponse);
  };
  const action = { headers: [], replaceBody: null, discardBody: false, abortReason: null },
    ordinary = [];
  for (let index = 1; index <= 2; index++) {
    const asset = await invoke("create_response_asset", {
      input: {
        id: "native-intake-body-" + index,
        revision: 1,
        status: 201,
        headers: [],
        body: [0, 255, index, 13, 10],
        mediaType: "application/octet-stream",
      },
    });
    ordinary.push({
      id: "native-intake-rule-" + index,
      displayName: index === 1 ? "Native active checkout rule" : "Native excluded account rule",
      enabled: index === 1,
      revision: 7,
      priority: -2000000 + index,
      matcher: {
        method: "POST",
        url: { kind: "exact", value: "https://native-intake.invalid/rule/" + index },
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
        responseAsset: asset.id + "@" + asset.revision,
        allowNonIdempotentBodyReplacement: false,
      },
      response: { ...action },
    });
  }
  const current = await invoke("automation_status"),
    candidate = await invoke("validate_automation", {
      document: {
        schemaVersion: 2,
        generation: current.generation,
        autoresponsesEnabled: current.autoresponsesEnabled,
        rules: [...current.rules, ...ordinary],
      },
    });
  await invoke("activate_automation", { candidateId: candidate.candidateId });
  await main.evaluate(a + ".refreshAutomation()");
  const openRules = async () => {
    await pointer(sets + ".addButton");
    await pointer(button(sets + ".addMenu", "From existing rules…"));
    await waitFor(`${dialog}.open`);
    await idle();
    for (const rule of ordinary)
      await pointer(dialog + `.querySelector('#intake-source-${rule.id} input')`);
  };
  await openRules();
  assert.equal(await main.evaluate(picker + ".intakeMode"), "copy");
  await pointer(button(dialog, "Review responses"));
  await idle();
  await acknowledge();
  await pointer(button(dialog, "Add responses"));
  await idle();
  const copied = await invoke("automation_status");
  assert.deepEqual(
    copied.rules.filter((rule) => ordinary.some((source) => source.id === rule.id)),
    ordinary,
  );
  assert.deepEqual(
    copied.responseSets
      .find((set) => set.id === owner.id)
      .rules.slice(-2)
      .map((rule) => [rule.displayName, rule.enabled, rule.request.responseAsset]),
    ordinary.map((rule) => [rule.displayName, rule.enabled, rule.request.responseAsset]),
  );
  await assertMounted(copied);
  await pointer(button(dialog, "Close"));
  await input(a + ".autoResponseBody", '{"savedAfterIntake":true}');
  await key("s", "KeyS", 83, 2);
  await waitFor(`!${sets}.busy&&!${sets}.dirty&&!${a}.loadingSavedResponse`);
  const resaved = await invoke("automation_status");
  assert.equal(resaved.responseSets.find((set) => set.id === owner.id).rules.length, 4);
  await assertMounted(resaved);
  await input(sets + ".notesInput", "Keep this native destination draft");
  await openRules();
  await pointer(dialog + ".querySelector('input[value=" + '"move"' + "]')");
  await pointer(button(dialog, "Review responses"));
  await waitFor(a + ".unsavedDialog.open");
  await pointer(button(a + ".unsavedDialog", "Discard changes"));
  await idle();
  await key("Escape", "Escape", 27);
  assert.equal(
    await main.evaluate(sets + ".notesInput.value"),
    "Keep this native destination draft",
  );
  assert.equal(await main.evaluate(sets + ".dirty"), true);
  await openRules();
  await pointer(dialog + ".querySelector('input[value=" + '"move"' + "]')");
  await pointer(button(dialog, "Review responses"));
  await waitFor(a + ".unsavedDialog.open");
  await pointer(button(a + ".unsavedDialog", "Discard changes"));
  await idle();
  await acknowledge();
  const reviewed = await invoke("automation_status");
  assert.equal(
    await main.evaluate(picker + ".intakeReview.activeCount"),
    reviewed.autoresponsesEnabled ? 1 : 0,
  );
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await main.call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const image = await main.call("Page.captureScreenshot", { format: "png" });
    await writeFile(resolve(output, `native-move-${width}.png`), Buffer.from(image.data, "base64"));
    assert.equal(
      await main.evaluate(
        `(()=>{const r=${dialog}.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;})()`,
      ),
      true,
    );
  }
  await pointer(button(dialog, "Move 2 rules"));
  await idle();
  const moved = await invoke("automation_status");
  assert.equal(
    moved.rules.some((rule) => ordinary.some((source) => source.id === rule.id)),
    false,
  );
  assert.equal(moved.responseSets.find((set) => set.id === owner.id).applied, null);
  await assertMounted(moved);
  assert.equal(await main.evaluate(sets + ".dirty"), false);
  assert.equal(await main.evaluate(sets + ".notesInput.value"), owner.notes);
  await pointer(button(dialog, "Close"));
  await main.call("Emulation.clearDeviceMetricsOverride");
  assert.equal((await invoke("app_status")).lifecycle, "stopped");
  await writeFile(
    resolve(output, "native-intake-result.json"),
    JSON.stringify(
      {
        before,
        collected,
        copied,
        moved,
        exactBinaryBodies: true,
        pointerAndKeyboard: true,
        discardDeferredAcrossCancel: true,
        mountedRowsAndRevisionRefreshed: true,
        subsequentSaveRetainsAdditions: true,
        noAutoApply: true,
      },
      null,
      2,
    ),
  );
  return { ownerId: owner.id, traffic: 2, copiedRules: 2, movedRules: 2, exactBinaryBodies: true };
}

/** Focused round-two rerun on the final immutable native snapshot. */
export async function smokeResponseSetMoveSource({ main, waitFor, directory }) {
  const output = resolve(directory, "intake");
  await mkdir(output, { recursive: true });
  const shell = "document.querySelector('app-shell')",
    a = shell + ".automation",
    p = shell + ".responseSetIntake",
    dialog = p + ".intakeDialog";
  const invoke = (command, args = {}) =>
    main.evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`,
    );
  const button = (container, text) =>
    `[...${container}.querySelectorAll('button')].find(node=>node.textContent.trim()===${JSON.stringify(text)}&&node.getClientRects().length)`;
  const pointer = async (expression) => {
    await main.evaluate(`${expression}.scrollIntoView({block:'nearest'})`);
    const point = await main.evaluate(
      `(()=>{const r=${expression}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`,
    );
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"])
      await main.call("Input.dispatchMouseEvent", {
        type,
        button: "left",
        buttons: type === "mousePressed" ? 1 : 0,
        clickCount: 1,
        ...point,
      });
  };
  const key = async (key, code, vk, modifiers = 0) => {
    for (const type of ["keyDown", "keyUp"])
      await main.call("Input.dispatchKeyEvent", {
        type,
        key,
        code,
        windowsVirtualKeyCode: vk,
        modifiers,
        ...(key === " " && type === "keyDown" ? { text: " " } : {}),
      });
  };
  const input = async (expression, text) => {
    await pointer(expression);
    await key("a", "KeyA", 65, 2);
    await main.call("Input.insertText", { text });
  };
  const idle = () => waitFor(`!${p}.intakeLoading&&!${p}.intakeReviewBusy&&!${p}.intakeSaving`);
  await main.evaluate(
    `(async()=>{await ${shell}.activateView('automation');await ${a}.showAutomationSection('responses');})()`,
  );
  const suffix = Date.now(),
    action = { headers: [], replaceBody: null, discardBody: false, abortReason: null },
    rules = [];
  for (let index = 0; index < 2; index++) {
    const asset = await invoke("create_response_asset", {
      input: {
        id: `native-source-draft-${suffix}-${index}`,
        revision: 1,
        status: 200,
        headers: [],
        body: [...Buffer.from("saved source body " + index)],
        mediaType: "text/plain",
      },
    });
    rules.push({
      id: `native-source-rule-${suffix}-${index}`,
      displayName: "Native source draft " + index,
      enabled: true,
      revision: 1,
      priority: -1000000 + index,
      matcher: {
        method: "GET",
        url: { kind: "exact", value: "https://native-intake.invalid/draft/" + index },
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
        responseAsset: asset.id + "@" + asset.revision,
        allowNonIdempotentBodyReplacement: false,
      },
      response: { ...action },
    });
  }
  const current = await invoke("automation_status"),
    candidate = await invoke("validate_automation", {
      document: {
        schemaVersion: 2,
        generation: current.generation,
        autoresponsesEnabled: current.autoresponsesEnabled,
        rules: [...current.rules, ...rules],
      },
    });
  await invoke("activate_automation", { candidateId: candidate.candidateId });
  await main.evaluate(
    `(async()=>{await ${a}.refreshAutomation();await ${a}.editAutoResponse(${JSON.stringify(rules[0])},true);})()`,
  );
  await waitFor(`!${a}.loadingSavedResponse`);
  await input(a + ".autoResponseBody", "unsaved native source body");
  await input(a + ".autoResponseForm.elements.namedItem('name')", "Saved native source draft");
  await pointer(a + ".ruleMoreButton");
  await pointer(button(a + ".ruleMenu", "Add to response set…"));
  await waitFor(dialog + ".open");
  await idle();
  const checkbox = (index) => dialog + `.querySelector('#intake-source-${rules[index].id} input')`;
  await pointer(checkbox(1));
  await main.evaluate(checkbox(0) + ".focus()");
  await key(" ", "Space", 32);
  assert.equal(await main.evaluate(checkbox(0) + ".checked"), false);
  assert.equal(await main.evaluate(checkbox(1) + ".checked"), true);
  await key(" ", "Space", 32);
  assert.equal(await main.evaluate(checkbox(0) + ".checked"), true);
  assert.equal(await main.evaluate(checkbox(1) + ".checked"), true);
  await pointer(checkbox(1));
  await main.evaluate(p + ".destinationSelect.focus()");
  await key("End", "End", 35);
  await key("Tab", "Tab", 9);
  await input(p + ".intakeNameInput", "Native source boundary " + suffix);
  await pointer(dialog + ".querySelector('input[value=move]')");
  await pointer(button(dialog, "Review responses"));
  await waitFor(a + ".unsavedDialog.open");
  await pointer(button(a + ".unsavedDialog", "Keep editing"));
  await idle();
  assert.equal(await main.evaluate(a + ".draftDirty"), true);
  assert.equal(
    (await invoke("automation_status")).rules.some((rule) => rule.id === rules[0].id),
    true,
  );
  await pointer(button(dialog, "Review responses"));
  await waitFor(a + ".unsavedDialog.open");
  await pointer(button(a + ".unsavedDialog", "Discard changes"));
  await idle();
  await key("Escape", "Escape", 27);
  assert.equal(await main.evaluate(a + ".draftDirty"), true);
  assert.equal(await main.evaluate(a + ".autoResponseBody.value"), "unsaved native source body");
  await pointer(a + ".ruleMoreButton");
  await pointer(button(a + ".ruleMenu", "Add to response set…"));
  await waitFor(dialog + ".open");
  await idle();
  await main.evaluate(p + ".destinationSelect.focus()");
  await key("End", "End", 35);
  await key("Tab", "Tab", 9);
  await input(p + ".intakeNameInput", "Native source boundary " + suffix);
  await pointer(dialog + ".querySelector('input[value=move]')");
  await pointer(button(dialog, "Review responses"));
  await waitFor(a + ".unsavedDialog.open");
  await pointer(button(a + ".unsavedDialog", "Save changes"));
  await idle();
  assert.match(await main.evaluate(dialog + ".textContent"), /Saved native source draft/);
  if (await main.evaluate(p + ".intakeReview.warnings.length>0"))
    await pointer(dialog + ".querySelector('.intake-acknowledge input')");
  await pointer(button(dialog, "Move 1 rule"));
  await idle();
  const moved = await invoke("automation_status"),
    destination = moved.responseSets.find((set) => set.name === "Native source boundary " + suffix);
  assert.ok(destination);
  assert.equal(
    moved.rules.some((rule) => rule.id === rules[0].id),
    false,
  );
  assert.equal(destination.rules[0].displayName, "Saved native source draft");
  const inspection = await invoke("inspect_response_asset", {
    reference: destination.rules[0].request.responseAsset,
  });
  assert.equal(inspection.display, "unsaved native source body");
  assert.equal(await main.evaluate(a + ".editorHidden"), true);
  assert.equal(await main.evaluate(a + ".editingAutoResponseId"), null);
  assert.equal(destination.applied, null);
  const image = await main.call("Page.captureScreenshot", { format: "png" });
  await writeFile(
    resolve(output, "native-source-draft-move.png"),
    Buffer.from(image.data, "base64"),
  );
  await pointer(button(dialog, "Close"));
  await writeFile(
    resolve(output, "native-source-draft-result.json"),
    JSON.stringify(
      {
        destinationId: destination.id,
        keepPreserved: true,
        discardCanceledPreserved: true,
        saveThenMoveBody: inspection.display,
        removedSourceEditorClosed: true,
        checkboxSpacePreservedOtherSelection: true,
      },
      null,
      2,
    ),
  );
  return { destinationId: destination.id, sourceDraftBoundary: true, checkboxSpace: true };
}
