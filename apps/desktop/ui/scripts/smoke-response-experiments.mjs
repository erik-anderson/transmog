import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

// Uses production workspace actions and real native commands, in the harness's isolated app data.
export async function smokeResponseExperiments({main,traffic,documentId,targets,connect,waitFor,closeNative,screenshot}) {
  const shell="document.querySelector('app-shell')",editor=shell+'.automation';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const before=await invoke('automation_status');
  try {
    const source=await main.evaluate(`(async()=>{const t=${traffic};const row=t.sessions.find(row=>row.path==='/page.css');await t.revealSession(row.id);t.beginExperimentFromSelected();return row.id;})()`);
    await waitFor(()=>main.evaluate(`${editor}.responseExperiment&&!${editor}.editBodyDisabled&&!${editor}.loadingSavedResponse`),'Native experiment draft did not load');
    await main.evaluate(`(()=>{const a=${editor},name=a.autoResponseForm.elements.namedItem('name');name.value='Native CSS hypothesis';name.dispatchEvent(new Event('input',{bubbles:true}));a.autoResponseEditBody.checked=true;a.updateAutoResponseBodyEditState();a.autoResponseBody.value='h1 { color: rgb(0, 0, 255); }';a.autoResponseBody.dispatchEvent(new Event('input',{bubbles:true}));a.compareResponseDraft('source');})()`);
    await waitFor(()=>main.evaluate(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`),'Native experiment comparison did not complete');
    assert.equal(await main.evaluate(`${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`),true);
    await main.evaluate(`${shell}.comparison.closeComparison()`);
    if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-experiment-editor.png'),Buffer.from(image.data,'base64'));}
    assert.equal(await main.evaluate(`${editor}.saveAutoResponse(new Event('submit',{cancelable:true}))`),true,'Native experiment save failed');
    await waitFor(()=>main.evaluate(`!${editor}.loadingSavedResponse`),'Saved experiment did not reopen');
    const ruleId=await main.evaluate(`${editor}.editingAutoResponseId`);
    const saved=await invoke('automation_status');
    assert.equal(saved.rules.find(row=>row.id===ruleId).enabled,false);
    assert.equal(saved.autoresponsesEnabled,before.autoresponsesEnabled);
    assert.deepEqual(saved.rules.filter(row=>row.id!==ruleId),before.rules);
    const prior=(await targets()).map(row=>row.id);
    await main.evaluate(`${editor}.previewSavedRule()`);
    await waitFor(()=>main.evaluate(`${traffic}.previewPageDialog.open&&!${traffic}.previewRulesBusy`),'Native experiment page choices did not open');
    assert.equal(await main.evaluate(`${traffic}.selectedSessionId`),source,'Experiment changed the inspected CSS');
    assert.equal(await main.evaluate(`${traffic}.previewDocumentChoices.some(row=>row.id===${JSON.stringify(documentId)})`),true);
    await main.evaluate(`(()=>{const t=${traffic};t.previewDocumentChanged({currentTarget:{value:${JSON.stringify(documentId)}}});})()`);
    await main.evaluate(`${traffic}.openCapturedPage()`);
    const target=await waitFor(async()=>(await targets()).find(row=>!prior.includes(row.id)&&row.url==='https://example.invalid/captured-page'),'Native experiment page did not navigate');
    const preview=await connect(target);
    await waitFor(()=>preview.evaluate(`getComputedStyle(document.getElementById('captured-heading')).color==='rgb(0, 0, 255)'`),'Native experiment did not serve edited CSS');
    const label=await main.evaluate(`${traffic}.previewReportLabel`);
    const report=await invoke('captured_page_report',{label});
    assert.deepEqual(report.rules.map(row=>row.id),[ruleId]);
    assert.ok(report.requests.some(row=>row.ruleId===ruleId&&row.outcome==='auto-response'));
    assert.equal((await invoke('automation_status')).rules.find(row=>row.id===ruleId).enabled,false);
    await closeNative('Modified preview · Scripts off — Transmog');
    await waitFor(async()=>!(await targets()).some(row=>row.id===target.id),'Native experiment preview did not close');
    process.stdout.write('Native response experiment: captured CSS → disabled draft → edit → compare → save → choose page → modified WebView2 render; unrelated rules and live gate preserved.\n');
  } finally {
    const current=await invoke('automation_status');
    const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:before.rules,autoresponsesEnabled:before.autoresponsesEnabled}});
    await invoke('activate_automation',{candidateId:candidate.candidateId});
    await main.evaluate(`${shell}.activateView('traffic')`);
  }
}
