import assert from "node:assert/strict";
import { resolve } from "node:path";

export async function checkTraceSave(page, root, dialog) {
  const duplicate = await page.evaluate(async () => {
    const state = globalThis.__workspaceFixture,
      traffic = document.querySelector("app-shell").traffic,
      manager = traffic.markerIntake,
      apply = manager.applyPending,
      dirty = traffic.markerDraftDirty,
      markers = traffic.saveTraceForm.elements.namedItem("markers"),
      originalMarkers = markers.checked,
      calls = state.calls.save_traffic_trace ?? 0;
    let release;
    const gate = new Promise((resolve) => (release = resolve));
    manager.applyPending = async () => {
      await gate;
      return true;
    };
    traffic.markerDraftDirty = true;
    markers.checked = true;
    const expectedMarkers = !markers.matches(":disabled");
    state.cancelTraceSave = true;
    try {
      const first = traffic.saveTrafficTrace(new Event("submit", { cancelable: true })),
        second = traffic.saveTrafficTrace(new Event("submit", { cancelable: true })),
        busy = traffic.savingTrace;
      release();
      await Promise.all([first, second]);
      const result = {
        busy,
        calls: (state.calls.save_traffic_trace ?? 0) - calls,
        includeMarkers: state.savedTraceArgs.options.includeMarkers,
        expectedMarkers,
      };
      manager.applyPending = async () => false;
      traffic.markerDraftDirty = true;
      await traffic.saveTrafficTrace(new Event("submit", { cancelable: true }));
      return {
        ...result,
        failureReleased: !traffic.savingTrace,
        finalCalls: (state.calls.save_traffic_trace ?? 0) - calls,
        failureStatus: traffic.saveTraceStatus,
      };
    } finally {
      release();
      manager.applyPending = apply;
      traffic.markerDraftDirty = dirty;
      markers.checked = originalMarkers;
      state.cancelTraceSave = false;
    }
  });
  assert.equal(duplicate.busy, true, "Marker preparation did not claim its save before yielding");
  assert.equal(duplicate.calls, 1, "Marker preparation admitted duplicate trace saves");
  assert.equal(
    duplicate.includeMarkers,
    duplicate.expectedMarkers,
    "Disabling the form lost authored save options",
  );
  assert.equal(duplicate.failureReleased, true, "A failed marker edit left saving locked");
  assert.equal(duplicate.finalCalls, 1, "A failed marker edit still saved the trace");
  assert.match(duplicate.failureStatus, /Marker notes could not be applied/);
  await dialog.getByText(duplicate.failureStatus, { exact: true }).waitFor();
  const viewport = page.viewportSize();
  try {
    await page.setViewportSize({ width: 800, height: 600 });
    const close = await dialog.getByRole("button", { name: "Close", exact: true }).boundingBox();
    assert.ok(
      close && close.y >= 0 && close.y + close.height <= 600,
      "Marker failure feedback hid Close in a compact window",
    );
    await page.screenshot({
      path: resolve(root, "../../../target/ui-check/trace-save-marker-failure.png"),
    });
  } finally {
    await page.setViewportSize(viewport);
  }
  const before = await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.deferTraceSave = true;
    state.tracePlaceholders = 2;
    state.traceConnections = 1;
    const traffic = document.querySelector("app-shell").traffic;
    return {
      selection: [...traffic.trafficSelection.ids],
      scroll: traffic.sessionScroller.scrollTop,
    };
  });
  await dialog.getByRole("button", { name: "Save as…", exact: true }).click();
  await page.waitForFunction(() => globalThis.__workspaceFixture.traceSavePending);
  const progress = dialog.getByRole("progressbar", { name: "Saving trace" });
  assert.equal(
    await progress.getAttribute("value"),
    null,
    "Destination/preparation must be indeterminate",
  );
  assert.equal(
    await dialog.getByRole("button", { name: "Save as…", exact: true }).isDisabled(),
    true,
  );
  await page.evaluate(() =>
    globalThis.__workspaceFixture.saveTraceChannel.onmessage({
      phase: "writing",
      completed: 2,
      total: 4,
    }),
  );
  await dialog.getByText("Writing trace · 2 of 4 entries…", { exact: true }).waitFor();
  assert.equal(await progress.getAttribute("value"), "2");
  assert.equal(await progress.getAttribute("max"), "4");
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 960, 720],
    ["small", 800, 600],
  ]) {
    await page.setViewportSize({ width, height });
    await page.screenshot({
      path: resolve(root, `../../../target/ui-check/trace-save-${name}.png`),
    });
    const box = await dialog.boundingBox(),
      button = await dialog.getByRole("button", { name: "Close", exact: true }).boundingBox();
    assert.ok(
      box.x >= 0 && box.x + box.width <= width && box.y >= 0 && box.y + box.height <= height,
      "Save dialog escaped viewport",
    );
    assert.ok(button.y + button.height <= height, "Close became unreachable");
  }
  await page.emulateMedia({ forcedColors: "active", reducedMotion: "reduce" });
  await page.screenshot({
    path: resolve(root, "../../../target/ui-check/trace-save-forced-colors.png"),
  });
  await page.emulateMedia({ forcedColors: "none", reducedMotion: "no-preference" });
  await dialog.getByRole("button", { name: "Close", exact: true }).focus();
  await page.keyboard.press("Enter");
  await page.locator(".trace-save-dialog[open]").waitFor({ state: "hidden" });
  assert.equal(
    await page
      .getByRole("button", { name: "Files", exact: true })
      .evaluate((button) => button === button.getRootNode().activeElement),
    true,
  );
  await page.getByRole("progressbar", { name: "Saving trace" }).waitFor();
  await page.getByRole("button", { name: "Save progress…", exact: true }).click();
  await page.keyboard.press("Escape");
  await page.locator(".trace-save-dialog[open]").waitFor({ state: "hidden" });
  await page.getByRole("button", { name: "Save progress…", exact: true }).click();
  await page.evaluate(() =>
    globalThis.__workspaceFixture.saveTraceChannel.onmessage({
      phase: "converting",
      completed: 0,
      total: 0,
    }),
  );
  await dialog.getByText("Creating SAZ archive…", { exact: true }).waitFor();
  assert.equal(
    await progress.getAttribute("value"),
    null,
    "Unknown conversion work must be indeterminate",
  );
  await page.evaluate(() =>
    globalThis.__workspaceFixture.saveTraceChannel.onmessage({
      phase: "converting",
      completed: 3,
      total: 4,
    }),
  );
  await dialog.getByText("Creating SAZ archive · 3 of 4 entries…", { exact: true }).waitFor();
  await page.evaluate(() =>
    globalThis.__workspaceFixture.saveTraceChannel.onmessage({
      phase: "finalizing",
      completed: 0,
      total: 0,
    }),
  );
  await dialog.getByText("Finalizing trace…", { exact: true }).waitFor();
  await page.evaluate(() => globalThis.__workspaceFixture.releaseTraceSave());
  await page.waitForFunction(() => !document.querySelector("app-shell").traffic.savingTrace);
  await page.getByText(/2 entries use labeled placeholders for uncaptured headers/).waitFor();
  await page.getByText(/1 generated connection summary/).waitFor();
  assert.equal(await page.getByRole("progressbar", { name: "Saving trace" }).count(), 0);
  const after = await page.evaluate(() => {
    const traffic = document.querySelector("app-shell").traffic;
    return {
      selection: [...traffic.trafficSelection.ids],
      scroll: traffic.sessionScroller.scrollTop,
    };
  });
  assert.deepEqual(after.selection, before.selection, "Saving changed inspected selection");
  await page.evaluate(() => {
    const state = globalThis.__workspaceFixture;
    state.deferTraceSave = false;
    state.tracePlaceholders = 0;
    state.traceConnections = 0;
    state.saveTraceChannel.onmessage({ phase: "writing", completed: 1, total: 99 });
  });
  assert.equal(
    await page.getByRole("progressbar", { name: "Saving trace" }).count(),
    0,
    "Late progress revived the completed save",
  );
  await page.setViewportSize({ width: 1280, height: 800 });
  console.log(
    "Trace save progress, placeholder disclosure, background work, keyboard close, narrow layouts and late updates verified.",
  );
}
