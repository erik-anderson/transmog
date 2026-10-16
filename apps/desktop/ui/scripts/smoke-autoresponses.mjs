import {mkdir,open,writeFile,unlink} from 'node:fs/promises';
import {resolve} from 'node:path';
import {gzipSync} from 'node:zlib';

export async function smokeAutoResponseFiles(call,evaluate,screenshotPath) {
  const directory=resolve('../../../target/ui-check/autoresponse-native-files');
  await mkdir(directory,{recursive:true});
  const path=resolve(directory,'linked-response.bin');
  const file=await open(path,'w');await file.truncate(25000001);await file.close();
  const rawHeaders=['alpha \t',[255],'omega'].map(value=>({name:Array.from(Buffer.from('X-Repeat')),value:Array.isArray(value)?value:Array.from(Buffer.from(value))}));
  const mediaType='text/plain; Charset = "UTF-16LE"';
  try {
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),invoke=window.__TAURI_INTERNALS__.invoke;
      const original=await invoke('automation_status');
      globalThis.__responseFileProbe={original,view:shell.activeView};
      const sourceMediaType=${JSON.stringify(mediaType)};
      const source=await invoke('create_response_asset',{input:{id:'native-compressed-source',revision:1,status:200,headers:${JSON.stringify([...rawHeaders,{name:Array.from(Buffer.from('content-encoding')),value:Array.from(Buffer.from('x-gzip'))}])},body:${JSON.stringify(Array.from(gzipSync(Buffer.from([0,255,1,254]))))},mediaType:sourceMediaType}});
      const asset=await invoke('edit_response_asset',{input:{assetReference:source.id+'@1',status:200,headers:source.headers,bodyPath:${JSON.stringify(path)},bodyFileMode:'linked',decodedBody:null,mediaType:source.mediaType,preserveContentEncoding:true}});
      if(!asset.linkedBody||asset.linkedBody.path!==${JSON.stringify(path)})throw new Error('Native file import did not preserve its linked path');
      const matcher={method:'GET',url:{kind:'exact',value:'https://example.test/native-file'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null};
      const rule={id:'native-linked-rule',displayName:'Native linked file',enabled:true,revision:1,priority:-2000000,matcher,request:{headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:{headers:[],replaceBody:null,discardBody:false,abortReason:null}};
      const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:original.generation,autoresponsesEnabled:original.autoresponsesEnabled,rules:[rule,...original.rules]}});
      await invoke('activate_automation',{candidateId:candidate.candidateId});
      shell.shadowRoot.querySelector('a[data-view="automation"]').click();
      await new Promise(resolve=>setTimeout(resolve,100));
      const workspace=shell.shadowRoot.querySelector('automation-workspace');
      await workspace.refreshAutomation();await workspace.editAutoResponse(rule,true);workspace.$flushUpdates();
      const waitForFile=async()=>{const deadline=Date.now()+15000;while((workspace.loadingSavedResponse||workspace.fileChoiceBusy)&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,20));if(workspace.loadingSavedResponse||workspace.fileChoiceBusy)throw new Error('Native file inspection did not finish');};
      await waitForFile();
      if(!workspace.autoResponseBody.disabled||!workspace.textContent.includes('Response body from file'))throw new Error('Native linked-file overlay is missing');
      if(workspace.autoResponsePreserveEncoding.disabled||!workspace.textContent.includes('Apply original compression type (gzip)')||!asset.linkedBody.applyOriginalCompression)throw new Error('Native linked-file compression choice is missing');
      if(/x-gzip/i.test(workspace.autoResponsePreserveEncoding.closest('label').textContent))throw new Error('Native compression control exposed the gzip alias');
      workspace.autoResponseForm.elements.namedItem('status').value='201';
      if(!await workspace.saveAutoResponse(new Event('submit',{cancelable:true})))throw new Error('Native metadata-only Save failed: '+workspace.autoResponseError);
      await waitForFile();
      const saved=await invoke('inspect_response_asset',{reference:workspace.rules.find(r=>r.id===rule.id).request.responseAsset});
      const repeated=saved.asset.headers.filter(field=>new TextDecoder().decode(new Uint8Array(field.name))==='X-Repeat');
      if(JSON.stringify(repeated)!==${JSON.stringify(JSON.stringify(rawHeaders))})throw new Error('Native metadata-only Save changed raw repeated header bytes or order');
      workspace.autoResponseForm.closest('.rule-properties-pane').scrollTop=100000;
    })()`);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-linked-file.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await unlink(path);
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');await w.refreshResponseFile();w.$flushUpdates();if(!w.responseBodyFileError.includes('missing or unreadable')||!w.responseBodyFile)throw new Error('Native missing-file state lost configuration or recovery');})()`);
    await writeFile(path,'external replacement');
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');await w.refreshResponseFile();if(w.responseBodyFileError)throw new Error('Native restored-file state did not recover');w.clearResponseBodyFile();w.$flushUpdates();if(w.autoResponseBody.disabled||w.autoResponseBody.readOnly)throw new Error('Native clear-file action did not restore editing');})()`);
    await evaluate(`(async()=>{
      const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),body='{"native":[1,2],"ready":true}';
      w.autoResponseBody.value=body;w.openBodyDialog();
      const deadline=Date.now()+15000;while(!w.bodyEditor.bodyEditorReady&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));
      if(!w.bodyEditor.editor||w.bodyEditor.bodyLanguage!=='json'||!w.bodyEditor.bodyCanSave)throw new Error('Native syntax dialog did not hydrate with body-only saving');
      await w.bodyEditor.format();if(!w.bodyEditor.model.getValue().includes('\\n'))throw new Error('Native JSON formatting did not run');
      w.bodyEditor.editor.focus();
    })()`);
    await call('Input.dispatchKeyEvent',{type:'keyDown',modifiers:2,key:'z',code:'KeyZ',windowsVirtualKeyCode:90});
    await call('Input.dispatchKeyEvent',{type:'keyUp',modifiers:2,key:'z',code:'KeyZ',windowsVirtualKeyCode:90});
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');await new Promise(resolve=>setTimeout(resolve,50));if(w.bodyEditor.model.getValue()!=='{"native":[1,2],"ready":true}')throw new Error('Native formatting undo failed');await w.bodyEditor.format();})()`);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-body-dialog.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await evaluate(`(async()=>{
      const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');w.bodyEditor.commit(false);
      await new Promise(resolve=>setTimeout(resolve,50));if(!w.autoResponseBody.value.includes('\\n')||w.bodyEditor.model)throw new Error('Native dialog Apply did not restore and release the draft');
      const applied=w.autoResponseBody.value,appliedBytes=w.currentBodyBytes();w.openBodyDialog();const deadline=Date.now()+15000;
      while(!w.bodyEditor.bodyEditorReady&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));
      w.bodyEditor.editor.setValue('cancelled');w.bodyEditor.cancel();await new Promise(resolve=>setTimeout(resolve,50));
      if(w.autoResponseBody.value!==applied)throw new Error('Native Cancel changed the draft');
      w.openBodyDialog();while(!w.bodyEditor.bodyEditorReady&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));
      w.bodyEditor.commit(true);const savedDeadline=Date.now()+15000;
      while((w.bodyEditor.surface.open||w.draftDirty||w.bodyEditor.model)&&Date.now()<savedDeadline)await new Promise(resolve=>setTimeout(resolve,25));
      if(w.bodyEditor.surface.open||w.draftDirty||w.bodyEditor.model)throw new Error('Native dialog Save rule failed: '+JSON.stringify({open:w.bodyEditor.surface.open,dirty:w.draftDirty,hasModel:Boolean(w.bodyEditor.model),canSave:w.bodyEditor.bodyCanSave,busy:w.bodyEditor.bodyBusy,error:w.bodyEditor.bodyError,ruleError:w.autoResponseError}));
      const saved=await window.__TAURI_INTERNALS__.invoke('inspect_response_asset',{reference:w.rules.find(rule=>rule.id==='native-linked-rule').request.responseAsset});
      if(JSON.stringify(Array.from(atob(saved.bytesBase64),c=>c.charCodeAt(0)))!==JSON.stringify(appliedBytes)||saved.textEncoding!=='utf-16le'||saved.asset.mediaType!==${JSON.stringify(mediaType)})throw new Error('Native clear-file text body disagrees with its original bytes or declared charset');
    })()`);
    return {linkedPath:true,disabledEditor:true,compressionChoice:true,gzipAlias:true,rawHeaderOrder:true,linkedFileCharset:true,missingFile:true,recovery:true,clearFile:true,syntaxEditor:true,formatUndo:true,applyCancel:true,dialogSave:true};
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

export async function smokeAutoResponseDragging(call,evaluate,screenshotPath) {
  const start=async id=>{
    const point=await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),grip=w.querySelector('[data-rule-id="${id}"] .rule-drag-grip');grip.scrollIntoView({block:'nearest',behavior:'instant'});await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));const r=grip.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',...point});await call('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',buttons:1,clickCount:1});
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:point.x+12,y:point.y,buttons:1});
    await evaluate(`(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');if(!w.ruleDragging)throw new Error('Native pointer drag did not begin: '+JSON.stringify({pointer:w.rulePointer?{id:w.rulePointer.id,x:w.rulePointer.x,y:w.rulePointer.y}:null,view:w.view,focus:document.hasFocus(),rect:w.autoResponseList.getBoundingClientRect().toJSON()}));})()`);
  };
  const edge=async bottom=>{
    const point=await evaluate(`(()=>{const r=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').autoResponseList.getBoundingClientRect();return {x:r.x+r.width/2,y:${bottom?'r.bottom-4':'r.top+4'}};})()`);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',...point,buttons:1});
    await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),list=w.autoResponseList,deadline=Date.now()+10000;while(${bottom?'list.scrollTop<list.scrollHeight-list.clientHeight-2':'list.scrollTop>0'}&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));if(${bottom?'list.scrollTop<list.scrollHeight-list.clientHeight-2':'list.scrollTop>0'})throw new Error('Native stationary edge drag did not scroll');})()`);
    return point;
  };
  const release=async point=>{await call('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',buttons:0,clickCount:1});await evaluate(`(async()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),deadline=Date.now()+10000;while((w.changingAutoResponse||w.loadingSavedResponse)&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));})()`);};
  try {
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),invoke=window.__TAURI_INTERNALS__.invoke,original=await invoke('automation_status');globalThis.__responseDragProbe={original,view:shell.activeView};
      const asset=await invoke('create_response_asset',{input:{id:'native-drag-source',revision:1,status:200,headers:[],body:[78,97,116,105,118,101],mediaType:'text/plain'}});
      const matcher={method:'GET',url:{kind:'exact',value:''},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null};
      const request={headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:asset.id+'@1',allowNonIdempotentBodyReplacement:false},response={headers:[],replaceBody:null,discardBody:false,abortReason:null};
      const rules=Array.from({length:55},(_,index)=>({id:'native-drag-'+index,displayName:'Native drag rule '+index,enabled:true,revision:1,priority:-3000000+index,matcher:{...matcher,url:{kind:'exact',value:'https://example.test/native-drag/'+index}},request,response}));
      const candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:original.generation,autoresponsesEnabled:original.autoresponsesEnabled,rules:[...rules,...original.rules]}});await invoke('activate_automation',{candidateId:candidate.candidateId});
      shell.shadowRoot.querySelector('a[data-view="automation"]').click();await new Promise(resolve=>setTimeout(resolve,100));const w=shell.shadowRoot.querySelector('automation-workspace');w.draftDirty=false;await w.cancelAutoResponseEdit();await w.refreshAutomation();await w.editAutoResponse(rules[0],true);w.$flushUpdates();
    })()`);
    await start('native-drag-0');const bottom=await edge(true);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-drag-bottom.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await release(bottom);
    await evaluate(`(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');if(w.rules.filter(r=>r.id.startsWith('native-drag-')).at(-1).id!=='native-drag-0')throw new Error('Native bottom drop did not reorder');})()`);
    await start('native-drag-0');const top=await edge(false);await release(top);
    await evaluate(`(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');if(w.rules[0].id!=='native-drag-0')throw new Error('Native top drop did not reorder');})()`);
    await start('native-drag-2');
    await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await release(top);
    await evaluate(`(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');if(w.ruleDragging||w.dragScrollTimer!==undefined||w.ruleSelection.focused!=='native-drag-0'||!w.ruleSelection.ids.has('native-drag-0'))throw new Error('Native Escape did not restore the selection and release scrolling');})()`);
    return {pointerDrag:true,stationaryBottomScroll:true,stationaryTopScroll:true,firstLastDrop:true,escapeRestoresSelection:true};
  }finally {
    await evaluate(`(async()=>{const probe=globalThis.__responseDragProbe;if(!probe)return;const shell=document.querySelector('app-shell'),w=shell.shadowRoot.querySelector('automation-workspace'),invoke=window.__TAURI_INTERNALS__.invoke;w.finishRuleDrag();w.draftDirty=false;await w.cancelAutoResponseEdit();const current=await invoke('automation_status'),candidate=await invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,autoresponsesEnabled:probe.original.autoresponsesEnabled,rules:probe.original.rules}});await invoke('activate_automation',{candidateId:candidate.candidateId});await w.refreshAutomation();shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__responseDragProbe;})()`);
  }
}
