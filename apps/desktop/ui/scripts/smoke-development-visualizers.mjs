import assert from "node:assert/strict";
import { readFile, writeFile, realpath } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

/** Settings requests and actual filesystem changes reach every visible native instance. */
export async function smokeDevelopmentVisualizers(evaluate, tracePath) {
  const invoke = (command, args = {}) =>
    evaluate(
      `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`,
    );
  const entry = (await invoke("extension_status")).entries.find(
    (entry) => entry.catalog.manifest.id === "test.development",
  );
  const directory = await realpath(join(dirname(tracePath), "extension-source"));
  assert.equal(
    resolve(entry.directory).toLowerCase(),
    directory.toLowerCase(),
    "Development probe must use its owned source fixture",
  );
  const path = join(directory, "parser.js"),
    original = await readFile(path, "utf8");
  await evaluate(
    `(()=>{const t=document.querySelector('app-shell').traffic;globalThis.__developmentViews=()=>[...(t.shadowRoot??t).querySelectorAll('message-inspector')].flatMap(i=>[...(i.shadowRoot??i).querySelectorAll('extension-visualizer')]).filter(v=>v.view?.instance.extensionId==='test.development');})()`,
  );
  const snapshot = () =>
    evaluate(
      `__developmentViews().map(v=>({id:v.view.handle.instanceId,title:v.view.document.title,busy:v.visualizerBusy,notice:v.visualizerReloadNotice}))`,
    );
  const wait = async (condition) => {
    const until = Date.now() + 30000;
    for (;;) {
      const views = await snapshot();
      if (views.length === 3 && condition(views)) return views;
      if (Date.now() > until)
        throw new Error("Native development reload did not settle: " + JSON.stringify(views));
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  };
  const request = () =>
    evaluate(
      `document.querySelector('app-shell').settings.extensionsEditor.reload('test.development')`,
    );
  try {
    assert.equal((await snapshot()).length, 3);
    await invoke("extension_hot_reload", { id: "test.development", enabled: false });
    const before = (await snapshot()).map((view) => view.id);
    const changed = original.replace("title:'Synthetic protocol'", "title:'Reloaded protocol'");
    assert.notEqual(changed, original);
    await writeFile(path, changed);
    await new Promise((resolve) => setTimeout(resolve, 700));
    assert.deepEqual(
      (await snapshot()).map((view) => view.id),
      before,
      "Hot reload off still replaced code",
    );
    await request();
    const good = await wait((views) =>
      views.every(
        (view) => view.title === "Reloaded protocol" && !before.includes(view.id) && !view.busy,
      ),
    );
    await writeFile(
      path,
      "throw new Error('synthetic development failure'); export function visualize(){}",
    );
    await request();
    const failed = await wait((views) =>
      views.every((view) => view.notice.includes("Reload failed:") && !view.busy),
    );
    assert.deepEqual(
      failed.map((view) => view.id),
      good.map((view) => view.id),
      "A failed candidate replaced a good native worker",
    );
    await writeFile(path, original);
    await invoke("extension_hot_reload", { id: "test.development", enabled: true });
    await wait((views) => views.every((view) => view.title === "Synthetic protocol" && !view.busy));
    await writeFile(
      path,
      original.replace("title:'Synthetic protocol'", "title:'Hot reloaded protocol'"),
    );
    await wait((views) =>
      views.every((view) => view.title === "Hot reloaded protocol" && !view.busy),
    );
    await writeFile(path, original);
    await wait((views) => views.every((view) => view.title === "Synthetic protocol" && !view.busy));
    await evaluate(
      `globalThis.__visualizerProbe.handles=__developmentViews().map(view=>view.view.handle)`,
    );
    return {
      settingsOnly: true,
      manualAllInstances: true,
      hotOffPreservedWorkers: true,
      failedCandidatesPreserved: true,
      enableRebuiltMissedEdits: true,
      sourceChangesReloadedAllInstances: true,
    };
  } finally {
    await writeFile(path, original);
    await invoke("extension_hot_reload", { id: "test.development", enabled: entry.hotReload });
    await evaluate("delete globalThis.__developmentViews");
  }
}
