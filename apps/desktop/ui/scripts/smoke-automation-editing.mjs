import { writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";

/** Real command contracts, persistence and authoring in the owned WebView2 profile. */
export async function smokeAutomationEditing(call, evaluate, screenshotPath, waitForEvent) {
  const bytes = [0, 255, 128, 13, 10, 42],
    encoded = Array.from(gzipSync(Buffer.from(bytes)));
  try {
    await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),invoke=window.__TAURI_INTERNALS__.invoke;
    const original=await invoke('automation_status');globalThis.__automationEditingProbe={original,view:shell.activeView};
    shell.shadowRoot.querySelector('a[data-view="automation"]').click();
    const w=shell.shadowRoot.querySelector('automation-workspace');w.showAutomationSection('responses');w.$flushUpdates();await w.refreshAutomation();
    const asset=await invoke('create_response_asset',{input:{id:'native-hex-editing',revision:1,status:200,headers:[{name:[67,111,110,116,101,110,116,45,69,110,99,111,100,105,110,103],value:[103,122,105,112]}],body:${JSON.stringify(encoded)},mediaType:'application/octet-stream'}});
    const matcher={method:'GET',url:{kind:'exact',value:'https://example.test/hex'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null,examples:[]};
    const rule={id:'native-hex-rule',displayName:'Native binary response',enabled:true,revision:1,priority:-3000000,matcher,request:{headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:asset.id+'@1',allowNonIdempotentBodyReplacement:false},response:{headers:[],replaceBody:null,discardBody:false,abortReason:null}};
    const candidate=await invoke('validate_automation',{document:{schemaVersion:2,generation:original.generation,autoresponsesEnabled:true,responseSets:original.responseSets,rules:[rule,...original.rules]}});await invoke('activate_automation',{candidateId:candidate.candidateId});
    await w.refreshAutomation();await w.editAutoResponse(rule,true);w.$flushUpdates();
    if(w.bodyMode!=='hex'||w.bodyReadOnly||w.autoResponseBody.value!=='00 ff 80 0d 0a 2a')throw new Error('Native binary body did not open editable Hex');
    const before=asset.sha256;w.autoResponseStatus.value='201';
    if(!await w.saveAutoResponse(new Event('submit',{cancelable:true})))throw new Error(w.autoResponseError);
    const unchanged=await invoke('inspect_response_asset',{reference:w.rules.find(item=>item.id===rule.id).request.responseAsset});
    if(unchanged.asset.sha256!==before)throw new Error('Metadata-only Save changed encoded binary bytes');
    w.autoResponseBody.value='00 ff 81 0d 0a 2a';w.draftDirty=true;
    if(!await w.saveAutoResponse(new Event('submit',{cancelable:true})))throw new Error(w.autoResponseError);
    const changed=await invoke('inspect_response_asset',{reference:w.rules.find(item=>item.id===rule.id).request.responseAsset});
    if(JSON.stringify(Array.from(atob(changed.bytesBase64),c=>c.charCodeAt(0)))!==JSON.stringify([0,255,129,13,10,42])||changed.contentCodings.join(',')!=='gzip')throw new Error('Native hex save changed bytes or compression');
    const nameInput=w.autoResponseForm.elements.namedItem('name'),savedName=nameInput.value;
    nameInput.value='';nameInput.dispatchEvent(new Event('input',{bubbles:true}));await Promise.resolve();
    const leave=w.canLeaveEditor();if(!w.unsavedDialog.open)throw new Error('Native invalid draft did not open leave confirmation');
    await w.resolveUnsaved('save');
    if(await leave||w.unsavedDialog.open||nameInput.getRootNode().activeElement!==nameInput)throw new Error('Native invalid-field recovery did not return focus to the editor');
    nameInput.value=savedName;nameInput.dispatchEvent(new Event('input',{bubbles:true}));await Promise.resolve();w.autoResponseForm.reportValidity();
    w.matchTestUrl.value='https://example.test/does-not-match';w.matchTestMethod.value='GET';await w.testMatcher();w.matchExpected='nonmatch';w.addMatchExample();
    while(w.matcherTestBusy)await new Promise(resolve=>setTimeout(resolve,20));w.$flushUpdates();
    if(w.matchCaseRows[0].expected!=='Does not match'||w.matchCaseRows[0].actual!=='Does not match'||w.matchCaseRows[0].result!=='Passed')throw new Error('Native negative expectation is ambiguous');
    w.matchExpected='match';w.addMatchExample();while(w.matcherTestBusy)await new Promise(resolve=>setTimeout(resolve,20));w.$flushUpdates();
    if(w.matchCaseRows[0].result!=='Failed'||w.matchTestResult.test.matched)throw new Error('Native expectation changed actual match');
    w.draftDirty=false;await w.cancelAutoResponseEdit();
    const oldScripts=await invoke('script_status');if(oldScripts.active.length||oldScripts.saved.length)throw new Error('Script smoke requires an empty isolated profile');
    w.showAutomationSection('scripts');w.$flushUpdates();const editor=w.querySelector('script-editor');
    const end=Date.now()+20000;while(!editor.scriptReady&&Date.now()<end)await new Promise(resolve=>setTimeout(resolve,25));
    if(!editor.scriptReady)throw new Error('Native script editor did not hydrate: '+JSON.stringify({status:editor.scriptText,active:editor.active,view:w.view,section:w.automationSection,connected:editor.isConnected,tag:editor.tagName,type:editor.constructor.name}));
    await editor.activateScript();
    let status=await invoke('script_status');if(!status.active.length||editor.scriptDirty||editor.scriptUnapplied)throw new Error('One-click script enable failed: '+editor.scriptText);
    if(status.saved[0].source!==status.active[0].source)throw new Error('Enabled source was not saved as editing baseline');
    const source=editor.sourceEditor.getValue();editor.sourceEditor.setValue(source+'\\n// native edited version');await editor.activateScript();
    status=await invoke('script_status');if(!status.active[0].source.includes('native edited version')||status.saved[0].source!==status.active[0].source||editor.scriptDirty)throw new Error('One-click edited script activation failed');
    editor.scriptTestUrl.value='https://example.test/';await editor.testScript();if(!editor.testResult.includes('Completed for this input'))throw new Error('Native sandbox result missing: '+editor.scriptText);
    editor.scriptTestUrl.value='https://example.test/changed';editor.markTestChanged();if(!editor.testOutdated)throw new Error('Native sandbox result was not invalidated');
    const good=structuredClone(status.active[0]);editor.sourceEditor.setValue('export function onRequestHead( {');await editor.activateScript();
    status=await invoke('script_status');if(status.active[0].source!==good.source||!editor.scriptDirty)throw new Error('Invalid source replaced the active script');
    editor.sourceEditor.setValue(good.source);await editor.disableScript();if(editor.scriptActive)throw new Error('Native Pause failed');
    editor.$flushUpdates();
  })()`);
  } finally {
    await evaluate(
      `(async()=>{const probe=globalThis.__automationEditingProbe;if(!probe)return;const invoke=window.__TAURI_INTERNALS__.invoke,current=await invoke('automation_status');const candidate=await invoke('validate_automation',{document:{schemaVersion:2,generation:current.generation,autoresponsesEnabled:probe.original.autoresponsesEnabled,responseSets:probe.original.responseSets,rules:probe.original.rules}});await invoke('activate_automation',{candidateId:candidate.candidateId});delete globalThis.__automationEditingProbe;})()`,
    );
  }
  if (screenshotPath)
    await writeFile(
      screenshotPath.replace(/\.png$/i, "-script-editing.png"),
      Buffer.from(
        (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
          .data,
        "base64",
      ),
    );
  // Reload the real WebView and check persistence without explicitly saving a draft.
  const loaded = waitForEvent("Page.loadEventFired", 20000);
  await call("Page.reload", { ignoreCache: true });
  await loaded;
  await evaluate(`(async()=>{
    const end=Date.now()+20000;while(!document.querySelector('app-shell')?.shadowRoot?.querySelector('a[data-view="automation"]')&&Date.now()<end)await new Promise(resolve=>setTimeout(resolve,25));
    const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="automation"]').click();
    while(typeof shell.shadowRoot.querySelector('automation-workspace')?.showAutomationSection!=='function'&&Date.now()<end)await new Promise(resolve=>setTimeout(resolve,25));
    const w=shell.shadowRoot.querySelector('automation-workspace');w.showAutomationSection('scripts');w.$flushUpdates();
    while(!w.querySelector('script-editor')?.scriptReady&&Date.now()<end)await new Promise(resolve=>setTimeout(resolve,25));
    const editor=w.querySelector('script-editor');
    if(!editor?.sourceEditor?.getValue().includes('native edited version')||editor.scriptDirty)throw new Error('Reopened native script lost activated source');
    shell.shadowRoot.querySelector('a[data-view="traffic"]').click();
  })()`);
  return {
    binaryFallback: true,
    bytePreservation: true,
    compressedHexEdit: true,
    unsavedValidationFocus: true,
    explicitExpectations: true,
    oneClickActivation: true,
    atomicFailure: true,
    sandboxEvidence: true,
    activatedSourceReload: true,
  };
}
