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
    await evaluate(client, "t.clearPicker();");
    await click(client, "Use saved term api.example.test");
    await click(client, "Use saved term *.domain.example");
    await evaluate(
      client,
      `const select=t.filterPicker.querySelector('[aria-label="Combine selections"]');select.value=${JSON.stringify(mode)};select.dispatchEvent(new Event('change'));`,
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
        true,
        "Persisted Favorites did not expand Advanced filters in a fresh window",
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
  await evaluate(main, "t.filterPicker.querySelector('.filter-custom > summary').click();");
  assert.equal(await evaluate(main, "return t.pickerRequirements.length;"), 0);
  await withFavoriteGate(
    main,
    't.filterPaste.value="cancelled-native.test";t.filterPaste.dispatchEvent(new Event("input"));review.operation=t.applyFilter(new Event("submit",{cancelable:true}));',
    "[...t.filterPicker.querySelectorAll('button')].find(button=>button.textContent.trim()==='Cancel').click();",
  );
  assert.deepEqual(await evaluate(main, "return t.filterGroups;"), []);
  assert.equal(
    (await list(main)).entries.some((entry) => entry.value?.value === "cancelled-native.test"),
    true,
  );
  await open(main);
  await evaluate(
    main,
    't.filterPaste.value="api.example.test, *.domain.example, future.example.test";t.filterPaste.dispatchEvent(new Event("input"));',
  );
  await preview(main, 0, 0);
  await apply(main, 0);
  for (const client of [main, viewer]) {
    await evaluate(client, `await t.importTrace(${JSON.stringify(fixturePath)});`);
    await ready(client);
  }
  assert.equal(await evaluate(main, "return t.totalMatched;"), 4);
  assert.equal(await evaluate(viewer, "return t.totalMatched;"), 6);
  for (const [role, client] of [
    ["main", main],
    ["viewer", viewer],
  ]) {
    await open(client);
    const appliedBefore = await evaluate(client, "return t.totalMatched;");
    await reset(client);
    await preview(client, 4);
    assert.equal(
      await evaluate(
        client,
        'return t.pickerOptions.find(row=>row.label==="cdn.domain.example").reason;',
      ),
      "Included by *.domain.example",
    );
    await click(client, "Pick observed host domain.example");
    await preview(client, 5);
    await evaluate(
      client,
      "[...t.filterPicker.querySelectorAll('button')].find(button=>button.textContent.trim()==='Cancel').click();",
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
      ["either", null, 4],
      ["either", "domain.example", 5],
      ["both", "domain.example", 0],
      ["both", "cdn.domain.example", 1],
      ["either", "cdn.domain.example", 4],
      ["ignore-saved", "domain.example", 1],
      ["ignore-saved", null, 6],
    ]) {
      await reset(client, mode, pick);
      await preview(client, expected);
      await apply(client, expected);
      await open(client);
    }
    await reset(client, "both", "domain.example");
    await click(client, "Use saved term domain.example");
    await preview(client, 1);
    await apply(client, 1);
    await evaluate(client, "await t.clearFilters();t.openFilterPicker('method');");
    await ready(client);
    await click(client, "GET");
    await apply(client, 4);
    await open(client);
    await reset(client);
    await preview(client, 3, 4);
    await evaluate(
      client,
      't.observedSearch.value="other";t.observedSearch.dispatchEvent(new Event("input"));',
    );
    await preview(client, 3, 4);
    await evaluate(client, "t.closePicker();await t.clearFilters();");
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

    await preview(client, 2);
    await apply(client, 2);
    await open(client);
    if (screenshot) {
      const image = await client.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", `-favorites-${role}.png`),
        Buffer.from(image.data, "base64"),
      );
    }
    await evaluate(client, "t.closePicker();await t.clearFilters();");
  }
  await open(main);
  await reset(main);
  await apply(main, 4);
  await open(main);
  await open(viewer);
  await reset(viewer, "both", "domain.example");
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
  assert.equal(await evaluate(main, "return t.totalMatched;"), 4);
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
  };
}
