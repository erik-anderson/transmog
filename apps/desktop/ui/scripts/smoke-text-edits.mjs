import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
export async function smokeTextEdits({main,viewer,waitFor,screenshot}) {
 const shell="document.querySelector('app-shell')",editor=shell+".automation.querySelector('[editor-kind=\"response-text\"]')";
 const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
 const before=await invoke('automation_status');
 try {
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${shell}.automation.showAutomationSection('response-text');await ${editor}.newRule();})()`);
  await main.evaluate(`(()=>{const h=${editor};h.hrName='Native literal hypothesis';h.hrMatcher.matchMode='exact';h.hrMatcher.addressInput.value='https://example.test/fresh';h.hrMatcher.changed();h.hrFind='old';h.hrReplacement='new $& é';h.hrExpected='2';h.hrSampleUrl='https://example.test/fresh';h.hrSampleBody='old token=keep old';h.testDraft();})()`);
  await waitFor(()=>main.evaluate(`!${editor}.hrTestBusy`),'Native literal test stayed busy');
  assert.equal(await main.evaluate(`${editor}.hrAfterText`),'new $& é token=keep new $& é');assert.match(await main.evaluate(`${editor}.hrBodySummary`),/2 matches · 2 replacements/);
  assert.deepEqual((await invoke('automation_status')).rules,before.rules);
  await main.evaluate(`${editor}.hrExpected='3';${editor}.testDraft()`);await waitFor(()=>main.evaluate(`!${editor}.hrTestBusy`),'Native required-count test stayed busy');assert.match(await main.evaluate(`${editor}.hrBodySummary`),/Failed: required match count/);assert.equal(await main.evaluate(`${editor}.hrAfterText`),'old token=keep old');
  await main.evaluate(`${editor}.hrExpected='2'`);assert.equal(await main.evaluate(`${editor}.save()`),true);let state=await invoke('automation_status');const saved=state.rules.find(r=>r.displayName==='Native literal hypothesis');assert.equal(saved.enabled,false);assert.equal(state.autoresponsesEnabled,before.autoresponsesEnabled);
  assert.equal(await main.evaluate(`${editor}.save(undefined,true)`),true);state=await invoke('automation_status');assert.equal(state.rules.find(r=>r.id===saved.id).enabled,true);
  for(const command of ['change_response_transform','test_response_transform'])assert.equal(await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},{}).then(()=>false,()=>true)`),true);
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-text-edits.png'),Buffer.from(image.data,'base64'));}
  await main.evaluate(`${editor}.remove()`);await main.evaluate(`${editor}.undoRemove()`);assert.equal((await invoke('automation_status')).rules.some(r=>r.id===saved.id),true);
  process.stdout.write('Native literal response edits: exact UTF-8 literals, total-count failure, inert before/after sample, passive save, explicit enable, viewer denial and remove/Undo verified.\n');
 } finally {const current=await invoke('automation_status');const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:before.rules,experiments:before.experiments,autoresponsesEnabled:before.autoresponsesEnabled}});const state=await invoke('activate_automation',{candidateId:candidate.candidateId});await main.evaluate(`${shell}.automation.renderAutomation(${JSON.stringify(state)})`);}
}
