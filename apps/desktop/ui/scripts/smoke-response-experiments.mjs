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
    assert.equal(await main.evaluate(`${editor}.automationSection`),'experiments','Native experiment opened outside Experiments');
    await main.evaluate(`(()=>{const a=${editor},name=a.autoResponseForm.elements.namedItem('name');name.value='Native CSS hypothesis';name.dispatchEvent(new Event('input',{bubbles:true}));a.autoResponseEditBody.checked=true;a.updateAutoResponseBodyEditState();a.autoResponseBody.value='h1 { color: rgb(0, 0, 255); }';a.autoResponseBody.dispatchEvent(new Event('input',{bubbles:true}));a.compareResponseDraft('source');})()`);
    await waitFor(()=>main.evaluate(`${shell}.comparison.comparisonResult&&!${shell}.comparison.comparisonBusy`),'Native experiment comparison did not complete');
    assert.equal(await main.evaluate(`${shell}.comparison.comparisonResult.rows.some(row=>row.field==='Decoded bytes'&&row.state==='different')`),true);
    await main.evaluate(`${shell}.comparison.closeComparison()`);
    if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-experiment-editor.png'),Buffer.from(image.data,'base64'));}
    assert.equal(await main.evaluate(`${editor}.saveAutoResponse(new Event('submit',{cancelable:true}))`),true,'Native experiment save failed');
    await waitFor(()=>main.evaluate(`${editor}.editorHidden&&!${editor}.loadingSavedResponse`),'Saved experiment did not return to Experiments');
    assert.equal(await main.evaluate(`${editor}.automationSection==='experiments'&&${editor}.editorHidden`),true,'Native save did not return to Experiments');
    assert.deepEqual(await main.evaluate(`(()=>{const v=${editor}.experimentVariants;return {experimentId:v.experimentId,variantId:v.variantId,revision:v.variantRevision};})()`),await main.evaluate(`${editor}.editingVariant`));
    const ruleId=await main.evaluate(`${editor}.editingAutoResponseId`);
    const saved=await invoke('automation_status');
    assert.equal(saved.rules.some(row=>row.id===ruleId),false);
    assert.equal(saved.autoresponsesEnabled,before.autoresponsesEnabled);
    assert.deepEqual(saved.rules.filter(row=>row.id!==ruleId),before.rules);
    const prior=(await targets()).map(row=>row.id);
    await main.evaluate(`${editor}.experimentVariants.previewVariant()`);
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
    assert.deepEqual(report.variant,await main.evaluate(`${editor}.editingVariant`));
    assert.ok(report.requests.some(row=>row.ruleId===ruleId&&row.outcome==='auto-response'));
    assert.equal((await invoke('automation_status')).rules.some(row=>row.id===ruleId),false);
    const original=saved.experiments.find(e=>e.variants.some(v=>v.revisions.some(r=>r.rules.some(rule=>rule.id===ruleId))));
    const originalSelection=await main.evaluate(`${editor}.editingVariant`);
    let status=await invoke('change_experiment',{generation:saved.generation,change:{action:'duplicate',selection:originalSelection,variantId:'native-alternative',name:'Native alternate CSS'}});
    const variant=status.experiments.find(e=>e.id===original.id).variants.find(v=>v.id==='native-alternative');
    const alternative={experimentId:original.id,variantId:variant.id,revision:1};
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'activate',selection:alternative,acknowledgeOverlaps:true}});
    assert.equal(status.autoresponsesEnabled,before.autoresponsesEnabled);assert.deepEqual(status.rules.filter(r=>!variant.revisions[0].rules.some(owned=>owned.id===r.id)),before.rules);
    const activeRules=structuredClone(status.rules);
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'pause',id:original.id,paused:true}});
    assert.deepEqual(status.rules,activeRules);assert.equal(status.experiments.find(e=>e.id===original.id).paused,true);
    await main.evaluate(`(async()=>{const a=${editor};a.renderAutomation(${JSON.stringify(status)});await ${shell}.activateView('automation');a.showAutomationSection('experiments');await a.experimentVariants.reveal(${JSON.stringify(alternative)});})()`);
    assert.equal(await main.evaluate(`${editor}.experimentVariants.variantRules.length`),1);
    if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-named-variants.png'),Buffer.from(image.data,'base64'));}
    // Variant selection is passive; explicit Baseline activation removes only this collection.
    await main.evaluate(`${editor}.experimentVariants.selectVariant({currentTarget:{value:'baseline'}})`);
    assert.deepEqual((await invoke('automation_status')).rules,activeRules);
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'activate',selection:{experimentId:original.id,variantId:'baseline',revision:1},acknowledgeOverlaps:false}});
    assert.deepEqual(status.rules,before.rules);
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'undo-activation',id:original.id}});assert.deepEqual(status.rules,activeRules);
    // The already-open preview still displays its frozen selected revision.
    assert.equal(await preview.evaluate(`getComputedStyle(document.getElementById('captured-heading')).color`),'rgb(0, 0, 255)');
    await closeNative('Modified preview · Scripts off — Transmog');
    preview.close();
    await waitFor(async()=>!(await targets()).some(row=>row.id===target.id),'Native experiment preview did not close');
    process.stdout.write('Native response experiment: captured CSS → saved variant → exact-revision preview → duplicate → scoped activation → pause → Baseline → Undo; unrelated rules, global gate and open preview preserved.\n');
  } finally {
    let current=await invoke('automation_status');
    for(const e of current.experiments??[]){if((before.experiments??[]).some(old=>old.id===e.id))continue;current=await invoke('change_experiment',{generation:current.generation,change:{action:'activate',selection:{experimentId:e.id,variantId:'baseline',revision:1},acknowledgeOverlaps:false}});current=await invoke('change_experiment',{generation:current.generation,change:{action:'archive',id:e.id,variantId:null,archived:true}});}
    const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:before.rules,autoresponsesEnabled:before.autoresponsesEnabled}});
    await invoke('activate_automation',{candidateId:candidate.candidateId});
    await main.evaluate(`${shell}.activateView('traffic')`);
  }
}
