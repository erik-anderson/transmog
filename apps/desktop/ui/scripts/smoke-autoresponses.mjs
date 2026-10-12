import {mkdir,open,writeFile,unlink} from 'node:fs/promises';
import {resolve} from 'node:path';

export async function smokeAutoResponseFiles(call,evaluate,screenshotPath) {
  const directory=resolve('../../../target/ui-check/autoresponse-native-files');
  await mkdir(directory,{recursive:true});
  const path=resolve(directory,'linked-response.bin');
  const file=await open(path,'w');await file.truncate(25000001);await file.close();
  try {
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),invoke=window.__TAURI_INTERNALS__.invoke;
      const original=await invoke('automation_status');
      globalThis.__responseFileProbe={original,view:shell.activeView};
      const asset=await invoke('import_response_asset',{input:{id:'native-linked-file',revision:1,status:200,headers:[],bodyPath:${JSON.stringify(path)},mediaType:'application/octet-stream'}});
      if(!asset.linkedBody||asset.linkedBody.path!==${JSON.stringify(path)})throw new Error('Native file import did not preserve its linked path');
      const matcher={method:'GET',url:{kind:'exact',value:'https://example.test/native-file'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null};
      const rule={id:'native-linked-rule',displayName:'Native linked file',enabled:true,revision:1,priority:-2000000,matcher,request:{headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:{headers:[],replaceBody:null,discardBody:false,abortReason:null}};
      const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:original.generation,autoresponsesEnabled:original.autoresponsesEnabled,rules:[rule,...original.rules]}});
      await invoke('activate_automation',{candidateId:candidate.candidateId});
      shell.shadowRoot.querySelector('a[data-view="automation"]').click();
      await new Promise(resolve=>setTimeout(resolve,100));
      const workspace=shell.shadowRoot.querySelector('automation-workspace');
      await workspace.refreshAutomation();await workspace.editAutoResponse(rule,true);workspace.$flushUpdates();
      if(!workspace.autoResponseBody.disabled||!workspace.textContent.includes('Response body being served from file'))throw new Error('Native linked-file overlay is missing');
      workspace.autoResponseForm.closest('.rule-properties-pane').scrollTop=100000;
    })()`);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-linked-file.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await unlink(path);
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');await w.refreshResponseFile();w.$flushUpdates();if(!w.responseBodyFileError.includes('missing or unreadable')||!w.responseBodyFile)throw new Error('Native missing-file state lost configuration or recovery');})()`);
    await writeFile(path,'external replacement');
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');await w.refreshResponseFile();if(w.responseBodyFileError)throw new Error('Native restored-file state did not recover');w.clearResponseBodyFile();w.$flushUpdates();if(w.autoResponseBody.disabled||w.autoResponseBody.readOnly)throw new Error('Native clear-file action did not restore editing');})()`);
    return {linkedPath:true,disabledEditor:true,missingFile:true,recovery:true,clearFile:true};
  } finally {
    await evaluate(`(async()=>{
      const probe=globalThis.__responseFileProbe;if(!probe)return;
      const shell=document.querySelector('app-shell'),w=shell.shadowRoot.querySelector('automation-workspace'),invoke=window.__TAURI_INTERNALS__.invoke;
      w.draftDirty=false;await w.cancelAutoResponseEdit();
      const current=await invoke('automation_status');const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,autoresponsesEnabled:probe.original.autoresponsesEnabled,rules:probe.original.rules}});
      await invoke('activate_automation',{candidateId:candidate.candidateId});await w.refreshAutomation();
      shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__responseFileProbe;
    })()`);
    await unlink(path).catch(()=>undefined);
  }
}
