import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join,dirname} from 'node:path';
import {X509Certificate,createHash} from 'node:crypto';

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
  const trigger=await viewer.evaluate(`(()=>{const button=Array.from(${traffic}.querySelectorAll('.selection-actions button')).find(button=>button.textContent==='Certificates…');button.scrollIntoView({block:'nearest'});const rect=button.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`);
  for(const type of ['mousePressed','mouseReleased'])await viewer.call('Input.dispatchMouseEvent',{type,...trigger,button:'left',clickCount:1});
  const component=`${traffic}.certificateViewer`,dialog=`${component}.certificateDialog`;
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
  assert.ok(await viewer.evaluate(`(()=>{const active=${traffic}.getRootNode().activeElement;return active?.textContent==='Certificates…';})()`));
  await viewer.call('Emulation.clearDeviceMetricsOverride');
  await viewer.evaluate(`(async()=>{await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(id)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`);
}
