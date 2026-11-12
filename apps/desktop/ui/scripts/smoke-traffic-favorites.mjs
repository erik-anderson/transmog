import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { join, dirname } from "node:path";
import { nativeFixture } from "./native-fixture.mjs";

const traffic = 'document.querySelector("app-shell").traffic';
export async function smokeTrafficFavorites({
  main,
  viewer,
  source,
  screenshot,
  waitFor,
  restart = false,
}) {
  const evaluate = (client, body) => client.evaluate(`(async()=>{const t=${traffic};${body}})()`);
  const ready = (client) =>
    waitFor(
      () =>
        evaluate(
          client,
          "return !t.pickerLoading && !t.pickerSaving && !t.pickerLibraryLoading && !t.queryPending && !t.rowRequests.size;",
        ),
      "Traffic Favorite operation did not finish",
    );
  const open = async (client) => {
    await evaluate(client, 't.openFilterPicker("host");');
    await ready(client);
    await evaluate(client, "await t.loadFavorites();");
  };
  const click = (client, label) =>
    evaluate(
      client,
      `const input=[...t.filterPicker.querySelectorAll('input[type=checkbox]')].find(input=>input.getAttribute('aria-label')===${JSON.stringify(label)});if(!input)throw new Error('Missing checkbox '+${JSON.stringify(label)});input.click();`,
    );
  const preview = async (client, expected, denominator = 6) => {
    await ready(client);
    assert.equal(
      await evaluate(client, 'return t.filterPicker.querySelector(".filter-preview").textContent;'),
      `After Apply · ${expected} of ${denominator} requests`,
    );
  };
  const apply = async (client, expected) => {
    await evaluate(client, 'await t.applyFilter(new Event("submit",{cancelable:true}));');
    await ready(client);
    assert.equal(await evaluate(client, "return t.totalMatched;"), expected);
    const ids = await evaluate(
      client,
      "return await window.__TAURI_INTERNALS__.invoke('matching_traffic_ids',{query:t.trafficQuery()});",
    );
    assert.equal(ids.length, expected, "Filtered selection/export IDs disagree with the UI");
  };
  const reset = async (client, mode = "either", pick) => {
    await evaluate(
      client,
      "if(!t.filterPicker.querySelector('.filter-custom').open)t.filterPicker.querySelector('.filter-custom > summary').click();t.clearPicker();",
    );
    await click(client, "Use saved term api.example.test");
    await click(client, "Use saved term *.domain.example");
    await evaluate(
      client,
      `const select=t.filterPicker.querySelector('[aria-label="Combine groups"]');select.value=${JSON.stringify(mode)};select.dispatchEvent(new Event('change'));`,
    );
    if (pick) await click(client, "Pick observed host " + pick);
  };
  const list = (client) =>
    evaluate(
      client,
      "return await window.__TAURI_INTERNALS__.invoke('traffic_favorites',{dimension:'host',offset:0,limit:256});",
    );
  const withFavoriteGate = async (client, start, whilePending) => {
    await evaluate(
      client,
      `
      const review = globalThis.__favoriteReview = { original: t.changeFavorites };
      const gate = new Promise(resolve => review.release = resolve);
      t.changeFavorites = async function(change) {
        const result = await review.original.call(this, change);
        review.persisted = true;
        await gate;
        return result;
      };
      ${start}
    `,
    );
    try {
      await waitFor(
        () => evaluate(client, "return !!globalThis.__favoriteReview.persisted;"),
        "The native Favorite save did not reach its gate",
      );
      await evaluate(
        client,
        `
        ${whilePending}
        globalThis.__favoriteReview.release();
        await globalThis.__favoriteReview.operation;
      `,
      );
    } finally {
      await evaluate(
        client,
        `
        t.changeFavorites = globalThis.__favoriteReview.original;
        globalThis.__favoriteReview.release();
        delete globalThis.__favoriteReview;
      `,
      );
    }
  };
  const expectedPath = join(dirname(source), "traffic-favorites-expected.json");
  if (restart) {
    const expected = JSON.parse(await readFile(expectedPath, "utf8"));
    assert.deepEqual((await list(viewer)).entries, expected);
    for (const client of [main, viewer]) {
      assert.deepEqual(
        await evaluate(client, "return t.filterGroups;"),
        [],
        "A fresh window inherited another window's selection",
      );
      await open(client);
      assert.equal(
        await evaluate(client, "return t.filterPicker.querySelector('.filter-custom').open;"),
        false,
        "Idle Favorites expanded the saved/custom group in a fresh window",
      );
      assert.equal(
        await evaluate(
          client,
          "return t.pickerRequirements.some(row=>row.selected) || t.pickerSavedOptions.some(row=>row.selected);",
        ),
        false,
      );
      await evaluate(client, "t.closePicker();");
    }
    return { restartPersistence: true, freshWindowUnrestricted: true };
  }
  const fixturePath = join(dirname(source), "traffic-favorites.tmcap");
  const hosts = [
    ["api.example.test", "GET"],
    ["api.example.test", "POST"],
    ["cdn.domain.example", "GET"],
    ["eu.cdn.domain.example", "GET"],
    ["domain.example", "POST"],
    ["otherdomain.example", "GET"],
  ];
  await writeFile(
    fixturePath,
    nativeFixture(
      hosts.flatMap(([host, method], index) => [
        {
          kind: "request-head",
          exchange_id: index + 1,
          payload: { boundary: "client-request", method, target: `https://${host}/`, headers: [] },
        },
        {
          kind: "response-head",
          exchange_id: index + 1,
          payload: { boundary: "client-response", status: 200, headers: [] },
        },
      ]),
      1,
    ),
  );
  for (const client of [main, viewer])
    await evaluate(
      client,
      "await t.clearFilters();await window.__TAURI_INTERNALS__.invoke('clear_traffic');await t.refreshSessions(undefined,true);",
    );
  await evaluate(
    main,
    "await window.__TAURI_INTERNALS__.invoke('change_traffic_favorites',{change:{action:'clear',dimension:'host'}});",
  );
  await open(main);
  assert.equal(
    await evaluate(main, "return t.filterPicker.querySelector('.filter-custom').open;"),
    false,
  );
  assert.equal(
    await evaluate(main, "return t.filterPicker.querySelector('.filter-combine').hidden;"),
    true,
    "The default native picker shows Combine groups without any custom values",
  );
  await evaluate(main, "t.filterPicker.querySelector('.filter-custom > summary').click();");
  assert.equal(await evaluate(main, "return t.pickerRequirements.length;"), 0);
  await withFavoriteGate(
    main,
    't.filterPaste.value="cancelled-native.test";t.filterPaste.dispatchEvent(new Event("input"));review.operation=t.applyFilter(new Event("submit",{cancelable:true}));',
    "[...t.filterPicker.querySelectorAll('button')].find(button=>button.textContent.trim()==='Discard changes').click();",
  );
  assert.deepEqual(await evaluate(main, "return t.filterGroups;"), []);
  assert.equal(
    (await list(main)).entries.some((entry) => entry.value?.value === "cancelled-native.test"),
    true,
  );
  await open(main);
  await withFavoriteGate(
    main,
    't.filterPaste.value="late-native.test";t.filterPaste.dispatchEvent(new Event("input"));review.operation=t.applyFilter(new Event("submit",{cancelable:true}));',
    't.filterInput.value="newer comparison";t.filterInput.dispatchEvent(new Event("input"));',
  );
  assert.deepEqual(await evaluate(main, "return t.filterGroups;"), []);
  assert.equal(await evaluate(main, "return t.filterInput.value;"), "newer comparison");
  assert.equal(await evaluate(main, "return t.pickerOpen;"), true);
  await evaluate(main, "t.discardPicker();");
  await open(main);
  await evaluate(
    main,
    't.filterPaste.value="api.example.test, *.domain.example, future.example.test";t.filterPaste.dispatchEvent(new Event("input"));',
  );
  await preview(main, 0, 0);
  assert.equal(
    await evaluate(
      main,
      "return t.filterPicker.querySelector('.filter-list-entry label > span').textContent;",
    ),
    "Is in (values separated by commas or newlines)",
  );
  await apply(main, 0);
  for (const client of [main, viewer]) {
    await evaluate(client, `await t.importTrace(${JSON.stringify(fixturePath)});`);
    await ready(client);
  }
  assert.equal(await evaluate(main, "return t.totalMatched;"), 5);
  assert.equal(await evaluate(viewer, "return t.totalMatched;"), 6);
  for (const [role, client] of [
    ["main", main],
    ["viewer", viewer],
  ]) {
    await open(client);
    const appliedBefore = await evaluate(client, "return t.totalMatched;");
    await reset(client);
    await preview(client, 5);
    for (const host of ["domain.example", "cdn.domain.example", "eu.cdn.domain.example"])
      assert.equal(
        await evaluate(
          client,
          `return t.pickerOptions.find(row=>row.label===${JSON.stringify(host)}).reason;`,
        ),
        "Included by *.domain.example",
      );
    assert.equal(
      await evaluate(
        client,
        'return t.pickerSavedOptions.find(row=>row.label==="*.domain.example").countText;',
      ),
      "3",
    );
    await click(client, "Pick observed host domain.example");
    await preview(client, 5);
    await evaluate(client, "t.closePicker();");
    assert.equal(await evaluate(client, "return t.filterDraftPending;"), true);
    await open(client);
    assert.equal(
      await evaluate(
        client,
        'return t.pickerOptions.find(row=>row.label==="domain.example").selected;',
      ),
      true,
      "Native close/reopen lost the filter draft",
    );
    assert.equal(await evaluate(client, "return t.totalMatched;"), appliedBefore);
    await evaluate(
      client,
      "[...t.filterPicker.querySelectorAll('button')].find(button=>button.textContent.trim()==='Discard changes').click();",
    );
    await ready(client);
    assert.equal(await evaluate(client, "return t.totalMatched;"), appliedBefore);
    await open(client);
    await reset(client, "either", "domain.example");
    await preview(client, 5);
    await evaluate(
      client,
      'const row=t.pickerOptions.find(row=>row.label==="domain.example");if(!row.favorite)await t.toggleFavorite(row.key);',
    );
    await preview(client, 5);
    assert.equal(
      await evaluate(
        client,
        'return t.pickerSavedOptions.find(row=>row.label==="domain.example").selected;',
      ),
      false,
    );
    const accessibility = await client.call("Accessibility.getFullAXTree");
    const checkboxNames = accessibility.nodes
      .filter((node) => node.role?.value === "checkbox")
      .map((node) => node.name?.value);
    assert.ok(checkboxNames.includes("Use saved term api.example.test"));
    assert.ok(checkboxNames.includes("Pick observed host api.example.test"));
    await evaluate(
      client,
      't.favoriteSearch.value="api";t.favoriteSearch.dispatchEvent(new Event("input"));t.$flushUpdates();t.favoriteSearch.focus();',
    );
    await client.call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
    await client.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Enter",
      code: "Enter",
      windowsVirtualKeyCode: 13,
    });
    assert.equal(
      await evaluate(client, "return t.filterPicker.matches(':popover-open');"),
      true,
      "Native Favorite search submitted the filter form",
    );
    await evaluate(
      client,
      't.favoriteSearch.value="";t.favoriteSearch.dispatchEvent(new Event("input"));',
    );
    assert.ok(
      accessibility.nodes.some(
        (node) =>
          node.role?.value === "button" &&
          node.name?.value === "Favorite domain.example" &&
          node.properties?.some(
            (property) => property.name === "pressed" && String(property.value.value) === "true",
          ),
      ),
    );
    await client.call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
    await client.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
    await ready(client);
    assert.equal(await evaluate(client, "return t.filterPicker.matches(':popover-open');"), false);
    assert.equal(
      await evaluate(
        client,
        "return t.filterButton === t.filterButton.getRootNode().activeElement;",
      ),
      true,
    );
    await open(client);
    assert.equal(
      await evaluate(
        client,
        'return t.pickerSavedOptions.find(row=>row.label==="domain.example").selected;',
      ),
      false,
    );
    for (const [mode, pick, expected] of [
      ["either", null, 5],
      ["either", "domain.example", 5],
      ["both", "domain.example", 1],
      ["both", "otherdomain.example", 0],
      ["both", "cdn.domain.example", 1],
      ["either", "cdn.domain.example", 5],
      ["ignore-saved", "domain.example", 1],
      ["ignore-saved", null, 6],
    ]) {
      await reset(client, mode, pick);
      await preview(client, expected);
      await apply(client, expected);
      await open(client);
    }
    await reset(client, "either", "otherdomain.example");
    await apply(client, 6);
    await open(client);
    const order = (client) =>
      evaluate(
        client,
        "return [t.pickerSavedOptions.map(row=>row.key),t.pickerOptions.map(row=>row.key)];",
      );
    const beforeOrder = await order(client);
    assert.equal(beforeOrder[1][0], "text:otherdomain.example");
    const presentation = await evaluate(
      client,
      `
      const field=t.filterPicker.querySelector('[aria-label="Filter field"]');
      const host=field.querySelector('[value="host"]');
      return {
        field:host.textContent, accessible:host.getAttribute('aria-label'),
        accent:getComputedStyle(field).color===getComputedStyle(host).color,
        label:t.filterPicker.querySelector('.filter-list-entry label > span').textContent,
        example:t.filterPicker.querySelector('.filter-list-entry .filter-picker-hint').textContent,
        modes:[...t.filterPicker.querySelector('[aria-label="Combine groups"]').options].map(option=>option.textContent),
        countNote:t.pickerScope,
        savedHeading:t.filterPicker.querySelector('.filter-custom > summary').textContent,
        observedHeading:t.filterPicker.querySelector('.filter-observed-group > h3').textContent,
        groupSummary:t.pickerCombineSummary,
        requirementHint:t.filterPicker.querySelector('.filter-requirements > summary').textContent,
      };
    `,
    );
    assert.equal(presentation.field, "● Host");
    assert.equal(presentation.accessible, "Host (filtered)");
    assert.equal(presentation.accent, true);
    assert.equal(presentation.label, "Is in");
    assert.equal(
      presentation.example,
      "Example: exact-domain.example, *.include-subdomains.example",
    );
    assert.deepEqual(presentation.modes, [
      "Match either group (OR)",
      "Match both groups (AND)",
      "Use observed hosts only",
    ]);
    assert.match(presentation.countNote, /Counts ignore this field’s filters/);
    assert.match(presentation.savedHeading, /Saved and custom hosts.*2 selected/);
    assert.match(presentation.observedHeading, /Observed hosts.*1 selected/);
    assert.match(presentation.groupSummary, /Saved and custom hosts OR Observed hosts/);
    assert.match(presentation.requirementHint, /All checked requirements must also match/);
    await click(client, "Use saved term api.example.test");
    await click(client, "Use saved term future.example.test");
    await click(client, "Pick observed host otherdomain.example");
    await click(client, "Pick observed host domain.example");
    await ready(client);
    assert.deepEqual(
      await order(client),
      beforeOrder,
      "Native checkbox edits moved rows before reopening",
    );
    await apply(client, 3);
    await open(client);
    assert.equal((await order(client))[1][0], "text:domain.example");
    assert.equal(
      await evaluate(client, "return t.pickerSavedOptions.slice(0,2).every(row=>row.selected);"),
      true,
    );
    await reset(client, "both", "domain.example");
    await click(client, "Use saved term domain.example");
    await preview(client, 1);
    await evaluate(
      client,
      'const select=t.filterPicker.querySelector(\'[aria-label="Combine groups"]\');select.value="either";select.dispatchEvent(new Event("change"));',
    );
    await preview(client, 5);
    await apply(client, 5);
    await evaluate(client, "await t.clearFilters();t.openFilterPicker('method');");
    await ready(client);
    assert.equal(
      await evaluate(
        client,
        "return t.filterPicker.querySelector('.filter-list-entry .filter-picker-hint');",
      ),
      null,
    );
    await click(client, "GET");
    await apply(client, 4);
    await open(client);
    assert.equal(
      await evaluate(client, "return t.filterField.querySelector('[value=method]').textContent;"),
      "● Method",
    );
    await reset(client);
    await preview(client, 3, 4);
    await evaluate(
      client,
      't.observedSearch.value="other";t.observedSearch.dispatchEvent(new Event("input"));',
    );
    await preview(client, 3, 4);
    await evaluate(client, "t.closePicker();await t.clearFilters();");
    await open(client);
    await click(client, "Pick observed host api.example.test");
    await evaluate(
      client,
      't.filterField.value="method";t.filterField.dispatchEvent(new Event("change"));',
    );
    await ready(client);
    await click(client, "POST");
    await preview(client, 1, 2);
    await apply(client, 1);
    assert.deepEqual(
      await evaluate(client, "return t.filterGroups.map(group=>group.dimension).sort();"),
      ["host", "method"],
    );
    assert.equal(await evaluate(client, "return t.filterDraftPending;"), false);
    await open(client);
    await evaluate(client, "t.clearPicker();");
    await click(client, "Pick observed host domain.example");
    await evaluate(
      client,
      "t.closePicker();[...t.getRootNode().querySelectorAll('.filter-chip-remove')].find(button=>button.getAttribute('aria-label').startsWith('Remove Host:')).click();",
    );
    await ready(client);
    await evaluate(client, "t.openFilterPicker('method');");
    await ready(client);
    await apply(client, 2);
    assert.deepEqual(
      await evaluate(client, "return t.filterGroups.map(group=>group.dimension);"),
      ["method"],
      "A native Apply resurrected the removed Host draft",
    );
    await evaluate(client, "await t.clearFilters();");
    await open(client);
    await reset(client);
    const beforeRequirement = await list(client);
    await withFavoriteGate(
      client,
      't.filterOperator.value="contains";t.filterInput.value="cdn";review.operation=t.addRequirement();',
      't.filterInput.value="next requirement";t.filterInput.dispatchEvent(new Event("input"));t.filterOperator.value="equals";',
    );
    assert.equal(await evaluate(client, "return t.filterInput.value;"), "next requirement");
    assert.equal(await evaluate(client, "return t.filterOperator.value;"), "equals");
    if (
      beforeRequirement.entries.some(
        (entry) => entry.condition?.operator === "contains" && entry.condition.value === "cdn",
      )
    )
      assert.equal((await list(client)).revision, beforeRequirement.revision);

    await ready(client);
    assert.match(
      await evaluate(client, "return t.pickerPreview;"),
      /Finish or correct pending fields/,
    );
    const beforeUnfinishedApply = await evaluate(client, "return t.totalMatched;");
    await evaluate(client, 'await t.applyFilter(new Event("submit",{cancelable:true}));');
    assert.equal(await evaluate(client, "return t.totalMatched;"), beforeUnfinishedApply);
    assert.equal(await evaluate(client, "return t.pickerOpen;"), true);
    assert.equal(await evaluate(client, "return t.filterInput.value;"), "next requirement");
    await evaluate(
      client,
      't.getRootNode().getElementById("header-method").querySelector("button").click();',
    );
    await open(client);
    assert.equal(
      await evaluate(client, "return t.filterInput.value;"),
      "next requirement",
      "Opening a native column menu overwrote the retained comparison draft",
    );
    assert.equal(await evaluate(client, "return t.filterOperator.value;"), "equals");
    await evaluate(
      client,
      't.filterInput.value="cdn";t.filterInput.dispatchEvent(new Event("input"));t.filterOperator.value="contains";await t.applyFilter(new Event("submit",{cancelable:true}));await t.addRequirement();',
    );
    assert.equal(await evaluate(client, "return t.filterInput.value;"), "");
    assert.equal(
      await evaluate(client, "return t.filterInput.checkValidity();"),
      true,
      "Adding the pending comparison left native Apply blocked by stale validity",
    );
    await apply(client, 2);
    await open(client);
    if (screenshot) {
      const image = await client.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", `-favorites-${role}.png`),
        Buffer.from(image.data, "base64"),
      );
    }
    await evaluate(
      client,
      "t.closePicker();await t.clearFilters();t.openFilterPicker('fetch-destination');",
    );
    await ready(client);
    assert.equal(
      await evaluate(client, "return t.pickerMoreOptions.some(row=>row.key==='text:style');"),
      true,
    );
    await evaluate(
      client,
      't.observedSearch.value="style";t.observedSearch.dispatchEvent(new Event("input"));',
    );
    await ready(client);
    await evaluate(client, "t.filterPicker.querySelector('.filter-observed button').click();");
    await ready(client);
    assert.equal(
      await evaluate(
        client,
        `
      const input=t.filterPicker.querySelector('[aria-label="Stylesheet"]');
      return !!input?.checked && !!input.getClientRects().length && !input.closest('details');
    `,
      ),
      true,
      "Native exact authoring left a checked choice hidden inside More values",
    );
    await apply(client, 0);
    await evaluate(client, "await t.clearFilters();");
    await open(client);
    const authored = `editing-review-${role}.test`;
    await evaluate(
      client,
      `t.filterPaste.value=${JSON.stringify(authored)};t.filterPaste.dispatchEvent(new Event('input'));`,
    );
    const editingAccessibility = await client.call("Accessibility.getFullAXTree");
    assert.equal(
      editingAccessibility.nodes.find(
        (node) => node.role?.value === "textbox" && node.name?.value === "Is in",
      )?.description?.value,
      "Enter values separated by commas or newlines",
    );
    await evaluate(client, "await t.savePaste();");
    await ready(client);
    await evaluate(client, `await t.toggleFavorite(${JSON.stringify("text:" + authored)},true);`);
    await ready(client);
    await click(client, "Use saved term " + authored);
    await ready(client);
    assert.equal(
      await evaluate(
        client,
        `
      const input=[...t.filterPicker.querySelectorAll('input')].find(input=>input.getAttribute('aria-label')===${JSON.stringify("Use saved term " + authored)});
      return !!input && !input.checked && !!input.getClientRects().length;
    `,
      ),
      true,
      "Native uncheck removed the newly authored unstarred row",
    );
    assert.equal(
      (await list(client)).entries.some((entry) => entry.value?.value === authored),
      false,
    );
    await apply(client, 6);
    await open(client);
    assert.equal(
      await evaluate(
        client,
        `return t.pickerSelectedOptions.some(row=>row.value?.value===${JSON.stringify(authored)});`,
      ),
      false,
    );
    await evaluate(client, "t.closePicker();await t.clearFilters();");
  }
  await open(main);
  await reset(main);
  await apply(main, 5);
  await open(main);
  await open(viewer);
  await reset(viewer, "both", "otherdomain.example");
  await preview(viewer, 0);
  await evaluate(main, "await t.clearFavorites();");
  await ready(viewer);
  await evaluate(viewer, "await t.loadFavorites();");
  assert.equal(
    await evaluate(viewer, "return t.pickerSelectedOptions.length;"),
    2,
    "Shared library refresh rewrote the viewer draft",
  );
  await evaluate(
    viewer,
    't.filterPaste.value="intervening.test";t.filterPaste.dispatchEvent(new Event("input"));await t.savePaste();',
  );
  await evaluate(main, "await t.undoFavorites();");
  assert.equal(
    (await list(main)).entries.some((entry) => entry.value?.value === "intervening.test"),
    true,
  );
  await apply(viewer, 0);
  assert.equal(await evaluate(main, "return t.totalMatched;"), 5);
  assert.equal(await evaluate(viewer, "return t.totalMatched;"), 0);
  await writeFile(expectedPath, JSON.stringify((await list(main)).entries));
  return {
    mainAcceptanceRows: true,
    savedViewerAcceptanceRows: true,
    sharedFavorites: true,
    independentDrafts: true,
    filteredIds: true,
    interleavedClearAddUndoApply: true,
    explicitApplyAndCancel: true,
    pendingSaveCancel: true,
    preservesRequirementDraft: true,
    idempotentRetries: true,
    pickerEditingReview: true,
    combinedFieldApply: true,
    wildcardApex: true,
  };
}
