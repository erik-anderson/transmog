// Real WebView2 -> Tauri -> live TLS/QUIC -> non-exportable Windows CNG acceptance.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import net from 'node:net';
import tls from 'node:tls';
import {resolve} from 'node:path';
const argument=name=>process.argv[process.argv.indexOf(name)+1];
const port=argument('--port');
const mode=argument('--mode');
const output=argument('--output');
const targets=await fetch(`http://127.0.0.1:${port}/json/list`).then(response=>response.json());
let target=targets.find(target=>target.type==='page'&&target.title==='Transmog');
assert(target,'The harness-owned Transmog window is unavailable');
let socket;
let sequence=0;const pending=new Map();
async function connectMain(){socket=new WebSocket(target.webSocketDebuggerUrl);await new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});});socket.addEventListener('message',event=>{const result=JSON.parse(String(event.data));if(result.id){const waiter=pending.get(result.id);pending.delete(result.id);if(result.error)waiter.reject(new Error(result.error.message));else waiter.resolve(result.result);}});}
await connectMain();
const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++sequence;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params}));});
const evaluate=async expression=>{const result=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(result.exceptionDetails)throw new Error(result.exceptionDetails.exception?.description??result.exceptionDetails.text);return result.result.value;};
const invoke=(command,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
async function evaluateTarget(url,expression){const socket=new WebSocket(url);try {await new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});});return await new Promise((resolve,reject)=>{socket.addEventListener('message',event=>{const response=JSON.parse(String(event.data));if(response.id!==1)return;if(response.error||response.result.exceptionDetails)reject(new Error(JSON.stringify(response.error??response.result.exceptionDetails)));else resolve(response.result.result.value);});socket.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{expression,awaitPromise:true,returnByValue:true}}));});}finally{socket.close();}}
async function until(predicate,label,milliseconds=8000){const end=Date.now()+milliseconds;for(;;){const result=await predicate();if(result)return result;if(Date.now()>=end)throw new Error(`Timed out: ${label}`);await new Promise(resolve=>setTimeout(resolve,40));}}
const shell="document.querySelector('app-shell')";
const queue=`${shell}.shadowRoot.querySelector('client-certificate-requests')`;
async function protocol(route){const state=await invoke('product_state');const catalog=state.preferences.networkProfiles;const profile=structuredClone(catalog.activeProfile);profile.id='custom-native-'+route;profile.name='Native '+route;profile.settings.protocols={http1:route==='http1',http2:route==='http2',http3:route==='http3'};await invoke('save_network_profiles',{expectedRevision:catalog.revision,profiles:[...catalog.customProfiles.filter(item=>item.id!==profile.id),profile],useProfile:profile.id});}
async function beginComposer(peer){await evaluate(`(async()=>{const shell=${shell};await shell.activateView('composer');const composer=shell.composer;composer.showComposerPane('request');const set=(name,value)=>{const input=composer.composerForm.elements.namedItem(name);input.value=value;input.dispatchEvent(new Event('input',{bubbles:true}));};set('method','GET');set('url',${JSON.stringify(`https://localhost:${peer}/`)});set('headers','');set('body','');composer.composerForm.requestSubmit();})()`);}
async function composerResult(authenticated){await until(()=>evaluate(`!${shell}.composer.composerBusy`),'Composer completion');const result=await evaluate(`({error:${shell}.composer.composerError,result:${shell}.composer.composerResult})`);assert.equal(result.error,'');assert.equal(result.result.status,200);assert.match(result.result.body,new RegExp(`:${authenticated}$`));assert(result.result.detail.performance.transports.some(transport=>transport.tls&&transport.tls.clientChain.length===(authenticated?1:0)),'Received certificate evidence does not match the live server');const certificates=await invoke('inspect_certificates',{id:result.result.detail.id});assert(certificates.connections.some(connection=>connection.clientChain.length===(authenticated?1:0)));return result.result.body;}
async function inspectRequest(id){
  await until(()=>evaluate(`Array.from(${queue}.querySelectorAll('.certificate-pending-row')).some(row=>row.dataset.requestId===${JSON.stringify(id)})`),'rendered blocked connection');
  assert.equal(await evaluate(`(()=>{const status=${queue}.querySelector('span[role="status"]');const box=status.getBoundingClientRect();return box.width<=1&&box.height<=1&&getComputedStyle(status).position==='absolute';})()`),true,'The accessibility announcement occupied visible header space');
  await evaluate(`(()=>{const panel=${queue};if(!panel.certificatePanelOpen)panel.certificateRequestButton.click();const row=Array.from(panel.querySelectorAll('.certificate-pending-row')).find(row=>row.dataset.requestId===${JSON.stringify(id)});row.click();})()`);
  await until(()=>evaluate(`${queue}.certificateRequest?.pending.id===${JSON.stringify(id)}&&!${queue}.certificateDetailBusy&&${queue}.querySelector('select')?.options.length>1`),'rendered eligible certificate choices');
  assert.equal(await evaluate(`Array.from(${queue}.querySelectorAll('.certificate-pending-row')).find(row=>row.dataset.requestId===${JSON.stringify(id)}).getAttribute('aria-pressed')`),'true');
}
async function answer(action,identity){const status=await until(async()=>{const status=await invoke('client_certificate_status');return status.pending.length?status:null;},'live certificate request');const id=status.pending.at(-1).id;
  await inspectRequest(id);
  if(identity)await evaluate(`(()=>{const panel=${queue};const select=panel.querySelector('select');select.value=${JSON.stringify(identity)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  await evaluate(`(()=>{const panel=${queue};const label=${JSON.stringify(action==='certificate'?'Use selected certificate':action==='cancel'?'Cancel connection':'Continue without a certificate')};const button=Array.from(panel.querySelectorAll('button')).find(button=>button.textContent.trim()===label);if(!button||button.disabled)throw new Error('Certificate decision control is unavailable');button.click();})()`);
  await until(async()=>!(await invoke('client_certificate_status')).pending.some(pending=>pending.id===id),'decision removed');
  return id;
}
async function screenshot(name){await writeFile(resolve(output,name),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));}
async function proxyRequest(listener,peer,ca){const [host,port]=listener.split(':');const socket=net.connect({host,port:Number(port)});await new Promise((resolve,reject)=>{socket.once('connect',resolve);socket.once('error',reject);});socket.write(`CONNECT localhost:${peer} HTTP/1.1\r\nHost: localhost:${peer}\r\n\r\n`);await new Promise((resolve,reject)=>{let head='';const onData=data=>{head+=data.toString();if(head.endsWith('\r\n\r\n')){socket.off('data',onData);assert.match(head,/^HTTP\/1\.1 200/);resolve();}};socket.on('data',onData);socket.once('error',reject);});const encrypted=tls.connect({socket,servername:'localhost',ca});await new Promise((resolve,reject)=>{encrypted.once('secureConnect',resolve);encrypted.once('error',reject);});encrypted.write(`GET / HTTP/1.1\r\nHost: localhost:${peer}\r\nConnection: close\r\n\r\n`);let response='';for await(const data of encrypted)response+=data.toString();assert.match(response,/^HTTP\/1\.1 200/);return response.split('\r\n\r\n').at(-1);}
try {
  await call('Runtime.enable');await call('Page.enable');
  await until(()=>evaluate(`Boolean(${shell}?.settings?.ready)`),'hydrated application');
  assert.equal(await evaluate(`(()=>{const root=${shell}.shadowRoot;const brand=root.querySelector('.brand').getBoundingClientRect();const actions=root.querySelector('.top-actions').getBoundingClientRect();return Math.abs(brand.y+brand.height/2-actions.y-actions.height/2)<2;})()`),true,'Empty certificate requests added a native header row');
  const fixture=await invoke('certificate_test_info');
  const state=await invoke('product_state');
  if(mode==='restart'){
    const expected=JSON.parse(await readFile(resolve(output,'saved-policy.json'),'utf8'));
    assert.deepEqual(state.preferences.clientCertificatePolicy,expected,'Certificate policy was not restored at startup');
    await protocol('http1');await beginComposer(fixture.http1);await answer('noCertificate');await composerResult(false);
    process.stdout.write(JSON.stringify({restartPolicyRestored:true,restartedPromptReachedLiveHandshake:true})+'\n');
  } else {
    state.preferences.configureSystemProxy=false;state.preferences.decryptHttps=true;
    await invoke('save_product_state',{productState:state});
    const bootstrap=await invoke('desktop_bootstrap');
    await invoke('create_ca',{request:{certificatePath:bootstrap.caCertificatePath,privateKeyPath:bootstrap.caPrivateKeyPath,commonName:'Owned desktop certificate acceptance interception',validityDays:2}});
    // Authored global Prompt is saved through the real editor and native command.
    await evaluate(`(async()=>{const shell=${shell};await shell.activateView('settings');shell.settings.showSettingsSection('client-certificates');})()`);
    await until(()=>evaluate(`${shell}.settings.clientCertificates.loaded&&!${shell}.settings.clientCertificates.certificateSettingsBusy`),'certificate settings loaded');
    await evaluate(`(()=>{const editor=${shell}.settings.clientCertificates;const select=editor.certificatePolicyForm.elements.namedItem('default');select.value='prompt';select.dispatchEvent(new Event('change',{bubbles:true}));editor.certificatePolicyForm.requestSubmit();})()`);
    await until(async()=>(await invoke('product_state')).preferences.clientCertificatePolicy.default==='prompt','global Prompt saved');
    for(const route of ['http1','http2','http3']){
      await protocol(route);await beginComposer(fixture[route]);
      await answer('certificate',fixture.identity);await composerResult(true);
      await beginComposer(fixture[route]);await answer('noCertificate');await composerResult(false);
      await beginComposer(fixture[route]);await answer('cancel');
      await until(()=>evaluate(`!${shell}.composer.composerBusy`),'cancelled Composer completion');
      assert.match(await evaluate(`${shell}.composer.composerError`),/Send failed/);
    }
    // An actual QUIC peer timeout expires the still-open panel without sending a key.
    await protocol('http3');await beginComposer(fixture.http3);
    const status=await until(async()=>{const status=await invoke('client_certificate_status');return status.pending.length?status:null;},'timeout request');
    await inspectRequest(status.pending[0].id);
    await evaluate(`(()=>{const select=${queue}.querySelector('select');select.value=${JSON.stringify(fixture.identity)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await until(()=>evaluate(`Boolean(${queue}.querySelector('.certificate-choice-facts'))`),'selected public certificate metadata');
    await screenshot('certificate-prompt-native-wide.png');
    await call('Emulation.setDeviceMetricsOverride',{width:640,height:520,deviceScaleFactor:1,mobile:false});
    await screenshot('certificate-prompt-native-compact.png');await call('Emulation.clearDeviceMetricsOverride');
    await until(()=>evaluate(`${queue}.certificateRequestExpired`),'peer timeout expires selection');
    assert.equal(await evaluate(`${queue}.querySelector('button').disabled`),false);
    await evaluate(`${queue}.closePanel()`);
    // Automatic exact identities are saved through the same editor, with no prompts.
    await until(()=>evaluate(`!${shell}.composer.composerBusy`),'timed out Composer completion');
    // Closing the actual native viewer must abandon its waiting handshake,
    // and viewer IPC must not grant certificates or change saved authorization.
    await protocol('http1');
    const viewerLabel=await invoke('open_trace_viewer',{paths:[]});
    const viewer=await until(async()=>{const targets=await fetch(`http://127.0.0.1:${port}/json/list`).then(response=>response.json());return targets.find(candidate=>candidate.type==='page'&&candidate.id!==target.id&&candidate.url===target.url);},'capture viewer debugging target');
    await until(()=>evaluateTarget(viewer.webSocketDebuggerUrl,`Boolean(${shell}?.viewerMode)`),'viewer hydrated');
    assert.equal(await evaluateTarget(viewer.webSocketDebuggerUrl,`window.__TAURI_INTERNALS__.invoke('save_client_certificate_policy',{policy:{default:'noCertificate',rules:[]}}).then(()=>false,()=>true)`),true);
    await evaluateTarget(viewer.webSocketDebuggerUrl,`(async()=>{const shell=${shell};await shell.activateView('composer');const composer=shell.composer;composer.composerForm.elements.namedItem('url').value=${JSON.stringify(`https://localhost:${fixture.http1}/`)};composer.composerForm.requestSubmit();})()`);
    let viewerPending=await until(async()=>{const status=await invoke('client_certificate_status');return status.pending.find(pending=>pending.caller==='Capture viewer Composer');},'viewer certificate request');
    assert.equal(await evaluateTarget(viewer.webSocketDebuggerUrl,`window.__TAURI_INTERNALS__.invoke('decide_client_certificate',{id:${JSON.stringify(viewerPending.id)},decision:{action:'noCertificate'}}).then(()=>false,()=>true)`),true);
    const viewerGeneration=(await invoke('certificate_test_info')).authorizationGeneration;
    const priorTarget=target;
    await invoke('close_certificate_test_main');
    await until(async()=>!(await fetch(`http://127.0.0.1:${port}/json/list`).then(response=>response.json())).some(candidate=>candidate.id===priorTarget.id),'main window closed');
    assert.equal(await evaluateTarget(viewer.webSocketDebuggerUrl,`window.__TAURI_INTERNALS__.invoke('client_certificate_status').then(status=>status.pending.some(row=>row.id===${JSON.stringify(viewerPending.id)}))`),true,'Closing main cancelled the open viewer');
    await evaluateTarget(viewer.webSocketDebuggerUrl,"window.__TAURI_INTERNALS__.invoke('open_main_window')");
    target=await until(async()=>{const targets=await fetch(`http://127.0.0.1:${port}/json/list`).then(response=>response.json());return targets.find(candidate=>candidate.type==='page'&&candidate.id!==viewer.id&&candidate.url===priorTarget.url);},'reopened main window');
    await connectMain();await call('Runtime.enable');await call('Page.enable');
    await until(()=>evaluate(`Boolean(${shell}?.settings?.ready)`),'reopened main hydrated');
    assert.equal((await invoke('certificate_test_info')).authorizationGeneration,viewerGeneration);
    await answer('noCertificate');
    await until(()=>evaluateTarget(viewer.webSocketDebuggerUrl,`!${shell}.composer.composerBusy`),'preserved viewer completed');
    assert.equal(await evaluateTarget(viewer.webSocketDebuggerUrl,`${shell}.composer.composerError`),'');
    assert.match(await evaluateTarget(viewer.webSocketDebuggerUrl,`${shell}.composer.composerResult.body`),/:false$/);
    await evaluateTarget(viewer.webSocketDebuggerUrl,`${shell}.composer.composerForm.requestSubmit()`);
    viewerPending=await until(async()=>{const status=await invoke('client_certificate_status');return status.pending.find(pending=>pending.caller==='Capture viewer Composer');},'second viewer certificate request');
    await invoke('close_certificate_test_viewer',{label:viewerLabel});
    await until(async()=>!(await invoke('client_certificate_status')).pending.some(pending=>pending.id===viewerPending.id),'closed viewer abandons certificate wait',2000);
    await evaluate(`(async()=>{const shell=${shell};await shell.activateView('settings');shell.settings.showSettingsSection('client-certificates');})()`);
    await until(()=>evaluate(`${shell}.settings.clientCertificates.certificateIdentities.some(identity=>identity.id===${JSON.stringify(fixture.identity)})`),'owned Windows identity listed');
    for(const route of ['http1','http2','http3'])await evaluate(`(async()=>{const editor=${shell}.settings.clientCertificates;editor.addRule();const set=(name,value)=>{const input=editor.certificatePolicyForm.elements.namedItem(name);input.value=String(value);input.dispatchEvent(new Event(input instanceof HTMLSelectElement?'change':'input',{bubbles:true}));};set('host','localhost');set('port',${fixture[route]});set('action','automatic');await Promise.resolve();editor.$flushUpdates();set('identity',${JSON.stringify(fixture.identity)});})()`);
    await evaluate(`${shell}.settings.clientCertificates.certificatePolicyForm.requestSubmit()`);
    await until(async()=>(await invoke('product_state')).preferences.clientCertificatePolicy.rules.length===3,'automatic host rules saved');
    for(const route of ['http1','http2','http3']){await protocol(route);await beginComposer(fixture[route]);await composerResult(true);assert.equal((await invoke('client_certificate_status')).pending.length,0);}
    // Renewal attributes authored in the editor reach the same verified live handshakes.
    const ownedIdentity=(await invoke('client_certificate_identities')).find(identity=>identity.id===fixture.identity);
    assert.match(ownedIdentity.subject,/^CN=Transmog client authentication test [^,]+$/);
    const commonName=ownedIdentity.subject.slice(3);
    const pinned=(await invoke('product_state')).preferences.clientCertificatePolicy;
    await evaluate(`(async()=>{const shell=${shell};await shell.activateView('settings');shell.settings.showSettingsSection('client-certificates');})()`);
    for(const rule of pinned.rules)await evaluate(`(()=>{const editor=${shell}.settings.clientCertificates;editor.chooseRule(${JSON.stringify(rule.id)});const mode=editor.certificatePolicyForm.elements.namedItem('selector');mode.value='attributes';mode.dispatchEvent(new Event('change',{bubbles:true}));editor.$flushUpdates();const subject=editor.certificatePolicyForm.elements.namedItem('CN-subject');subject.value=${JSON.stringify(commonName)};subject.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await evaluate(`${shell}.settings.clientCertificates.certificatePolicyForm.requestSubmit()`);
    await until(async()=>(await invoke('product_state')).preferences.clientCertificatePolicy.rules.every(rule=>rule.mode.selector.identity===null&&rule.mode.selector.subject.CN===commonName),'renewal host rules saved');
    for(const route of ['http1','http2','http3']){await protocol(route);await beginComposer(fixture[route]);await composerResult(true);assert.equal((await invoke('client_certificate_status')).pending.length,0);}
    // The intercepted proxy uses the same real UI decisions on every egress protocol.
    const ca=await readFile(bootstrap.caCertificatePath,'utf8');
    const all=await invoke('product_state');await invoke('save_network_profiles',{expectedRevision:all.preferences.networkProfiles.revision,profiles:all.preferences.networkProfiles.customProfiles,useProfile:'chromium-154'});
    await invoke('save_client_certificate_policy',{policy:{default:'prompt',rules:[]}});
    for(const route of ['http1','http2','http3']){
      const running=await invoke('start_proxy',{request:{caCertificatePath:bootstrap.caCertificatePath,caPrivateKeyPath:bootstrap.caPrivateKeyPath,listen:'127.0.0.1:0',route,allowRemoteClients:false,recordingOnly:false}});
      let result;const request=proxyRequest(running.listener,fixture[route],ca).then(value=>result=value);
      await answer('certificate',fixture.identity);await request;assert.match(result,/:true$/);
      await invoke('stop_application');await until(async()=>(await invoke('app_status')).lifecycle==='stopped','proxy drained');
    }
    const policy={default:'noCertificate',rules:['http1','http2','http3'].map(route=>({id:route,enabled:true,host:'localhost',port:fixture[route],mode:{action:'automatic',selector:{identity:fixture.identity,subject:{},issuer:{}}}}))};
    await invoke('save_client_certificate_policy',{policy});
    const generation=BigInt((await invoke('certificate_test_info')).authorizationGeneration);
    await invoke('delete_certificate_test_key');
    await until(async()=>BigInt((await invoke('certificate_test_info')).authorizationGeneration)>generation,'removed key invalidates connections');
    for(const route of ['http1','http2','http3']){await protocol(route);await beginComposer(fixture[route]);await composerResult(false);assert.equal((await invoke('client_certificate_status')).pending.length,0);}
    const saved={default:'prompt',rules:[{id:'restart',enabled:true,host:'saved-certificate-policy.example',port:443,mode:{action:'noCertificate'}}]};
    await invoke('save_client_certificate_policy',{policy:saved});await writeFile(resolve(output,'saved-policy.json'),JSON.stringify(saved));
    process.stdout.write(JSON.stringify({composerProtocols:['H1','H2','H3'],proxyProtocols:['H1','H2','H3'],realWindowsKey:true,selection:true,noCertificate:true,cancel:true,peerTimeout:true,automaticRules:true,renewalRules:true,deletedKeyAnonymous:true,receivedCertificateEvidence:true,closedViewerCancelled:true,mainClosePreservedViewer:true,viewerPermissionsEnforced:true,operatingSystemTrustChanged:false})+'\n');
  }
} catch(error){await screenshot('certificate-native-failure.png').catch(()=>{});throw error;}
finally {socket.close();}
