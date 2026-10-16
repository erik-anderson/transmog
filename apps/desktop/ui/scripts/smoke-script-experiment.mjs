import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

// Cross-feature acceptance against real commands and WebView2, with isolated app data.
export async function smokeScriptExperiment({main,traffic,documentId,targets,connect,waitFor,closeNative,screenshot}) {
  const shell="document.querySelector('app-shell')",editor=shell+'.automation';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const before=await invoke('automation_status');
  let preview;
  try {
    const source=await main.evaluate(`(async()=>{const t=${traffic};const row=t.sessions.find(row=>row.path==='/experiment.js');await t.revealSession(row.id);t.beginExperimentFromSelected();return row.id;})()`);
    await waitFor(()=>main.evaluate(`${editor}.sourceSessionId===${JSON.stringify(source)}&&${editor}.responseExperiment&&!${editor}.editBodyDisabled&&!${editor}.loadingSavedResponse`),'Captured script experiment did not load');
    const original=await main.evaluate(`${editor}.autoResponseBody.value`);
    assert.ok(original.includes('"original"'));
    const edited=original.replace('"original"','"patched"');
    await main.evaluate(`(()=>{const a=${editor};const name=a.autoResponseForm.elements.namedItem('name');name.value='Captured script with API dependency';name.dispatchEvent(new Event('input',{bubbles:true}));a.autoResponseEditBody.checked=true;a.updateAutoResponseBodyEditState();a.autoResponseBody.value=${JSON.stringify(edited)};a.autoResponseBody.dispatchEvent(new Event('input',{bubbles:true}));a.compareResponseDraft('source');})()`);
    await waitFor(()=>main.evaluate(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`),'Script draft comparison did not finish');
    assert.equal(await main.evaluate(`${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`),true);
    await main.evaluate(`${shell}.comparison.closeComparison()`);
    assert.equal(await main.evaluate(`${editor}.saveAutoResponse(new Event('submit',{cancelable:true}))`),true);
    await waitFor(()=>main.evaluate(`!${editor}.loadingSavedResponse`),'Saved script experiment did not reopen');
    const selection=await main.evaluate(`${editor}.editingVariant`);
    const saved=await invoke('automation_status');
    assert.deepEqual(saved.rules,before.rules,'Saving a script experiment changed live rules');
    assert.equal(saved.autoresponsesEnabled,before.autoresponsesEnabled);
    assert.equal(await main.evaluate('globalThis.experimentScriptValue??null'),null,'Captured script executed in the trusted workbench');
    try {
      await main.call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:1,mobile:false});
      await main.evaluate(`document.documentElement.style.zoom='1.5'`);
      for(const name of ['Preview…','Use for live traffic','Save experiment']){
        assert.equal(await main.evaluate(`(()=>{const button=Array.from(${editor}.querySelectorAll('.auto-response-editor-actions button')).find(row=>row.textContent.trim()===${JSON.stringify(name)});if(!button)return false;button.focus();button.scrollIntoView({block:'nearest'});const box=button.getBoundingClientRect();return box.top>=0&&box.bottom<=innerHeight&&button.contains(button.getRootNode().elementFromPoint(box.x+box.width/2,box.y+box.height/2));})()`),true,`${name} is unreachable in native compact/zoom layout`);
      }
      if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-script-editor-zoom.png'),Buffer.from(image.data,'base64'));}
    } finally {
      await main.evaluate(`document.documentElement.style.zoom=''`);
      await main.call('Emulation.clearDeviceMetricsOverride');
    }

    const prior=(await targets()).map(row=>row.id);
    await main.evaluate(`${editor}.previewSavedRule()`);
    await waitFor(()=>main.evaluate(`${traffic}.previewPageDialog.open&&!${traffic}.previewRulesBusy`),'Script preview page chooser did not open');
    assert.equal(await main.evaluate(`${traffic}.selectedSessionId`),source,'Page chooser replaced the inspected script');
    await main.evaluate(`(()=>{const t=${traffic};t.previewDocumentChanged({currentTarget:{value:${JSON.stringify(documentId)}}});t.previewPageScripts.checked=true;t.openCapturedPage();})()`);
    const target=await waitFor(async()=>(await targets()).find(row=>!prior.includes(row.id)&&row.url==='https://example.invalid/captured-page'),'Script experiment preview did not navigate');
    preview=await connect(target);
    await waitFor(()=>preview.evaluate('globalThis.experimentApi'),'Captured script API dependency did not load');
    assert.equal(await preview.evaluate('globalThis.experimentScriptValue'),'patched');
    assert.deepEqual(await preview.evaluate('globalThis.experimentApi'),{token:'captured-token',value:42});
    assert.equal(await preview.evaluate(`document.getElementById('experiment-result').textContent`),'patched: 42');
    assert.equal(await preview.evaluate(`getComputedStyle(document.getElementById('captured-heading')).color`),'rgb(0, 128, 0)');
    assert.equal(await preview.evaluate(`document.getElementById('captured-image').naturalWidth`),20);
    const policies=await preview.evaluate(`Promise.all(['/experiment.js','/experiment-api','/captured-page'].map(url=>fetch(url).then(r=>({cache:r.headers.get('cache-control'),policy:r.headers.get('content-security-policy')}))))`);
    assert.deepEqual(policies.map(row=>row.cache),['max-age=3600','private, max-age=60','max-age=60']);
    assert.ok(policies.every(row=>row.policy.includes("worker-src 'none'")),'Preview lost mandatory isolation CSP');
    assert.ok(policies[2].policy.includes("default-src 'self'; script-src 'self' 'unsafe-inline'"),'Preview lost captured document CSP');
    const label=await main.evaluate(`${traffic}.previewReportLabel`);
    const report=await invoke('captured_page_report',{label});
    assert.deepEqual(report.variant,selection);
    assert.equal(report.sourceTraceId,await main.evaluate(`${traffic}.sessions.find(row=>row.id===${JSON.stringify(source)}).traceId`));
    const script=report.requests.find(row=>row.url==='https://example.invalid/experiment.js');
    assert.ok(script,'Preview did not record the supplied script');
    assert.equal(script.outcome,'auto-response');assert.ok(script.sha256);
    assert.ok(report.requests.some(row=>row.url==='https://example.invalid/experiment-api'&&row.outcome==='captured'&&row.entryId));
    assert.deepEqual((await invoke('automation_status')).rules,before.rules,'Preview activated the script experiment');
    await main.evaluate(`${traffic}.comparePreviewResponse(${JSON.stringify(String(script.id))},${JSON.stringify(source)},${JSON.stringify(script.ruleId)})`);
    await waitFor(()=>main.evaluate(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`),'Supplied script comparison did not finish');
    assert.equal(await main.evaluate(`${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`),true);
    await main.evaluate(`${shell}.comparison.closeComparison()`);
    if(screenshot){
      await writeFile(screenshot.replace('.png','-script-experiment-report.json'),JSON.stringify(report,null,2));
      const image=await preview.call('Page.captureScreenshot',{format:'png'});
      await writeFile(screenshot.replace('.png','-script-experiment.png'),Buffer.from(image.data,'base64'));
    }

    // Explicit activation is separate from save/preview and preserves all other live rules.
    await main.evaluate(`${editor}.useExperimentForLive()`);
    await waitFor(()=>main.evaluate(`!${editor}.experimentVariants.variantBusy`),'Script activation did not finish');
    const active=await invoke('automation_status');
    const experiment=active.experiments.find(row=>row.id===selection.experimentId);
    assert.deepEqual(experiment.active,selection);
    assert.deepEqual(active.rules.filter(row=>row.id!==script.ruleId),before.rules);
    assert.equal(active.autoresponsesEnabled,before.autoresponsesEnabled);
    const baseline=await invoke('change_experiment',{generation:active.generation,change:{action:'activate',selection:{experimentId:selection.experimentId,variantId:'baseline',revision:1},acknowledgeOverlaps:false}});
    assert.deepEqual(baseline.rules,before.rules);
    await preview.call('Page.reload',{ignoreCache:false});
    await waitFor(()=>preview.evaluate('globalThis.experimentApi'),'Frozen script preview did not reload');
    assert.equal(await preview.evaluate('globalThis.experimentScriptValue'),'patched','Live Baseline changed an open preview');
    await closeNative('Modified preview · Scripts on — Transmog');
    preview.close();
    await waitFor(async()=>!(await targets()).some(row=>row.id===target.id),'Script experiment preview did not close');
    process.stdout.write('Native captured JavaScript: inert draft → original/edited comparison → passive save → script-enabled preview with captured HTML/CSS/image/API and source/hash evidence → supplied-response comparison → explicit activation → Baseline; frozen warm reload and unrelated rules preserved.\n');
  } finally {
    preview?.close();
    let current=await invoke('automation_status');
    for(const experiment of current.experiments??[]){
      if((before.experiments??[]).some(row=>row.id===experiment.id))continue;
      current=await invoke('change_experiment',{generation:current.generation,change:{action:'activate',selection:{experimentId:experiment.id,variantId:'baseline',revision:1},acknowledgeOverlaps:false}});
      current=await invoke('change_experiment',{generation:current.generation,change:{action:'archive',id:experiment.id,variantId:null,archived:true}});
    }
    const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:before.rules,autoresponsesEnabled:before.autoresponsesEnabled}});
    const restored=await invoke('activate_automation',{candidateId:candidate.candidateId});
    await main.evaluate(`${editor}.renderAutomation(${JSON.stringify(restored)});${shell}.activateView('traffic')`);
  }
}
