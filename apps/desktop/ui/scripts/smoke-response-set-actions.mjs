import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { nativeResponseSetContext } from "./native-response-set-helpers.mjs";

// Current-set preview origin/report checks. Named milestone/live dialogs are
// exercised by smoke-saved-demos and the dedicated lifecycle browser/native run.
export async function smokeResponseSetActions(options) {
  const { main, directory, documentId, closeNative } = options,
    h = nativeResponseSetContext(options),
    { shell: s, a, sets: v, traffic: t, invoke } = h,
    warning = t + ".previewPageDialog",
    report = t + ".previewReportDialog",
    before = await invoke("automation_status");
  const asset = await invoke("create_response_asset", {
      input: {
        id: "action-native-asset",
        revision: 1,
        status: 200,
        headers: [],
        body: [...Buffer.from("h1 { color: rgb(0, 0, 255); }")],
        mediaType: "text/css",
      },
    }),
    action = { headers: [], replaceBody: null, discardBody: false, abortReason: null },
    rule = {
      id: "action-native-rule",
      displayName: "Native action CSS",
      revision: 1,
      priority: 1,
      enabled: true,
      matcher: {
        method: "GET",
        url: { kind: "exact", value: "https://example.invalid/page.css" },
        scheme: null,
        host: null,
        port: null,
        pathPrefix: null,
        query: null,
        requestHeaders: [],
        responseHeaders: [],
        responseStatus: null,
        responseStatusClass: null,
      },
      request: {
        ...action,
        responseAsset: asset.id + "@" + asset.revision,
        allowNonIdempotentBodyReplacement: false,
      },
      response: action,
    };
  let status = await invoke("change_response_set", {
    generation: before.generation,
    change: {
      action: "create",
      id: "native-actions",
      name: "Native current actions",
      notes: "Saved origin and diagnostic checks",
      rules: [rule],
    },
  });
  await h.openSet(status, "native-actions");
  status = await h.associate("native-actions", documentId);
  const pageSet = status.responseSets.find((set) => set.id === "native-actions"),
    selection = await main.evaluate(v + ".currentSelection()");
  const preview = async () => {
    await h.pointer(v + ".lifecycle.tryButton");
    await h.pointer(h.button(v + ".lifecycle.tryMenu", "Preview captured page…"));
    await h.wait(`${warning}.open&&!${t}.previewRulesBusy`);
    assert.equal(await main.evaluate(s + ".activeView"), "automation");
  };
  await preview();
  await h.key("Escape");
  assert.equal(
    await main.evaluate(`${v}.getRootNode().activeElement===${v}.lifecycle.tryButton`),
    true,
  );
  await preview();
  await h.pointer(h.button(warning, "Open preview"));
  await h.wait(`!${t}.previewPageBusy&&${t}.previewReportAvailable`);
  const modifiedLabel = await main.evaluate(t + ".previewReportLabel"),
    modified = await invoke("captured_page_report", { label: modifiedLabel });
  assert.deepEqual(modified.variant, selection);
  assert.equal(modified.scriptsEnabled, false);
  assert.equal(await main.evaluate(s + ".activeView"), "automation");
  assert.deepEqual(await invoke("automation_status"), status);
  await h.pointer(h.button(v, "Last preview diagnostics"));
  await h.wait(`${report}.open&&!${t}.previewReportBusy`);
  for (const [width, height] of [
    [1440, 900],
    [800, 600],
  ]) {
    await main.call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const image = await main.call("Page.captureScreenshot", { format: "png" });
    await writeFile(
      join(directory, `native-diagnostics-${width}.png`),
      Buffer.from(image.data, "base64"),
    );
  }
  await main.call("Emulation.clearDeviceMetricsOverride");
  await h.key("Escape");
  assert.equal(
    await main.evaluate(
      `${v}.getRootNode().activeElement===${h.button(v, "Last preview diagnostics")}`,
    ),
    true,
  );
  await main.evaluate(t + ".showPreviewReport()");
  await h.wait(`${report}.open&&!${t}.previewReportBusy`);
  await h.pointer(h.button(report, "Open baseline…"));
  await h.wait(`${warning}.open&&!${t}.previewRulesBusy`);
  assert.deepEqual(await main.evaluate(t + ".previewVariantSelection"), selection);
  await h.pointer(h.button(warning, "Open preview"));
  await h.wait(`!${t}.previewPageBusy&&!${warning}.open`);
  const baselineLabel = await main.evaluate(t + ".previewReportLabel"),
    baseline = await invoke("captured_page_report", { label: baselineLabel });
  assert.equal(baseline.variant ?? null, null);
  assert.deepEqual(baseline.rules, []);
  await main.evaluate(
    `(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks,original=callbacks.set;globalThis.releaseActionReport=null;callbacks.set=function(id,callback){return original.call(this,id,result=>{if(result?.url==='https://example.invalid/captured-page'&&Array.isArray(result.resources)&&Array.isArray(result.requests)&&result.variant==null)globalThis.releaseActionReport=()=>callback(result);else callback(result);});};})()`,
  );
  await h.pointer(h.button(v, "Last preview diagnostics"));
  await h.wait(`${report}.open&&${t}.previewReportBusy&&globalThis.releaseActionReport`);
  assert.equal(await main.evaluate(t + ".previewReportUrl"), "");
  assert.equal(
    await main.evaluate(
      `${t}.previewRequestRows.length+${t}.previewRuleRows.length+${t}.previewResourceRows.length`,
    ),
    0,
  );
  assert.equal(await main.evaluate(`${h.button(report, "Open updated preview…")}.disabled`), true);
  await main.evaluate(
    "delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releaseActionReport()",
  );
  await h.wait(`!${t}.previewReportBusy&&${t}.previewReportLoaded`);
  await h.pointer(h.button(report, "Open updated preview…"));
  await h.wait(`${warning}.open&&!${t}.previewRulesBusy`);
  assert.deepEqual(await main.evaluate(t + ".previewVariantSelection"), selection);
  await h.key("Escape");
  assert.deepEqual(await invoke("automation_status"), status);
  await closeNative("Baseline preview · Scripts off — Transmog");
  await closeNative("Modified preview · Scripts off — Transmog");
  await main.evaluate(a + ".leaveResponseSet()");
  await writeFile(
    join(directory, "native-actions-result.json"),
    JSON.stringify(
      {
        before,
        status,
        modifiedLabel,
        modified,
        baselineLabel,
        baseline,
        originPreserved: true,
        loadingReportCleared: true,
      },
      null,
      2,
    ),
  );
  return { before, status, pageSet, modifiedLabel, modified, baselineLabel, baseline, rule };
}
