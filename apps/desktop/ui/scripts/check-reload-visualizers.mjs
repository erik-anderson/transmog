import assert from "node:assert/strict";

export async function checkReloadVisualizers(page, visualizer, selector) {
  const invoke = (command, args = {}) =>
    page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), {
      command,
      args,
    });
  const ids = () =>
    page.evaluate(() =>
      globalThis.__reloadViews().map((view) => view.view?.handle.instanceId ?? null),
    );
  try {
    await page.evaluate(() => {
      const t = document.querySelector("app-shell").traffic,
        s = globalThis.__workspaceFixture;
      globalThis.__reloadViews = () =>
        [...(t.shadowRoot ?? t).querySelectorAll("message-inspector")]
          .flatMap((i) => [...(i.shadowRoot ?? i).querySelectorAll("extension-visualizer")])
          .filter((v) => v.view);
      s.visualizerDevelopment = true;
      s.visualizerHotReload = false;
      s.reloadEmitsMetadata = true;
      for (const view of Object.values(s.visualizerActive)) view.instance.development = true;
      for (const viewer of globalThis.__reloadViews()) viewer.view.instance.development = true;
    });
    const original = await ids();
    assert.equal(original.length, 3);
    assert.equal(
      await visualizer.getByRole("button", { name: /^Reload/ }).count(),
      0,
      "Development reload controls appeared outside Settings",
    );
    await invoke("reload_extension", { id: "example.protocol" });
    await page.waitForFunction(
      (previous) =>
        globalThis.__reloadViews().length === 3 &&
        globalThis
          .__reloadViews()
          .every((view) => !previous.includes(view.view.handle.instanceId) && !view.visualizerBusy),
      original,
    );
    const good = await ids();
    await page.evaluate(() => {
      globalThis.__workspaceFixture.reloadError = true;
    });
    await invoke("reload_extension", { id: "example.protocol" });
    await page.waitForFunction(() =>
      globalThis
        .__reloadViews()
        .every(
          (view) =>
            view.visualizerReloadNotice.includes("Synthetic reload failed") && !view.visualizerBusy,
        ),
    );
    assert.deepEqual(await ids(), good, "Failed replacement discarded a good worker");
    const failed = await page.evaluate(() => globalThis.__workspaceFixture.calls.reload_visualizer);
    await page.waitForTimeout(1200);
    assert.equal(
      await page.evaluate(() => globalThis.__workspaceFixture.calls.reload_visualizer),
      failed,
      "A failed edit looped without another request",
    );
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.reloadError = false;
      s.visualizerHotReload = true;
      s.sourceEpoch = (s.sourceEpoch ?? 0) + 1;
      s.emitNative("extensions-changed");
    });
    await page.waitForFunction(
      (previous) =>
        globalThis.__reloadViews().length === 3 &&
        globalThis
          .__reloadViews()
          .every((view) => !previous.includes(view.view.handle.instanceId) && !view.visualizerBusy),
      good,
    );
    const hot = await ids();
    // A source edit alone reaches every open instance after automatic observation is on.
    await page.evaluate(() => {
      globalThis.__workspaceFixture.sourceEpoch++;
    });
    await page.waitForFunction(
      (previous) =>
        globalThis
          .__reloadViews()
          .every((view) => !previous.includes(view.view.handle.instanceId) && !view.visualizerBusy),
      hot,
    );
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.visualizerHotReload = false;
      s.holdReload = true;
      s.holdReloadSlot = "traffic-response-body";
      s.emitNative("extensions-changed");
    });
    await invoke("reload_extension", { id: "example.protocol" });
    await page.waitForFunction(() => !!globalThis.__workspaceFixture.finishReload);
    await selector.selectOption("auto");
    await page.evaluate(() => {
      globalThis.__workspaceFixture.finishReload();
      delete globalThis.__workspaceFixture.finishReload;
    });
    await page.waitForTimeout(100);
    assert.equal(await visualizer.count(), 0, "Late replacement restored a hidden result");
    await selector.selectOption({ label: "Protocol structure" });
    await visualizer.locator(".visualizer-heading > strong").waitFor();
  } finally {
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.visualizerDevelopment = false;
      s.visualizerHotReload = false;
      s.reloadError = false;
      s.reloadEmitsMetadata = false;
      delete s.holdReloadSlot;
      if (s.finishReload) {
        s.finishReload();
        delete s.finishReload;
      }
      for (const viewer of globalThis.__reloadViews?.() ?? [])
        viewer.view.instance.development = false;
      delete globalThis.__reloadViews;
    });
  }
}
