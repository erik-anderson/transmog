import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { smokeResponseSetActions } from "./smoke-response-set-actions.mjs";
import { nativeResponseSetContext } from "./native-response-set-helpers.mjs";

export async function smokeResponseSetActionFollowup(options) {
  const { main, directory, closeNative } = options,
    h = nativeResponseSetContext(options),
    { shell: s, a, sets: v, traffic: t, invoke, button, pointer, key } = h,
    waitFor = h.wait,
    report = t + ".previewReportDialog";
  const fixture = await smokeResponseSetActions(options),
    { pageSet, modifiedLabel } = fixture;
  const shot = async (name) => {
    const image = await main.call("Page.captureScreenshot", { format: "png" });
    await writeFile(join(directory, name), Buffer.from(image.data, "base64"));
  };
  const delay = async (name, predicate) =>
    main.evaluate(
      `(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks;globalThis.${name}Set=callbacks.set;globalThis.${name}Release=null;callbacks.set=function(id,callback){return globalThis.${name}Set.call(this,id,result=>{if(${predicate})globalThis.${name}Release=()=>callback(result);else callback(result);});};})()`,
    );
  const release = async (name) =>
    main.evaluate(`delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.${name}Release()`);
  try {
    // These races select a second rule while its sibling's properties load. Use
    // the list/properties layout, as the browser check does: native window size
    // and display scaling can otherwise put the list behind Back to rules.
    await main.call("Emulation.setDeviceMetricsOverride", {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });
    const ordinaryBefore = await invoke("automation_status"),
      newerAsset = await invoke("create_response_asset", {
        input: {
          id: "action-native-newer-asset",
          revision: 1,
          status: 200,
          headers: [],
          body: [...Buffer.from("/* newer ordinary */")],
          mediaType: "application/javascript",
        },
      }),
      sourceRule = fixture.rule;
    const ordinary = {
        ...sourceRule,
        id: "ordinary-native",
        displayName: "Ordinary native response",
        priority: 3,
        matcher: {
          ...sourceRule.matcher,
          url: { kind: "exact", value: "https://example.invalid/ordinary.js" },
        },
      },
      newer = {
        ...sourceRule,
        id: "newer-native",
        displayName: "Newer native response",
        priority: 4,
        matcher: {
          ...sourceRule.matcher,
          url: { kind: "exact", value: "https://example.invalid/newer.js" },
        },
        request: {
          ...sourceRule.request,
          responseAsset: newerAsset.id + "@" + newerAsset.revision,
        },
      };
    const candidate = await invoke("validate_automation", {
      document: {
        schemaVersion: 2,
        generation: ordinaryBefore.generation,
        rules: [...ordinaryBefore.rules, ordinary, newer],
        responseSets: ordinaryBefore.responseSets,
        autoresponsesEnabled: ordinaryBefore.autoresponsesEnabled,
      },
    });
    await invoke("activate_automation", { candidateId: candidate.candidateId });
    const ordinaryStatus = await invoke("automation_status");
    const ordinaryLabel = await invoke("open_captured_page", {
      id: pageSet.previewPage.entryId,
      enableScripts: false,
      operationId: "owned-ordinary-focus-check",
      sourceLabel: null,
      options: {
        scope: "original-trace",
        ruleIds: [ordinary.id],
        generation: ordinaryStatus.generation,
      },
    });
    await main.evaluate(
      `(async()=>{${a}.draftDirty=false;${a}.editorHidden=true;${a}.renderAutomation(${JSON.stringify(ordinaryStatus)});${t}.previewReportSetId='';${t}.previewReportLabel=${JSON.stringify(ordinaryLabel)};await ${t}.showPreviewReport();})()`,
    );
    await waitFor(`${report}.open&&!${t}.previewReportBusy`);
    await delay(
      "followAsset",
      "result?.asset?.id==='action-native-asset'&&typeof result.display==='string'",
    );
    await pointer(button(report, "Show saved rule"));
    await waitFor(`${a}.loadingSavedResponse&&globalThis.followAssetRelease`);
    const nameField = a + ".autoResponseForm.elements.namedItem('name')";
    await pointer(nameField);
    await key("a", "KeyA", 65, 2);
    await main.call("Input.insertText", { text: "Native typed rule draft" });
    await release("followAsset");
    await waitFor(`!${a}.loadingSavedResponse`);
    assert.equal(await main.evaluate(`${nameField}.value`), "Native typed rule draft");
    assert.equal(await main.evaluate(`${a}.getRootNode().activeElement===${nameField}`), true);
    assert.notEqual(await main.evaluate(`${a}.highlightedRuleId`), ordinary.id);
    await shot("native-rule-draft-focus.png");
    await main.evaluate(
      `(async()=>{${a}.draftDirty=false;${a}.editorHidden=true;await ${t}.showPreviewReport();})()`,
    );
    await delay(
      "followAssetSelection",
      "result?.asset?.id==='action-native-asset'&&typeof result.display==='string'",
    );
    await pointer(button(report, "Show saved rule"));
    await waitFor(`${a}.loadingSavedResponse&&globalThis.followAssetSelectionRelease`);
    await pointer(`${a}.querySelector('#rule-newer-native .rule-description')`);
    await waitFor(`${a}.editingAutoResponseId==='newer-native'&&!${a}.loadingSavedResponse`);
    await pointer(`${a}.querySelector('#rule-newer-native .rule-description')`);
    await release("followAssetSelection");
    await main.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    assert.equal(await main.evaluate(`${a}.editingAutoResponseId`), newer.id);
    assert.equal(await main.evaluate(`${a}.getRootNode().activeElement?.id`), "rule-newer-native");
    assert.equal(await main.evaluate(`${nameField}.value`), "Newer native response");
    await main.evaluate(`${t}.showPreviewReport()`);
    await pointer(button(report, "Show saved rule"));
    await waitFor(`${a}.getRootNode().activeElement?.id==='rule-ordinary-native'`);
    const ordinaryReport = await invoke("captured_page_report", { label: ordinaryLabel });
    assert.equal(ordinaryReport.rules[0].id, ordinary.id);
    assert.deepEqual(await invoke("automation_status"), ordinaryStatus);
    // The reveal owns its unsaved-draft confirmation. Keep editing preserves the
    // draft; explicit Discard/Save can continue to ordinary and named targets.
    const unsaved = a + ".unsavedDialog",
      dirtyName = async (text) => {
        await pointer(nameField);
        await key("a", "KeyA", 65, 2);
        await main.call("Input.insertText", { text });
      },
      requestRule = async () => {
        await main.evaluate(`${t}.showPreviewReport()`);
        await pointer(button(report, "Show saved rule"));
        await waitFor(`${unsaved}.open`);
      };
    await pointer(`${a}.querySelector('#rule-newer-native .rule-description')`);
    await waitFor(`${a}.editingAutoResponseId==='newer-native'&&!${a}.loadingSavedResponse`);
    await dirtyName("Keep native confirmation draft");
    await requestRule();
    await pointer(button(unsaved, "Keep editing"));
    await waitFor(`!${unsaved}.open`);
    assert.equal(await main.evaluate(`${a}.editingAutoResponseId`), newer.id);
    assert.equal(await main.evaluate(`${nameField}.value`), "Keep native confirmation draft");
    await requestRule();
    await pointer(button(unsaved, "Discard changes"));
    await waitFor(`${a}.getRootNode().activeElement?.id==='rule-ordinary-native'`);
    assert.deepEqual(await invoke("automation_status"), ordinaryStatus);
    await pointer(`${a}.querySelector('#rule-newer-native .rule-description')`);
    await waitFor(`${a}.editingAutoResponseId==='newer-native'&&!${a}.loadingSavedResponse`);
    await dirtyName("Native confirmed rule save");
    await requestRule();
    await pointer(button(unsaved, "Save and continue"));
    await waitFor(`${a}.getRootNode().activeElement?.id==='rule-ordinary-native'`);
    const confirmedSave = await invoke("automation_status");
    assert.equal(
      confirmedSave.rules.find((rule) => rule.id === newer.id).displayName,
      "Native confirmed rule save",
    );
    assert.equal(await main.evaluate(`${nameField}.value`), "Ordinary native response");
    await dirtyName("Discard native draft for named target");
    await main.evaluate(
      `${t}.previewReportLabel=${JSON.stringify(modifiedLabel)};${t}.previewReportSetId='native-actions'`,
    );
    await requestRule();
    await pointer(button(unsaved, "Discard changes"));
    await waitFor(
      `${a}.editorHidden&&${a}.automationSection==='experiments'&&${a}.collectionOpen&&${v}.setId==='native-actions'&&${a}.getRootNode().activeElement===${v}.backButton`,
    );
    assert.equal(await main.evaluate(`${v}.setId`), "native-actions");
    assert.deepEqual(await invoke("automation_status"), confirmedSave);
    await shot("native-rule-confirmation-completed.png");
    // Supersession after an actual Discard decision must leave an unchanged old
    // editor dirty. A later successful Discard consumes it by loading the target.
    await main.evaluate(
      `(async()=>{await ${a}.leaveResponseSet();await ${a}.editAutoResponse(${JSON.stringify(confirmedSave.rules.find((rule) => rule.id === newer.id))},true);${t}.previewReportLabel=${JSON.stringify(ordinaryLabel)};${t}.previewReportSetId='';})()`,
    );
    await dirtyName("Protected native aborted draft");
    await main.evaluate(
      `(()=>{const a=${a},canLeave=a.canLeaveEditor;globalThis.releaseNativeRevealConfirmation=null;a.canLeaveEditor=async function(){a.canLeaveEditor=canLeave;const allowed=await canLeave.call(a);await new Promise(resolve=>globalThis.releaseNativeRevealConfirmation=resolve);return allowed;};})()`,
    );
    await requestRule();
    await pointer(button(unsaved, "Discard changes"));
    await waitFor("globalThis.releaseNativeRevealConfirmation");
    await main.evaluate(
      `${t}.previewReportGeneration++;globalThis.releaseNativeRevealConfirmation()`,
    );
    await waitFor(`!${unsaved}.open&&${a}.draftDirty`);
    assert.equal(await main.evaluate(`${nameField}.value`), "Protected native aborted draft");
    assert.equal(await main.evaluate(`${a}.editingAutoResponseId`), newer.id);
    assert.deepEqual(await invoke("automation_status"), confirmedSave);
    await shot("native-aborted-reveal-draft.png");
    await requestRule();
    await pointer(button(unsaved, "Discard changes"));
    await waitFor(`${a}.getRootNode().activeElement?.id==='rule-ordinary-native'`);
    await dirtyName("Discard same native rule");
    await requestRule();
    await pointer(button(unsaved, "Discard changes"));
    await waitFor(
      `${a}.getRootNode().activeElement?.id==='rule-ordinary-native'&&!${a}.loadingSavedResponse`,
    );
    assert.equal(await main.evaluate(`${nameField}.value`), "Ordinary native response");
    assert.equal(await main.evaluate(`${a}.draftDirty`), false);
    // A genuine removed historical rule must be detected before any prompt.
    await pointer(`${a}.querySelector('#rule-newer-native .rule-description')`);
    await waitFor(`${a}.editingAutoResponseId==='newer-native'&&!${a}.loadingSavedResponse`);
    await dirtyName("Protected native missing-target draft");
    const missingCandidate = await invoke("validate_automation", {
      document: {
        schemaVersion: 2,
        generation: confirmedSave.generation,
        rules: confirmedSave.rules.filter((rule) => rule.id !== ordinary.id),
        responseSets: confirmedSave.responseSets,
        autoresponsesEnabled: confirmedSave.autoresponsesEnabled,
      },
    });
    await invoke("activate_automation", { candidateId: missingCandidate.candidateId });
    const missingStatus = await invoke("automation_status");
    await main.evaluate(`${t}.showPreviewReport()`);
    await pointer(button(report, "Show saved rule"));
    await waitFor(`${s}.noticeVisible&&${s}.noticeTitleText==='Historical rule'`);
    assert.equal(await main.evaluate(`${unsaved}.open`), false);
    assert.equal(
      await main.evaluate(`${nameField}.value`),
      "Protected native missing-target draft",
    );
    assert.equal(await main.evaluate(`${a}.draftDirty`), true);
    assert.deepEqual(await invoke("automation_status"), missingStatus);
    await shot("native-missing-rule-draft.png");
    await writeFile(
      join(directory, "native-followup-result.json"),
      JSON.stringify(
        {
          ordinaryLabel,
          ordinaryReport,
          ordinaryFixtureStatus: ordinaryStatus,
          confirmedRuleSave: confirmedSave,
          missingRuleFixtureStatus: missingStatus,
          final: await invoke("automation_status"),
          lifecycle: (await invoke("app_status")).lifecycle,
        },
        null,
        2,
      ),
    );
  } finally {
    await main.call("Emulation.clearDeviceMetricsOverride");
    await closeNative("Modified preview · Scripts off — Transmog");
    await main.evaluate(`${a}.draftDirty=false;${a}.editorHidden=true`);
    await h.cleanup(fixture.before);
  }
}
