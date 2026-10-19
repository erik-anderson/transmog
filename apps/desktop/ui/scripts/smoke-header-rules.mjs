import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

export async function smokeHeaderRules({main,viewer,waitFor,screenshot}) {
 const shell="document.querySelector('app-shell')",editor=shell+".automation.querySelector('header-rules')";
 const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
 const before=await invoke('automation_status');
 try {
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${shell}.automation.showAutomationSection('header-rules');await ${editor}.newRule();})()`);
  await main.evaluate(`(()=>{const h=${editor};h.hrName='Native ordered policy';h.hrMatcher.matchMode='exact';h.hrMatcher.addressInput.value='https://example.test/policy';h.hrMatcher.changed();h.hrOperations=[{id:'remove',operation:'remove',name:'Content-Security-Policy',encoding:'text',value:''},{id:'set',operation:'set',name:'Cache-Control',encoding:'text',value:'no-cache'},{id:'append',operation:'append',name:'X-Bytes',encoding:'hex',value:'80 ff'}];h.hrStatusFilter='200';h.hrPredicates=[{id:'type',phase:'response',name:'Content-Type',kind:'prefix',encoding:'text',value:'text/html'}];h.hrSampleUrl='https://example.test/policy';h.hrSampleResponse="Content-Type: text/html\\nContent-Security-Policy: default-src 'none'\\nCache-Control: max-age=60\\ncache-control: private\\nSet-Cookie: one=1\\nSet-Cookie: two=2";h.hrDirty=true;h.testDraft();})()`);
  await waitFor(()=>main.evaluate(`!${editor}.hrTestBusy`),'Native header draft test stayed busy');
  const tested=await main.evaluate(`({summary:${editor}.hrTestSummary,before:${editor}.hrBefore,after:${editor}.hrAfter,operations:${editor}.hrResults})`);
  assert.match(tested.summary,/Draft matches/);assert.equal(tested.operations.length,3);
  assert.equal(tested.after.filter(h=>h.name.toLowerCase()==='cache-control').length,1);
  assert.equal(tested.after.filter(h=>h.name.toLowerCase()==='set-cookie').length,2);
  assert.ok(tested.after.some(h=>h.name==='X-Bytes'&&h.value==='Hex: 80 ff'));
  assert.deepEqual((await invoke('automation_status')).rules,before.rules,'Draft test changed active rules');
  assert.equal(await main.evaluate(`${editor}.save()`),true);
  let status=await invoke('automation_status');const saved=status.rules.find(r=>r.displayName==='Native ordered policy');assert.equal(saved.enabled,false);
  assert.equal(await main.evaluate(`${editor}.save(undefined,true)`),true);
  status=await invoke('automation_status');assert.equal(status.rules.find(r=>r.id===saved.id).enabled,true);assert.equal(status.autoresponsesEnabled,before.autoresponsesEnabled);
  const bad=structuredClone(saved);bad.response.headers=[{operation:'remove',field:'Content-Length'}];
  const invalidInput={rule:bad,method:'GET',url:'https://example.test/policy',requestHeaders:[],responseStatus:200,responseHeaders:[]};
  assert.match(await main.evaluate(`window.__TAURI_INTERNALS__.invoke('test_header_rule',{input:${JSON.stringify(invalidInput)}}).then(()=>'',error=>String(error.message??error))`),/protected header/);
  for(const command of ['change_header_rule','test_header_rule'])assert.equal(await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},{}).then(()=>false,()=>true)`),true,'Viewer invoked '+command);
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-header-rules.png'),Buffer.from(image.data,'base64'));}
  await main.evaluate(`${editor}.remove()`);assert.equal((await invoke('automation_status')).rules.some(r=>r.id===saved.id),false);
  await main.evaluate(`${editor}.undoRemove()`);assert.equal((await invoke('automation_status')).rules.some(r=>r.id===saved.id),true);
  process.stdout.write('Native header rules: production editor, byte-preserving ordered draft test, duplicate fields, disabled save, explicit enable, protected-field rejection, viewer denial and remove/Undo verified.\n');
 } finally {
  const current=await invoke('automation_status');const candidate=await invoke('validate_automation',{document:{schemaVersion:2,generation:current.generation,rules:before.rules,responseSets:before.responseSets,autoresponsesEnabled:before.autoresponsesEnabled}});const status=await invoke('activate_automation',{candidateId:candidate.candidateId});
  await main.evaluate(`${shell}.automation.renderAutomation(${JSON.stringify(status)})`);
 }
}
