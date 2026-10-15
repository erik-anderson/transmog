import assert from 'node:assert/strict';
import {writeFile,unlink} from 'node:fs/promises';
import {dirname,join} from 'node:path';

export async function smokeResponseFiles({main,traffic,documentId,pageSource,targets,connect,waitFor,closeNative,screenshot}){
 const shell="document.querySelector('app-shell')",editor=shell+'.automation';
 const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
 const before=await invoke('automation_status'),path=join(dirname(pageSource),'editable style.css');
 const color=page=>page.evaluate("document.getElementById('captured-heading')&&getComputedStyle(document.getElementById('captured-heading')).color");
 const blue='h1 { color: rgb(0, 0, 255); }',red='h1 { color: rgb(255, 0, 0); }';
 const choose=async mode=>main.evaluate(`(async()=>{const a=${editor},info=await window.__TAURI_INTERNALS__.invoke('response_body_file_info',{path:${JSON.stringify(path)},reference:null});a.bodyFileTouched=true;a.draftDirty=true;a.setResponseFile(info);a.$flushUpdates();const select=a.autoResponseForm.elements.namedItem('bodyFileMode');select.value=${JSON.stringify(mode)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
 const save=async()=>{assert.equal(await main.evaluate(`${editor}.saveAutoResponse(new Event('submit',{cancelable:true}))`),true,'File-source rule did not save');await waitFor(()=>main.evaluate(`!${editor}.loadingSavedResponse&&!${editor}.fileChoiceBusy`),'Saved file source did not load');};
 const open=async()=>{
  const prior=(await targets()).map(row=>row.id);
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${editor}.previewSavedRule();})()`);
  await waitFor(()=>main.evaluate(`${traffic}.previewPageDialog.open&&!${traffic}.previewRulesBusy`),'File preview choices did not open');
  await main.evaluate(`(async()=>{const t=${traffic};t.previewDocumentChanged({currentTarget:{value:${JSON.stringify(documentId)}}});await t.openCapturedPage();})()`);
  const target=await waitFor(async()=>(await targets()).find(row=>!prior.includes(row.id)&&row.url==='https://example.invalid/captured-page'),'File preview did not navigate');
  return {target,page:await connect(target)};
 };
 const close=async preview=>{await closeNative('Modified preview · Scripts off — Transmog');await waitFor(async()=>!(await targets()).some(row=>row.id===preview.target.id),'File preview did not close');};
 try{
  await writeFile(path,red);
  await main.evaluate(`(async()=>{const t=${traffic};await ${shell}.activateView('traffic');await t.revealSession(t.sessions.find(row=>row.path==='/page.css').id);t.beginExperimentFromSelected();})()`);
  await waitFor(()=>main.evaluate(`${editor}.responseExperiment&&!${editor}.editBodyDisabled&&!${editor}.loadingSavedResponse`),'File experiment did not open');
  await choose('linked');await save();
  const ruleId=await main.evaluate(`${editor}.editingAutoResponseId`);
  const first=await open();await waitFor(async()=>await color(first.page)==='rgb(255, 0, 0)','Linked small CSS did not render');
  await writeFile(path,blue);await first.page.call('Page.reload',{ignoreCache:true});
  await waitFor(async()=>await color(first.page)==='rgb(255, 0, 0)','File edit changed an open frozen preview');
  const next=await open();await waitFor(async()=>await color(next.page)==='rgb(0, 0, 255)','Updated preview did not reread small linked CSS');
  await closeNative('Modified preview · Scripts off — Transmog');await closeNative('Modified preview · Scripts off — Transmog');
  await waitFor(async()=>!(await targets()).some(row=>[next.target.id,first.target.id].includes(row.id)),'Linked-file previews did not close');
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');const a=${editor};a.changeResponseBodyFileMode({currentTarget:{value:'copy'}});})()`);await save();
  const assets=await invoke('response_assets'),current=(await invoke('automation_status')).rules.find(row=>row.id===ruleId);
  const copy=assets.find(asset=>asset.id+'@'+asset.revision===current.request.responseAsset);
  assert.equal(copy.linkedBody??null,null);assert.equal(copy.copiedFrom,path);
  await writeFile(path,red);const copied=await open();await waitFor(async()=>await color(copied.page)==='rgb(0, 0, 255)','External edit changed copied response bytes');
  await close(copied);
  await main.evaluate(`${shell}.activateView('automation')`);await choose('linked');await save();
  await unlink(path);await main.evaluate(`${editor}.refreshResponseFile()`);
  assert.match(await main.evaluate(`${editor}.responseBodyFileError`),/missing|unreadable/i);
  assert.equal(await main.evaluate(`${editor}.responseBodyFile`),path);
  await main.evaluate(`${editor}.autoResponseForm.querySelector('.response-file-overlay').scrollIntoView({block:'start'})`);
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-linked-file-repair.png'),Buffer.from(image.data,'base64'));}
  await writeFile(path,blue);await main.evaluate(`${editor}.refreshResponseFile()`);
  assert.equal(await main.evaluate(`${editor}.responseBodyFileError`),'');
  assert.equal((await invoke('automation_status')).rules.find(row=>row.id===ruleId).enabled,false);
  process.stdout.write('Native file modes: small linked CSS, external edits/new preview, frozen reload, explicit copied revision, copy independence and missing-file repair verified.\n');
 }finally{
  const current=await invoke('automation_status'),candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:before.rules,autoresponsesEnabled:before.autoresponsesEnabled}});
  await invoke('activate_automation',{candidateId:candidate.candidateId});
  await main.evaluate(`${shell}.activateView('traffic')`);
 }
}
