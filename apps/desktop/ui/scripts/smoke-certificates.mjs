import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join,dirname} from 'node:path';
import {X509Certificate,createHash} from 'node:crypto';
import {certificateAlpsFixture} from './certificate-alps-fixture.mjs';

export async function smokeCertificates(viewer, traffic, source, nativeFixture, screenshot) {
  const chain=[];
  for(const name of ['leaf','root']) {
    const certificate=new X509Certificate(await readFile(new URL('../../../../crates/app/tests/fixtures/certificates/'+name+'.pem',import.meta.url)));
    chain.push({sha256:createHash('sha256').update(certificate.raw).digest('hex'),der:Array.from(certificate.raw)});
  }
  const tls={serverName:'localhost',serverChain:chain.map(({sha256})=>({sha256})),clientChain:[],chainSource:'verification-path',offeredVersions:['TLSv1.2','TLSv1.3'],offeredCiphers:['ECDHE-ECDSA-AES128-GCM-SHA256'],offeredAlpn:['h2','http/1.1'],trustSource:'Recorded native fixture trust',trustGeneration:42,verification:{Rejected:{error_code:20,depth:1}},errors:['Certificate issuer was not trusted by the recorded snapshot'],details:{}};
  // VerificationResult uses Rust's existing externally tagged representation.
  const performance={points:[],protocols:[],transports:[{leg:'upstream',connectionId:'native-certificate',outcome:'tls-failed',shared:false,sampledOffsetMicros:0,peer:'127.0.0.1:443',local:'127.0.0.1:50000',dnsMicros:null,tcpMicros:1000,tlsMicros:2000,tlsVersion:null,tlsResumed:null,cipher:null,alpn:null,bytesRead:1024,bytesWritten:512,tls}]};
  const records=[...chain.map(definition=>({exchange_id:0,kind:'public-certificate',payload:definition})),{kind:'request-head',payload:{boundary:'client-request',method:'GET',target:'https://localhost/native-certificates',headers:[]}},{kind:'response-head',payload:{boundary:'client-response',status:502,headers:[]}},{kind:'performance',payload:performance},{kind:'failed',payload:{category:'Upstream',message:'Native fixture TLS verification failed'}}];
  const path=join(dirname(source),'certificate-fixture.tmcap');await writeFile(path,nativeFixture(records,105));
  await viewer.evaluate(`${traffic}.importTrace(${JSON.stringify(path)})`);
  const wait=async(expression)=>{const deadline=Date.now()+10000;while(!await viewer.evaluate(expression)){if(Date.now()>deadline)throw new Error('Native certificate viewer did not reach '+expression);await new Promise(resolve=>setTimeout(resolve,50));}};
  await wait(`!${traffic}.importingTrace && ${traffic}.sessions.some(row=>row.path==='/native-certificates')`);
  const id=await viewer.evaluate(`(async()=>{const workspace=${traffic},row=workspace.sessions.find(row=>row.path==='/native-certificates');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`);
  const context=await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('inspect_certificates',{id:${JSON.stringify(id)}})`);
  assert.equal(context.certificates.length,2);assert.equal(context.connections[0].serverChain.length,2);
  assert.ok(context.certificates.some(certificate=>certificate.subject==='CN=localhost'&&certificate.derBase64));
  const trigger=await viewer.evaluate(`(()=>{const button=Array.from(${traffic}.querySelectorAll('.selection-actions button')).find(button=>button.textContent==='Details…');button.scrollIntoView({block:'nearest'});const rect=button.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`);
  for(const type of ['mousePressed','mouseReleased'])await viewer.call('Input.dispatchMouseEvent',{type,...trigger,button:'left',clickCount:1});
  const dialog=`${traffic}.timingDialog`,component=`${traffic}.certificateInspector`;
  await wait(`${dialog}.open && !${traffic}.timingBusy`);
  const certificateTab=await viewer.evaluate(`(()=>{const button=${dialog}.querySelector('#details-tab-certificates');const rect=button.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`);
  for(const type of ['mousePressed','mouseReleased'])await viewer.call('Input.dispatchMouseEvent',{type,...certificateTab,button:'left',clickCount:1});
  await wait(`${dialog}.open && !${component}.certificateBusy && ${component}.certificateServerChain.length===2`);
  assert.ok(await viewer.evaluate(`${dialog}.textContent.includes('Certificate issuer was not trusted')`));
  await viewer.evaluate(`(()=>{const button=Array.from(${dialog}.querySelectorAll('.certificate-chain button'))[1];button.click();})()`);
  await wait(`${component}.certificateSubject.includes('fixture root')`);
  if(screenshot)await writeFile(screenshot.replace('.png','-certificates.png'),Buffer.from((await viewer.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  await viewer.call('Emulation.setDeviceMetricsOverride',{width:640,height:520,deviceScaleFactor:1,mobile:false});
  await viewer.evaluate(`(()=>{Array.from(${dialog}.querySelectorAll('button')).find(button=>button.textContent==='Hex').click();})()`);
  await wait(`${dialog}.querySelector('hex-viewer')?.hexRows.length>0`);
  assert.ok(await viewer.evaluate(`(()=>{const dialog=${dialog},button=Array.from(dialog.querySelectorAll('footer button')).find(button=>button.textContent==='Close'),rect=button.getBoundingClientRect(),bounds=dialog.getBoundingClientRect();return bounds.width<=innerWidth&&bounds.height<=innerHeight&&button.getRootNode().elementFromPoint(rect.x+rect.width/2,rect.y+rect.height/2)===button;})()`));
  if(screenshot)await writeFile(screenshot.replace('.png','-certificates-compact.png'),Buffer.from((await viewer.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  for(const type of ['keyDown','keyUp'])await viewer.call('Input.dispatchKeyEvent',{type,key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
  await wait(`!${dialog}.open`);
  assert.ok(await viewer.evaluate(`(()=>{const active=${traffic}.getRootNode().activeElement;return active===${traffic}.detailsButton;})()`));
  await viewer.call('Emulation.clearDeviceMetricsOverride');
  await viewer.evaluate(`(async()=>{await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(id)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`);
  await smokeFallbackConnection(viewer,traffic,source,nativeFixture,chain,performance,screenshot);
}

async function smokeFallbackConnection(viewer,traffic,source,nativeFixture,chain,performance,screenshot) {
  const alpsFixture=certificateAlpsFixture();
  const connected=structuredClone(performance.transports[0]);
  Object.assign(connected,{connectionId:'native-origin',outcome:'connected',tlsVersion:'TLSv1.3',tlsResumed:false,cipher:'TLS_AES_128_GCM_SHA256'});
  Object.assign(connected.tls,{verification:'Verified',errors:[],details:{'Key exchange':'Hybrid post-quantum key exchange','Negotiated group':'X25519MLKEM768','Negotiated group ID':'0x11ec','Peer handshake signature':'ecdsa_secp256r1_sha256','Session resumed':'No'}});
  Object.assign(connected.tls.details,{'Negotiated TLS version':'TLSv1.3','Negotiated cipher':'TLS_AES_128_GCM_SHA256','https-server-protocol':'TLSv1.3','https-server-cipher':'TLS_AES_128_GCM_SHA256'});
  connected.tls.http2Alps={negotiated:true,peerSettings:alpsFixture.bytes};
  connected.tls.details['HTTP/2 ALPS']=`Negotiated (${alpsFixture.bytes.length} peer bytes)`;
  const client=structuredClone(connected);Object.assign(client,{leg:'client',connectionId:'native-client'});
  client.tls.details={'Key exchange':'Classical key exchange','Negotiated group':'X25519','Negotiated group ID':'0x001d','Local handshake signature':'ecdsa_secp256r1_sha256'};
  client.tls.http2Alps={negotiated:false,peerSettings:[]};
  const controls=structuredClone(client);controls.connectionId='native-controls';
  Object.assign(controls.tls.details,{'Negotiated group':'X25519\0MLKEM768','Key exchange':'Hybrid post-quantum key exchange'});
  Object.assign(connected.tls.details,{'Certificate Transparency':'Compliant','Verified SCTs':'3','CT log list source':'Installed WebView2 PKIMetadata/ct_config.pb','CT log list updated':'2026-10-04T12:00:00Z','CRLSet result':'Inconclusive','CRLSet sequence':'187','Stapled OCSP':'Not provided'});
  const sample={...performance,transports:[performance.transports[0],connected,client,controls]};
  const records=[...chain.map(definition=>({exchange_id:0,kind:'public-certificate',payload:definition})),{kind:'request-head',payload:{boundary:'client-request',method:'GET',target:'https://localhost/native-fallback',headers:[]}},{kind:'response-head',payload:{boundary:'client-response',status:200,headers:[]}},{kind:'performance',payload:sample},{kind:'completed',payload:null}];
  const path=join(dirname(source),'fallback-fixture.tmcap');await writeFile(path,nativeFixture(records,106));
  await viewer.evaluate(`${traffic}.importTrace(${JSON.stringify(path)})`);
  const wait=async expression=>{const deadline=Date.now()+10000;while(!await viewer.evaluate(expression)){if(Date.now()>deadline)throw new Error('Native fallback viewer did not reach '+expression);await new Promise(resolve=>setTimeout(resolve,50));}};
  await wait(`!${traffic}.importingTrace && ${traffic}.sessions.some(row=>row.path==='/native-fallback')`);
  const id=await viewer.evaluate(`(async()=>{const workspace=${traffic},row=workspace.sessions.find(row=>row.path==='/native-fallback');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`);
  const context=await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('inspect_certificates',{id:${JSON.stringify(id)}})`);
  assert.equal(context.defaultConnectionId,'upstream:native-origin');
  const originPolicy=context.connections.find(connection=>connection.id==='upstream:native-origin');
  for(const label of ['Certificate Transparency','CT log list updated','CRLSet result','CRLSet sequence','Stapled OCSP']) {
    assert.ok(originPolicy.policyFields.some(field=>field.label===label));
    assert.equal(originPolicy.diagnosticFields.some(field=>field.label===label),false);
  }
  const alps=context.connections.find(connection=>connection.id==='upstream:native-origin').alps;
  assert.equal(alps.payloadBase64,alpsFixture.view.payloadBase64);
  assert.ok(alps.fields.some(field=>field.label==='Requested Client Hints (ignored)'&&field.value==='Sec-CH-UA-Platform, Sec-CH-UA-Model'));
  assert.equal(new Set(context.connections.map(connection=>connection.title)).size,4);
  const controlField=context.connections.find(connection=>connection.id==='client:native-controls').fields.find(field=>field.label==='Negotiated group');
  assert.equal(controlField.value,'X25519\0MLKEM768');assert.equal(controlField.displayValue,'X25519\\u{0000}MLKEM768');
  await viewer.evaluate(`${traffic}.showTimings()`);
  const dialog=`${traffic}.timingDialog`,component=`${traffic}.certificateInspector`;
  await wait(`${dialog}.open && !${traffic}.timingBusy`);
  await viewer.evaluate(`${dialog}.querySelector('#details-tab-connection').click()`);
  await wait(`!${component}.certificateBusy && ${component}.querySelector('select').value==='upstream:native-origin' && ${component}.textContent.includes('X25519MLKEM768')`);
  assert.equal(await viewer.evaluate(`${component}.textContent.includes('Certificate issuer was not trusted')`),false);
  for(const [width,height,label] of [[1440,900,'wide'],[800,600,'compact']]) {
    await viewer.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    assert.ok(await viewer.evaluate(`(()=>{const summary=${component}.querySelector('.certificate-tls-summary');summary.scrollIntoView({block:'nearest'});const rect=summary.getBoundingClientRect();return summary.textContent.includes('TLSv1.3 · X25519MLKEM768')&&rect.top>=0&&rect.bottom<=innerHeight;})()`));
    assert.ok(await viewer.evaluate(`(()=>{const policy=Array.from(${component}.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent==='Connection and recorded policy');return policy&&!policy.open;})()`));
    assert.ok(await viewer.evaluate(`(()=>{const group=Array.from(${component}.querySelectorAll('dd')).find(node=>node.textContent==='X25519MLKEM768');group.scrollIntoView({block:'nearest'});const rect=group.getBoundingClientRect();return rect.height>0&&rect.top>=0&&rect.bottom<=innerHeight;})()`));
    assert.ok(await viewer.evaluate(`(()=>{const button=Array.from(${dialog}.querySelectorAll('footer button')).find(button=>button.textContent==='Close'),rect=button.getBoundingClientRect();return rect.bottom<=innerHeight&&button.getRootNode().elementFromPoint(rect.x+rect.width/2,rect.y+rect.height/2)===button;})()`));
    if(screenshot)await writeFile(screenshot.replace('.png',`-fallback-${label}.png`),Buffer.from((await viewer.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  }
  await viewer.evaluate(`Array.from(${component}.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent==='Connection and recorded policy').querySelector('summary').focus()`);
  await viewer.call('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13,text:'\r',unmodifiedText:'\r'});
  await viewer.call('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});
  await wait(`Array.from(${component}.querySelectorAll('details')).some(node=>node.open&&node.querySelector('summary')?.textContent==='Connection and recorded policy')`);
  for(const [width,height,label] of [[1440,900,'wide'],[800,600,'compact']]) {
    await viewer.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    assert.ok(await viewer.evaluate(`(()=>{const value=Array.from(${component}.querySelectorAll('dd')).find(node=>node.textContent==='Compliant');value.scrollIntoView({block:'nearest'});const rect=value.getBoundingClientRect();return rect.height>0&&rect.top>=0&&rect.bottom<=innerHeight;})()`));
    if(screenshot)await writeFile(screenshot.replace('.png',`-ct-policy-${label}.png`),Buffer.from((await viewer.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  }
  await viewer.evaluate(`Array.from(${component}.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent==='Connection and recorded policy').querySelector('summary').click()`);
  await viewer.evaluate(`${component}.querySelector('.certificate-alps > summary').click()`);
  await wait(`${component}.querySelector('.certificate-alps').open && ${component}.textContent.includes('Sec-CH-UA-Platform, Sec-CH-UA-Model')`);
  await viewer.evaluate(`${component}.querySelector('.certificate-alps-raw > summary').click()`);
  await wait(`${component}.querySelector('.certificate-alps hex-viewer')?.hexRows.length>0`);
  assert.deepEqual(await viewer.evaluate(`Array.from(${component}.querySelector('.certificate-alps hex-viewer').preview.bytes)`),Array.from(Buffer.from(alpsFixture.view.payloadBase64,'base64')));
  await viewer.call('Emulation.setDeviceMetricsOverride',{width:640,height:520,deviceScaleFactor:1,mobile:false});
  await viewer.evaluate(`${component}.querySelector('.certificate-alps-raw').scrollIntoView({block:'nearest'})`);
  if(screenshot)await writeFile(screenshot.replace('.png','-alps-contents-compact.png'),Buffer.from((await viewer.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
  await viewer.evaluate(`(()=>{const select=${component}.querySelector('select');select.value='upstream:native-certificate';select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  await wait(`!${component}.alpsRecorded`);
  assert.equal(await viewer.evaluate(`${component}.querySelector('.certificate-alps')!==null`),false);
  await wait(`${component}.textContent.includes('Certificate issuer was not trusted')`);
  assert.equal(await viewer.evaluate(`${component}.textContent.includes('X25519MLKEM768')`),false);
  await viewer.evaluate(`(()=>{const select=${component}.querySelector('select');select.value='client:native-controls';select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  await wait(`${component}.textContent.includes('Invisible characters shown as Unicode escapes.')`);
  assert.equal(await viewer.evaluate(`${component}.textContent.includes(String.fromCharCode(0))`),false);
  assert.equal(await viewer.evaluate(`${component}.textContent.includes(String.fromCharCode(92)+'u{0000}')`),true);
  await viewer.evaluate(`(()=>{const select=${component}.querySelector('select');select.value='client:native-client';select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  await wait(`${component}.textContent.includes('Classical key exchange')`);
  assert.equal(await viewer.evaluate(`${component}.textContent.includes('X25519MLKEM768')`),false);
  await viewer.evaluate(`${traffic}.closeTimings()`);
  await viewer.call('Emulation.clearDeviceMetricsOverride');
  await viewer.evaluate(`(async()=>{await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(id)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`);
  process.stdout.write('Native fallback selection, distinct attempts and per-leg PQC context verified.\n');
}
