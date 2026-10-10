import assert from 'node:assert/strict';
import {createServer,request} from 'node:http';
import {writeFile} from 'node:fs/promises';

export async function smokeUserAgent(call,evaluate,screenshotPath) {
  const received=[];
  const origin=createServer((incoming,response)=>{received.push({...incoming.headers});response.setHeader('Content-Type','application/json');response.end(JSON.stringify(incoming.headers));});
  await new Promise(resolve=>origin.listen(0,'127.0.0.1',resolve));
  const uri='http://127.0.0.1:'+origin.address().port+'/ua-native';
  const component="document.querySelector('app-shell').automation.userAgentOverrides";
  const wait=async(expression)=>{const deadline=Date.now()+10000;while(!await evaluate(expression)){if(Date.now()>deadline)throw new Error('Native UA probe timed out: '+expression);await new Promise(resolve=>setTimeout(resolve,50));}};
  try {
    await evaluate(`(async()=>{const shell=document.querySelector('app-shell');globalThis.__uaNativeProbe={product:await window.__TAURI_INTERNALS__.invoke('product_state'),automation:await window.__TAURI_INTERNALS__.invoke('automation_status'),view:shell.activeView};shell.shadowRoot.querySelector('a[data-view="automation"]').click();})()`);
    await wait(`Boolean(document.querySelector('app-shell').automation?.userAgentOverrides)`);
    await evaluate(`(()=>{const workspace=document.querySelector('app-shell').automation;workspace.showAutomationSection('headers');const editor=${component};editor.add();editor.$flushUpdates();const fill=(name,value)=>{const field=editor.querySelector('[name="'+name+'"]');field.value=value;field.dispatchEvent(new Event('input',{bubbles:true}));};fill('uaName','Native Edge override');fill('uaHost','127.0.0.1');fill('uaPath','/ua-native');const select=editor.querySelector('[aria-label="User-Agent preset"]');select.value=Array.from(select.options).find(option=>/Edge.*Windows/.test(option.textContent)).value;select.dispatchEvent(new Event('change',{bubbles:true}));globalThis.__uaNativeSave=editor.save(new Event('submit',{cancelable:true}));})()`);
    await evaluate(`globalThis.__uaNativeSave`);await wait(`!${component}.uaBusy&&${component}.uaRows.some(row=>row.name==='Native Edge override')`);
    assert.ok(await evaluate(`document.querySelector('app-shell').automationEnabledCount>=1`));
    await evaluate(`(()=>{const editor=${component};editor.add();editor.$flushUpdates();for(const[name,value]of[['uaName','Duplicate native'],['uaHost','127.0.0.1'],['uaPath','/ua-native']]){const input=editor.querySelector('[name="'+name+'"]');input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));}globalThis.__uaNativeSave=editor.save(new Event('submit',{cancelable:true}));})()`);
    await wait(`${component}.uaDuplicateDialog.open`);
    const point=await evaluate(`(()=>{const button=Array.from(${component}.uaDuplicateDialog.querySelectorAll('button')).find(button=>button.textContent==='Keep existing override'),box=button.getBoundingClientRect();return{x:box.x+box.width/2,y:box.y+box.height/2};})()`);
    for(const type of ['mousePressed','mouseReleased'])await call('Input.dispatchMouseEvent',{type,...point,button:'left',clickCount:1});
    await evaluate(`globalThis.__uaNativeSave`);
    assert.ok(await evaluate(`${component}.uaSaveButton===${component}.uaSaveButton.getRootNode().activeElement`));
    await evaluate(`(()=>{const editor=${component};editor.uaDirty=false;const rule=editor.uaState.rules.find(rule=>rule.displayName==='Native Edge override');editor.load(rule);})()`);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-user-agent.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await call('Emulation.setDeviceMetricsOverride',{width:760,height:520,deviceScaleFactor:1,mobile:false});
    assert.ok(await evaluate(`(()=>{const editor=${component},button=editor.uaSaveButton;button.scrollIntoView({block:'nearest'});const box=button.getBoundingClientRect();return box.bottom<=innerHeight&&button.getRootNode().elementFromPoint(box.x+box.width/2,box.y+box.height/2)===button;})()`));
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-user-agent-compact.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await call('Emulation.clearDeviceMetricsOverride');
    const listener=await evaluate(`(async()=>{const probe=globalThis.__uaNativeProbe,manual=structuredClone(probe.product);manual.preferences.configureSystemProxy=false;manual.preferences.decryptHttps=false;manual.preferences.allowedOutboundProtocols.http1=true;await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:manual});const bootstrap=await window.__TAURI_INTERNALS__.invoke('desktop_bootstrap');const status=await window.__TAURI_INTERNALS__.invoke('start_proxy',{request:{listen:'127.0.0.1:0',caCertificatePath:bootstrap.caCertificatePath,caPrivateKeyPath:bootstrap.caPrivateKeyPath}});return status.listener;})()`);
    const proxy=new URL('http://'+listener);
    const send=headers=>new Promise((resolve,reject)=>{const outgoing=request({hostname:proxy.hostname,port:proxy.port,path:uri,method:'GET',headers:{Host:new URL(uri).host,...headers},agent:false},response=>{const chunks=[];response.on('data',chunk=>chunks.push(chunk));response.on('end',()=>response.statusCode===200?resolve(JSON.parse(Buffer.concat(chunks).toString())):reject(new Error('Native UA origin status '+response.statusCode)));});outgoing.setTimeout(10000,()=>outgoing.destroy(new Error('Native UA echo timeout')));outgoing.on('error',reject);outgoing.end();});
    const chrome='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36';
    const withHeaders=await send({'User-Agent':chrome,'Sec-CH-UA':'"Original";v="154"','Sec-CH-UA-Model':'"original model"'});
    assert.match(withHeaders['user-agent'],/Chrome\/154\.0\.0\.0.*Edg\/154\.0\.0\.0/);assert.match(withHeaders['sec-ch-ua'],/Microsoft Edge.*154/);assert.equal(withHeaders['sec-ch-ua-model'],'""');
    assert.equal('sec-ch-ua-platform' in withHeaders,false);assert.equal('sec-ch-ua-full-version' in withHeaders,false);
    const absent=await send({Accept:'*/*'});assert.equal('user-agent' in absent,false);assert.equal('sec-ch-ua' in absent,false);
    assert.equal(received.length,2);
    return {nativePreset:true,duplicateConfirmation:true,compactActions:true,sourceMajor:true,existingHintsOnly:true,absentHeadersPreserved:true};
  } finally {
    try {
    await call('Emulation.clearDeviceMetricsOverride');
    await evaluate(`(async()=>{const probe=globalThis.__uaNativeProbe;if(!probe)return;const shell=document.querySelector('app-shell');if((await window.__TAURI_INTERNALS__.invoke('app_status')).lifecycle!=='stopped')await window.__TAURI_INTERNALS__.invoke('stop_application');const current=await window.__TAURI_INTERNALS__.invoke('automation_status');const candidate=await window.__TAURI_INTERNALS__.invoke('validate_automation',{document:{schemaVersion:1,generation:current.generation,rules:probe.automation.rules,autoresponsesEnabled:probe.automation.autoresponsesEnabled}});const status=await window.__TAURI_INTERNALS__.invoke('activate_automation',{candidateId:candidate.candidateId});shell.onAutomationState(new CustomEvent('automation-state-change',{detail:status}));await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:probe.product});const rows=await window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{}});const ids=rows.sessions.filter(row=>row.path==='/ua-native').map(row=>row.id);if(ids.length)await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids,restore:false});shell.automation.userAgentOverrides.uaDirty=false;shell.shadowRoot.querySelector('a[data-view="'+probe.view+'"]').click();delete globalThis.__uaNativeProbe;delete globalThis.__uaNativeSave;})()`);
    } finally {await new Promise(resolve=>origin.close(resolve));}
  }
}
