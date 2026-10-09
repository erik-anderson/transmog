import assert from 'node:assert/strict';
import {writeFile, readFile, glob} from 'node:fs/promises';
import {join,dirname} from 'node:path';
import {spawn} from 'node:child_process';
import {createServer} from 'node:http';
import {zstdCompressSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {smokeTrafficList} from './smoke-traffic-list.mjs';

// Native probes use the current plain indexed container; encrypted I/O is covered
// by the CLI and Rust tests. Keep fixtures independent of historical layouts.
function nativeFixture(records, exchangeId) {
  const crc=bytes=>{let value=0xffffffff;for(const byte of bytes){value^=byte;for(let bit=0;bit<8;bit++)value=(value>>>1)^((value&1)?0xedb88320:0);}return (value^0xffffffff)>>>0;};
  const encode=(bytes,counter)=>{
    const compressed=zstdCompressSync(bytes),useCompressed=compressed.length<bytes.length;
    const prefix=Buffer.alloc(13);prefix.writeBigUInt64LE(BigInt(counter));prefix.writeUInt32LE(bytes.length,8);prefix[12]=Number(useCompressed);
    const payload=Buffer.concat([prefix,useCompressed?compressed:bytes]);
    const envelope=Buffer.alloc(8);envelope.writeUInt32LE(payload.length);envelope.writeUInt32LE(crc(payload),4);
    return Buffer.concat([envelope,payload]);
  };
  const header=Buffer.from(JSON.stringify({version:1,compression:'zstd',cipher:'none',kdf:'none',salt:Array(16).fill(0),nonce_prefix:Array(4).fill(0),memory_kib:0,iterations:0,lanes:0}));
  const size=Buffer.alloc(4);size.writeUInt32LE(header.length);
  const frames=[Buffer.from('TMCAP001\0'),size,header];
  for(const [index,original] of [...records,{kind:'seal',payload:{record_count:records.length}}].entries()) {
    const record={revision:3,sequence:index+1,exchange_id:original.kind==='seal'?0:exchangeId,...structuredClone(original)};
    let body;
    if(record.kind==='body-segment'&&Array.isArray(record.payload.bytes)) {
      const bytes=Buffer.from(record.payload.bytes);body=encode(bytes,index*2+2);
      record.body_index={frame_bytes:body.length,bytes:bytes.length,digest:Array.from(createHash('sha256').update(bytes).digest())};record.payload.bytes=null;
    }
    frames.push(encode(Buffer.from(JSON.stringify(record)),index*2+1));if(body)frames.push(body);
  }
  return Buffer.concat(frames);
}

const argument=name=>{const index=process.argv.indexOf(name);return index<0?undefined:process.argv[index+1];};
const port=argument('--port'), source=argument('--source'), executable=argument('--executable'), screenshot=argument('--screenshot');
const nativeSource=argument('--native-source');
const trafficSource=argument('--traffic-source');
const pageSource=argument('--page-source');
const profileRoot=argument('--profile-root');
const processId=argument('--process-id'), closeHelper=argument('--close-helper');
async function closeNative(title){
  if(!processId||!closeHelper)throw new Error('Native close verification requires an isolated process identity.');
  const child=spawn('pwsh.exe',['-NoProfile','-NonInteractive','-File',closeHelper,'-ProbeProcessId',processId,'-WindowTitle',title],{windowsHide:true,stdio:'inherit'});
  await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',code=>code===0?resolve():reject(new Error('Native close probe exited '+code)));});
}
if(!port||!source||!executable)throw new Error('Viewer verification requires a DevTools port, isolated executable, and fixture source.');
const sockets=[];
async function connect(target){
  const socket=new WebSocket(target.webSocketDebuggerUrl);sockets.push(socket);
  await new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});});
  let sequence=0;const pending=new Map();
  socket.addEventListener('message',({data})=>{const result=JSON.parse(String(data));if(result.id){const reply=pending.get(result.id);pending.delete(result.id);if(reply){clearTimeout(reply.timer);if(result.error)reply.reject(new Error(result.error.message));else reply.resolve(result.result);}}});
  const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++sequence;const timer=setTimeout(()=>{pending.delete(id);reject(new Error(method+' timed out'));},15000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const result=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(result.exceptionDetails)throw new Error(JSON.stringify(result.exceptionDetails));return result.result?.value;};
  return {call,evaluate};
}
async function targets(){
  const ports=new Set([port]);
  if(profileRoot)for await(const path of glob('**/DevToolsActivePort',{cwd:profileRoot})){
    try{ports.add((await readFile(join(profileRoot,path),'utf8')).split(/\r?\n/)[0]);}catch{/* A closed profile may be removed during discovery. */}
  }
  const pages=await Promise.all([...ports].map(async value=>{
    try{return await fetch(`http://127.0.0.1:${value}/json/list`).then(response=>response.json());}catch{return [];}
  }));
  return pages.flat().filter(target=>target.type==='page');
}
async function waitFor(check,message){const deadline=Date.now()+15000;while(Date.now()<deadline){const value=await check();if(value)return value;await new Promise(resolve=>setTimeout(resolve,50));}throw new Error(message);}
const root='document.querySelector("app-shell")';
const traffic=root+'.shadowRoot.querySelector("traffic-workspace")';
try{
  const pages=await targets();assert.equal(pages.length,1,'File launch opened extra windows');
  const viewer=await connect(pages[0]);
  await waitFor(()=>viewer.evaluate(`${root}?.viewerMode && ${traffic}?.sessions.length===1 && !${traffic}.importingTrace`),'Saved-file viewer did not finish automatic import');
  process.stdout.write('Viewer file import verified.\n');
  const result=await viewer.evaluate(`(async()=>{
    const shell=${root}, workspace=${traffic};
    const controls=[...shell.shadowRoot.querySelectorAll('proxy-toggle,settings-workspace,breakpoint-workspace,automation-workspace')];
    if(controls.length)throw new Error('Viewer exposed proxy controls');
    const id=workspace.sessions[0].id;
    let denial='';try{await window.__TAURI_INTERNALS__.invoke('start_proxy',{request:{}});}catch(error){denial=String(error);}
    const detail=await window.__TAURI_INTERNALS__.invoke('session_detail',{id});
    const headers=await window.__TAURI_INTERNALS__.invoke('copy_all_headers',{id});
    const composer=await window.__TAURI_INTERNALS__.invoke('composer_source',{id});
    await workspace.selectTraffic(workspace.sessions[0],new MouseEvent('click'));
    return {denial,sourceIp:detail.sourceIp,headers,body:composer.body,traceId:detail.traceId};
  })()`);
  assert.match(result.denial,/only in the main proxy window/);
  assert.equal(result.sourceIp,'192.0.2.25');assert.match(result.headers,/\r\n\r\n\r\nHTTP\/1\.1 200 Fixture/);assert.equal(result.body,'');assert.ok(result.traceId);
  process.stdout.write('Viewer inspection and permissions verified.\n');
  const search=await viewer.evaluate(`(async()=>{const workspace=${traffic};workspace.searchMetadata=false;workspace.searchRequestHeaders=false;workspace.searchResponseHeaders=false;workspace.searchBodies=true;workspace.selectSearchMatches=true;workspace.searchInput.value='viewer response';await workspace.runContentSearch();return {matches:workspace.contentMatchCount,selected:workspace.selectedTrafficCount,status:workspace.contentSearchStatus};})()`);
  assert.equal(search.matches,1);assert.equal(search.selected,1);assert.doesNotMatch(search.status,/binary/i);
  await viewer.evaluate(`(async()=>{const workspace=${traffic};workspace.searchMode='regex';workspace.searchInput.value='(';await workspace.runContentSearch();if(!workspace.contentSearchStatus.startsWith('Search failed:'))throw new Error('Invalid regex was not explained');await workspace.clearContentSearch();workspace.searchMode='text';})()`);
  await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('open_main_window')`);
  const mainTarget=await waitFor(async()=>{const all=await targets();return all.find(target=>target.id!==pages[0].id);},'Main window did not open');
  const main=await connect(mainTarget);
  await waitFor(()=>main.evaluate(`${root} && !${root}.viewerMode && ${traffic}?.queryLoaded`),'Main window did not initialize');
  assert.equal(await main.evaluate(`${traffic}.sessions.length`),0,'Viewer imported into the main catalog');
  process.stdout.write('Main window opened with an independent catalog.\n');
  const child=spawn(executable,[source],{windowsHide:true,stdio:'ignore'});
  await new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',code=>code===0?resolve():reject(new Error('File handoff exited '+code)));});
  await waitFor(()=>main.evaluate(`${traffic}.openTraceDialog.open`),'Existing main window did not offer an import/viewer choice');
  await main.evaluate(`${traffic}.chooseOpenedTrace(false)`);
  await waitFor(()=>main.evaluate(`${traffic}.sessions.length===1 && !${traffic}.importingTrace`),'Choice did not import into current session');
  assert.equal(await viewer.evaluate(`${traffic}.sessions.length`),1,'Main import changed the separate viewer');
  await viewer.evaluate(`${traffic}.showTraceMetadata(${JSON.stringify(result.traceId)})`);
  await waitFor(()=>viewer.evaluate(`${traffic}.metadataTraceName==='viewer-fixture.saz'`),'Request trace metadata navigation failed');
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot,Buffer.from(image.data,'base64'));}
  await viewer.evaluate(`${traffic}.closeMetadata()`);
  await viewer.evaluate(`${traffic}.showTimings()`);
  await waitFor(()=>viewer.evaluate(`!${traffic}.timingBusy && ${traffic}.timingSaved.length>0`),'Imported SAZ timer evidence did not appear');
  const timingState=await viewer.evaluate(`({open:${traffic}.timingDialog.open,summary:${traffic}.timingSummary,unknown:${traffic}.timingPhases[0].value})`);
  assert.ok(timingState.open);assert.match(timingState.summary,/no measured proxy timeline/);assert.equal(timingState.unknown,'Unavailable');
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-timings.png'),Buffer.from(image.data,'base64'));}

  await viewer.evaluate(`${traffic}.closeTimings()`);
  // A real native capture exercises import, timing persistence and WebView2
  // geometry together; it uses only deterministic, nonsensitive fixture data.
  const timingSource=join(dirname(source),'timing-fixture.tmcap');
  const perf={points:[{milestone:'request-headers',unixMillis:1800000000000,offsetMicros:0},{milestone:'exchange-done',unixMillis:1800000000004,offsetMicros:4000}],protocols:[],transports:[{leg:'upstream',connectionId:'native-tcp-stats',shared:true,outcome:'connected',sampledOffsetMicros:3500,bytesRead:8192,bytesWritten:4096,tcpSampledOffsetMicros:3000,tcp:{rttMicros:2400,congestionWindow:65536,sendWindow:32768,retransmittedBytes:2048},socketIo:{writeWaitMicros:1750,writeWaits:2,lastWriteOffsetMicros:2800,lastFlushOffsetMicros:2900}}],work:[{kind:'hook',label:'Request headers · native support script',beganOffsetMicros:0,endedOffsetMicros:1000,busyNanos:1000000,calls:1},{kind:'transform',label:'Response body · native support script',beganOffsetMicros:1200,endedOffsetMicros:3500,busyNanos:500000,calls:3}]};
  const fixture=[{kind:'request-head',payload:{boundary:'client-request',method:'GET',target:'https://example.invalid/native-waterfall',headers:[]}},{kind:'response-head',payload:{boundary:'client-response',status:200,headers:[]}},{kind:'body-segment',payload:{boundary:'client-response',byte_count:0,bytes:[],truncated:false}},{kind:'performance',payload:perf},{kind:'completed',payload:null}];
  await writeFile(timingSource,nativeFixture(fixture,99));
  await viewer.evaluate(`${traffic}.importTrace(${JSON.stringify(timingSource)})`);
  await waitFor(()=>viewer.evaluate(`!${traffic}.importingTrace && ${traffic}.sessions.some(row=>row.path==='/native-waterfall')`),'Native timing fixture did not import');
  const timingId=await viewer.evaluate(`(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/native-waterfall');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`);
  await viewer.evaluate(`${traffic}.showTimings()`);
  await waitFor(()=>viewer.evaluate(`!${traffic}.timingBusy && ${traffic}.timingWaterfall.length+${traffic}.timingMinor.length>=3`),'Native timing waterfall was not restored');
  assert.equal(await viewer.evaluate(`Array.from(${traffic}.timingDialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent.includes('Proxy operations under 1 ms'))?.open`),false);
  await viewer.evaluate(`Array.from(${traffic}.timingDialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent.includes('Proxy operations under 1 ms')).open=true`);
  const chart=await viewer.evaluate(`(()=>{const workspace=${traffic};const bars=Array.from(workspace.timingDialog.querySelectorAll('.waterfall-bar'));return {rows:workspace.timingWaterfall.length+workspace.timingMinor.length,svg:bars.every(node=>node.namespaceURI==='http://www.w3.org/2000/svg'),painted:bars.every(node=>node.getBoundingClientRect().width>0),report:workspace.timingReportText.includes('native support script'),footer:workspace.timingDialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight};})()`);
  assert.ok(chart.rows>=3&&chart.svg&&chart.painted&&chart.report&&chart.footer,'Native timing waterfall failed to render or left its actions unreachable: '+JSON.stringify(chart));
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-waterfall.png'),Buffer.from(image.data,'base64'));}
  await viewer.evaluate(`(()=>{const dialog=${traffic}.timingDialog;const section=Array.from(dialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent==='Proxy ↔ upstream');section.open=true;const label=Array.from(section.querySelectorAll('dt')).find(node=>node.textContent==='TCP estimated round trip');label.scrollIntoView({block:'center'});})()`);
  assert.ok(await viewer.evaluate(`${traffic}.timingReportText.includes('TCP estimated round trip: 2.4 ms')`));
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-tcp.png'),Buffer.from(image.data,'base64'));}
  await viewer.evaluate(`(async()=>{const workspace=${traffic};workspace.closeTimings();await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(timingId)}],restore:false});await workspace.refreshSessions(undefined,true);})()`);

  await viewer.evaluate(`${traffic}.closeTimings()`);
  // Exercise a real viewer Composer upload. The only contacted endpoint is
  // this owned loopback fixture; no captured URL or outside service is replayed.
  const replayLength=5*1024*1024+7;let replayBytes=0,replayError='';
  const replayServer=createServer(async(request,response)=>{
    try {for await(const chunk of request){replayBytes+=chunk.length;if(!chunk.every(byte=>byte===7))throw new Error('Captured replay bytes changed');}if(replayBytes!==replayLength)throw new Error('Captured replay was incomplete');response.end('received '+replayBytes);}
    catch(error){replayError=String(error);response.statusCode=500;response.end('fixture upload failed');}
  });
  await new Promise(resolve=>replayServer.listen(0,'127.0.0.1',resolve));
  try {
    const url='http://127.0.0.1:'+replayServer.address().port+'/native-replay';
    const records=[{kind:'request-head',payload:{boundary:'client-request',method:'POST',target:url,headers:[{name:Array.from(Buffer.from('Content-Length')),value:Array.from(Buffer.from(String(replayLength)))}]}},{kind:'response-head',payload:{boundary:'client-response',status:200,headers:[]}}];
    for(let remaining=replayLength;remaining>0;){const count=Math.min(remaining,512*1024);records.push({kind:'body-segment',payload:{boundary:'client-request',byte_count:count,bytes:Array(count).fill(7),truncated:false}});remaining-=count;}
    records.push({kind:'body-segment',payload:{boundary:'client-response',byte_count:0,bytes:[],truncated:false}},{kind:'completed',payload:null});
    const replaySource=join(dirname(source),'replay-fixture.tmcap');await writeFile(replaySource,nativeFixture(records,101));
    await viewer.evaluate(`${traffic}.importTrace(${JSON.stringify(replaySource)})`);
    const replayImportStatus=await viewer.evaluate(`${traffic}.importStatus`);assert.ok(!replayImportStatus.startsWith('Import failed'),replayImportStatus);
    await waitFor(()=>viewer.evaluate(`!${traffic}.importingTrace && ${traffic}.sessions.some(row=>row.path==='/native-replay')`),'Native replay fixture did not import');
    const replayId=await viewer.evaluate(`(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/native-replay');await workspace.selectTraffic(row,new MouseEvent('click'));await workspace.replaySelected();return row.id;})()`);
    const composer=root+'.shadowRoot.querySelector("composer-workspace")';
    await waitFor(()=>viewer.evaluate(`${composer}?.composerBodyMode==='captured' && !${composer}.composerBodyMissing`),'Native captured body did not load as a streamed Composer draft');
    assert.equal(await viewer.evaluate(`${composer}.input('body').value.length`),0,'Large captured body entered the text editor');
    await viewer.evaluate(`(async()=>{const workspace=${composer};workspace.input('nonIdempotent').checked=true;await workspace.executeComposer(new Event('submit',{cancelable:true}));})()`);
    assert.equal(replayBytes,replayLength);assert.equal(replayError,'');
    const replayResult=await viewer.evaluate(`({status:${composer}.composerResult?.status,body:${composer}.composerResult?.body,source:${composer}.composerResult?.source,error:${composer}.composerError})`);
    assert.equal(replayResult.error,'');assert.equal(replayResult.status,200);assert.equal(replayResult.body,'received '+replayLength);assert.equal(replayResult.source.entryId,replayId);assert.ok(replayResult.source.traceId);
    await viewer.evaluate(`(async()=>{const workspace=${composer};workspace.composerPane='request';const details=workspace.shadowRoot?.querySelector('.composer-history')??workspace.querySelector('.composer-history');details.open=true;await workspace.refreshHistory();})()`);
    await waitFor(()=>viewer.evaluate(`${composer}.composerHistory.length>0 && !${composer}.composerHistoryBusy`),'Latest Composer history refresh did not complete');
    assert.equal(await viewer.evaluate(`${composer}.composerHistory[0].sourceId`),replayId);
    assert.ok(await viewer.evaluate(`${composer}.composerHistory[0].sourceLabel.startsWith('replay-fixture.tmcap')`));
    if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-composer.png'),Buffer.from(image.data,'base64'));}
    await viewer.call('Emulation.setDeviceMetricsOverride',{width:760,height:520,deviceScaleFactor:1,mobile:false});
    assert.ok(await viewer.evaluate(`${composer}.composerForm.getBoundingClientRect().width<760`));
    if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-composer-compact.png'),Buffer.from(image.data,'base64'));}
    await viewer.call('Emulation.clearDeviceMetricsOverride');
    await viewer.evaluate(`(async()=>{await ${root}.activateView('traffic');const workspace=${traffic};workspace.searchInput.value='Content-Length';workspace.searchMetadata=false;workspace.searchRequestHeaders=true;workspace.searchResponseHeaders=false;workspace.searchBodies=false;workspace.searchRequestBodies=false;workspace.selectSearchMatches=true;await workspace.runContentSearch();})()`);
    assert.ok(await viewer.evaluate(`${traffic}.searchMatchIds.includes(${JSON.stringify(replayId)})`),'Native scoped search did not find the retained request header');
    await viewer.evaluate(`${traffic}.showMatches()`);
    assert.equal(await viewer.evaluate(`${traffic}.matchText`),'Content-Length');
    assert.ok(await viewer.evaluate(`${traffic}.matchField.startsWith('Client request') && ${traffic}.matchDialog.querySelector('mark').textContent==='Content-Length'`));
    if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-matches.png'),Buffer.from(image.data,'base64'));}
    await viewer.call('Emulation.setDeviceMetricsOverride',{width:760,height:520,deviceScaleFactor:1,mobile:false});
    assert.ok(await viewer.evaluate(`(()=>{const dialog=${traffic}.matchDialog;return dialog.getBoundingClientRect().width<=760&&dialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight;})()`));
    if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-matches-compact.png'),Buffer.from(image.data,'base64'));}
    await viewer.call('Emulation.clearDeviceMetricsOverride');await viewer.evaluate(`(async()=>{${traffic}.closeMatches();await ${traffic}.clearContentSearch();})()`);
    await viewer.evaluate(`(async()=>{await ${root}.activateView('traffic');await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(replayId)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`);
    process.stdout.write('Viewer streamed captured binary replay, original trace association and history verified.\n');
  }finally{replayServer.closeAllConnections();await new Promise(resolve=>replayServer.close(resolve));}

  await viewer.evaluate(`${traffic}.showSaveTrace()`);
  const saveChoices=await viewer.evaluate(`({open:${traffic}.saveTraceDialog.open,network:${traffic}.saveTraceForm.elements.namedItem('networkContext').checked,redact:${traffic}.saveTraceForm.elements.namedItem('redactHeaders').checked})`);
  assert.ok(saveChoices.open);assert.equal(saveChoices.network,false);assert.equal(saveChoices.redact,false);
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-save.png'),Buffer.from(image.data,'base64'));}
  await viewer.evaluate(`${traffic}.closeSaveTrace()`);
  const before=(await targets()).map(target=>target.id);
  await main.evaluate(`window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(source)}]})`);
  const additional=await waitFor(async()=>{const all=await targets();return all.find(target=>!before.includes(target.id));},'Additional viewer did not open');
  const second=await connect(additional);
  await waitFor(()=>second.evaluate(`${root}?.viewerMode && ${traffic}?.sessions.length===1 && !${traffic}.importingTrace`),'Additional viewer did not load its saved capture');
  if(trafficSource){
    const existing=(await targets()).map(target=>target.id);
    await main.evaluate(`window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(trafficSource)}]})`);
    const target=await waitFor(async()=>{const all=await targets();return all.find(target=>!existing.includes(target.id));},'Large traffic viewer did not open');
    const large=await connect(target);await smokeTrafficList(large,screenshot);
  }
  if(nativeSource){
    const existing=(await targets()).map(target=>target.id);
    await main.evaluate(`window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(nativeSource)}]})`);
    const target=await waitFor(async()=>{const all=await targets();return all.find(target=>!existing.includes(target.id));},'Native capture viewer did not open');
    const capture=await connect(target);
    await waitFor(()=>capture.evaluate(`${root}?.viewerMode && ${traffic}?.sessions.length>=1 && !${traffic}.importingTrace`),'Native capture did not import');
    const htmlId=await capture.evaluate(`(()=>{const rows=${traffic}.sessions.filter(row=>row.status===200&&row.contentType?.includes('text/html'));rows.sort((a,b)=>(b.responseBytes??0)-(a.responseBytes??0));return rows[0]?.id;})()`);
    if(htmlId){
      const beforePreview=(await targets()).map(row=>row.id);
      const label=await capture.evaluate(`window.__TAURI_INTERNALS__.invoke('open_captured_page',{id:${JSON.stringify(htmlId)},enableScripts:false,options:{scope:'original-trace'},operationId:'external-page-probe'})`);
      const previewTarget=await waitFor(async()=>{const all=await targets();return all.find(row=>!beforePreview.includes(row.id));},'Saved HTML preview did not open');
      const preview=await connect(previewTarget);
      await waitFor(()=>preview.evaluate("document.readyState==='complete' && document.body?.innerText.length>0"),'Saved HTML preview did not render');
      const report=await capture.evaluate(`window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(label)}})`);
      const sheets=await preview.evaluate("Array.from(document.querySelectorAll('link[rel=stylesheet]')).map(link=>({url:link.href,loaded:!!link.sheet}))");
      for(const sheet of sheets){if(report.resources?.some(row=>row.url===sheet.url&&row.bytes!==null))assert.ok(sheet.loaded,'Captured stylesheet did not load');}
      await preview.call('Page.reload',{ignoreCache:true});
      await waitFor(()=>preview.evaluate("document.readyState==='complete' && document.body?.innerText.length>0"),'Saved HTML reload did not render');
      for(const sheet of sheets.filter(row=>row.loaded))assert.ok(await preview.evaluate(`Array.from(document.querySelectorAll('link[rel=stylesheet]')).some(link=>link.href===${JSON.stringify(sheet.url)}&&!!link.sheet)`),'Saved stylesheet did not survive reload');
      if(screenshot){const image=await preview.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-external-page.png'),Buffer.from(image.data,'base64'));}
      await closeNative('Captured page preview — Transmog');
      process.stdout.write('Saved HTML preview, captured stylesheets and reload verified.\n');
    }
  }
  if(pageSource){
    await second.evaluate(`${traffic}.importTrace(${JSON.stringify(pageSource)})`);
    await waitFor(()=>second.evaluate(`${traffic}.sessions.some(row=>row.path==='/captured-page') && !${traffic}.importingTrace`),'HTML fixture import did not finish');
    const navigation=await second.evaluate(`(()=>{const workspace=${traffic};return {pages:workspace.sessions.filter(row=>row.topLevelNavigation).map(row=>row.path),badges:workspace.getRootNode().querySelectorAll('.navigation-badge').length};})()`);
    assert.deepEqual(navigation,{pages:['/captured-page'],badges:1});
    if(screenshot){const image=await second.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-navigation.png'),Buffer.from(image.data,'base64'));}
    await second.evaluate(`(()=>{const root=${traffic}.getRootNode();root.querySelector('button[popovertarget="filter-fields"]').click();root.querySelector('button[popovertarget="fetch-destination-options"]').click();root.querySelector('#fetch-destination-options input[value="document"]').click();root.querySelector('#fetch-destination-options input[value="style"]').click();})()`);
    await waitFor(()=>second.evaluate(`${traffic}.sessions.length===2 && ${traffic}.sessions.every(row=>['document','style'].includes(row.fetchDestination))`),'Native multi-destination filter did not select document and stylesheet');
    assert.equal(await second.evaluate(`${traffic}.getRootNode().querySelectorAll('#fetch-destination-options input:checked').length`),2);
    if(screenshot){const image=await second.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-destinations.png'),Buffer.from(image.data,'base64'));}
    await second.evaluate(`(async()=>{const root=${traffic}.getRootNode();root.querySelector('#fetch-destination-options').hidePopover();root.querySelector('#filter-fields').hidePopover();await ${traffic}.clearFetchDestinations();})()`);
    await waitFor(()=>second.evaluate(`${traffic}.sessions.some(row=>row.path==='/captured-frame')`),'Clearing the native filter did not restore iframe traffic');
    const htmlId=await second.evaluate(`(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/captured-page');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`);
    await waitFor(()=>second.evaluate(`${traffic}.selectedDetail?.id===${JSON.stringify(htmlId)} && ${traffic}.previewPageAvailable`),'Selected HTML inspection did not finish');
    await second.evaluate(`${traffic}.showCapturedPage()`);
    assert.equal(await second.evaluate(`${traffic}.previewPageScripts.checked`),false);
    if(screenshot){const image=await second.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-page-warning.png'),Buffer.from(image.data,'base64'));}
    const old=(await targets()).map(target=>target.id);
    await second.evaluate(`${traffic}.openCapturedPage()`);
    await waitFor(()=>second.evaluate(`!${traffic}.previewPageBusy`),'Captured preview preparation did not finish');
    const disabledTarget=await waitFor(async()=>{const all=await targets();return all.find(target=>!old.includes(target.id)&&target.url.includes('/captured-page'));},'Captured preview did not navigate');
    const disabled=await connect(disabledTarget);
    await waitFor(()=>disabled.evaluate(`document.readyState==='complete' && document.getElementById('captured-heading')?.textContent==='Captured page fixture'`),'Captured HTML and resources did not finish rendering');
    const disabledState=await disabled.evaluate(`({script:globalThis.capturedScriptRan===true,color:getComputedStyle(document.getElementById('captured-heading')).color,image:document.getElementById('captured-image').naturalWidth})`);
    if(disabledState.color!=='rgb(0, 128, 0)')process.stderr.write(JSON.stringify(await second.evaluate(`window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${traffic}.previewReportLabel})`))+'\n');
    assert.equal(disabledState.script,false);assert.equal(disabledState.color,'rgb(0, 128, 0)');assert.equal(disabledState.image,20);
    assert.equal(await disabled.evaluate('navigator.userAgent'),'Placeholder/1');
    await disabled.call('Emulation.setUserAgentOverride',{userAgent:'Placeholder/2'});
    await disabled.call('Page.reload',{ignoreCache:true});
    await waitFor(()=>disabled.evaluate("document.readyState==='complete' && document.getElementById('captured-heading') && getComputedStyle(document.getElementById('captured-heading')).color==='rgb(0, 128, 0)'"),'Captured preview reload did not restore HTML and styles');
    assert.equal(await disabled.evaluate('navigator.userAgent'),'Placeholder/2');
    const oldEnabled=(await targets()).map(target=>target.id);
    const enabledLabel=await second.evaluate(`window.__TAURI_INTERNALS__.invoke('open_captured_page',{id:${JSON.stringify(htmlId)},enableScripts:true,options:{scope:'all-loaded'},operationId:'native-enabled-probe'})`);
    const enabledTarget=await waitFor(async()=>{const all=await targets();return all.find(target=>!oldEnabled.includes(target.id)&&target.url.includes('/captured-page'));},'Script-enabled preview did not navigate');
    const enabled=await connect(enabledTarget);
    await waitFor(()=>enabled.evaluate(`globalThis.missingResult`),'Captured script did not complete its missing request');
    assert.deepEqual(await enabled.evaluate(`globalThis.missingResult`),{status:404,body:''});
    await waitFor(()=>enabled.evaluate(`globalThis.variantResults`),'Captured POST variants did not finish');
    assert.deepEqual(await enabled.evaluate(`globalThis.variantResults`),[{status:200,body:'dark variant'},{status:200,body:'light variant'}]);
    const sequence=['first response','next response','next response'];
    await waitFor(()=>enabled.evaluate(`globalThis.sequenceResults`),'Repeated captured requests did not finish');
    assert.deepEqual(await enabled.evaluate(`globalThis.sequenceResults`),sequence);
    await enabled.evaluate(`globalThis.beforeSequenceReload=true`);
    await enabled.call('Page.reload',{ignoreCache:true});
    await waitFor(()=>enabled.evaluate(`!globalThis.beforeSequenceReload && globalThis.sequenceResults`),'Captured sequence did not restart on reload');
    assert.deepEqual(await enabled.evaluate(`globalThis.sequenceResults`),sequence);
    const report=await second.evaluate(`window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(enabledLabel)}})`);assert.ok(report.hits>=5&&report.misses>=1);assert.equal(report.scriptsEnabled,true);assert.equal(report.scope,'all-loaded');assert.ok(report.requests.some(row=>row.method==='POST'&&row.outcome==='served'));
    assert.equal(await main.evaluate(`window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(enabledLabel)}}).then(()=>false,()=>true)`),true,'Another window read a preview report');
    await second.evaluate(`(async()=>{const workspace=${traffic};workspace.previewReportLabel=${JSON.stringify(enabledLabel)};await workspace.showPreviewReport();})()`);
    if(screenshot){const image=await second.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-preview-report.png'),Buffer.from(image.data,'base64'));}
    await second.call('Emulation.setDeviceMetricsOverride',{width:760,height:520,deviceScaleFactor:1,mobile:false});assert.ok(await second.evaluate(`${traffic}.previewReportDialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight`));
    if(screenshot){const image=await second.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-preview-report-compact.png'),Buffer.from(image.data,'base64'));}
    await second.call('Emulation.clearDeviceMetricsOverride');await second.evaluate(`${traffic}.closePreviewReport()`);
    assert.equal(await enabled.evaluate(`(()=>{try{new RTCPeerConnection();return false;}catch{return true;}})()`),true);
    assert.equal(await enabled.evaluate(`(()=>{try{new WebSocket('ws://127.0.0.1:9');return false;}catch{return true;}})()`),true);
    const denied=await enabled.evaluate(`Promise.race([Promise.resolve().then(()=>window.__TAURI_INTERNALS__.invoke('app_status')).then(()=>false,()=>true),new Promise(resolve=>setTimeout(()=>resolve(true),500))])`);
    assert.equal(denied,true,'Captured content accessed application IPC');
    let contacted=0;const sink=createServer((_,response)=>{contacted++;response.end('unexpected');});await new Promise(resolve=>sink.listen(0,'127.0.0.1',resolve));
    try{const url='http://127.0.0.1:'+sink.address().port+'/uncaptured';await enabled.evaluate(`fetch(${JSON.stringify(url)},{mode:'no-cors'}).then(()=>true,()=>false)`);await new Promise(resolve=>setTimeout(resolve,100));assert.equal(contacted,0,'Preview contacted an uncaptured endpoint');}finally{await new Promise(resolve=>sink.close(resolve));}
    assert.equal(await main.evaluate(`window.__TAURI_INTERNALS__.invoke('app_status').then(status=>status.lifecycle)`),'stopped','Preview enabled the proxy');
    if(screenshot){const image=await enabled.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-page.png'),Buffer.from(image.data,'base64'));}
    await closeNative('Captured page preview — Transmog');
    await waitFor(async()=>(await targets()).filter(target=>[disabledTarget.id,enabledTarget.id].includes(target.id)).length===1,'First native preview did not close');
    await closeNative('Captured page preview — Transmog');
    await waitFor(async()=>!(await targets()).some(target=>[disabledTarget.id,enabledTarget.id].includes(target.id)),'Closed captured previews retained browser targets');
    if(profileRoot)await waitFor(async()=>{
      for await(const _ of glob('temp/transmog-captured-webview-*',{cwd:profileRoot}))return false;
      return true;
    },'Closed captured preview profiles were not cleaned up');
    process.stdout.write('Captured HTML, CSS across UAs, image, script choice, response sequence/reset, empty misses, IPC and uncaptured egress verified.\n');
  }
  const beforeReopen=(await targets()).map(target=>target.id);
  await closeNative('Transmog');
  await waitFor(async()=>!(await targets()).some(target=>target.id===mainTarget.id),'Main window did not close with saved viewers remaining');
  await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('open_main_window')`);
  const reopenedTarget=await waitFor(async()=>{const all=await targets();return all.find(target=>target.url.includes('transmog-ui')&&!beforeReopen.includes(target.id));},'Main window did not reopen from the viewer');
  const reopened=await connect(reopenedTarget);
  await waitFor(()=>reopened.evaluate(`${traffic}?.queryLoaded && ${traffic}.sessions.length===0`),'Reopened main window retained ephemeral traffic');
  assert.equal(await reopened.evaluate(`window.__TAURI_INTERNALS__.invoke('app_status').then(status=>status.lifecycle)`),'stopped');
  process.stdout.write(JSON.stringify({viewerLaunch:true,mainChoice:true,isolatedCatalogs:true,proxyPermissionDenied:true,additionalViewer:true,traceMetadata:true,nativeCapture:!!nativeSource})+'\n');
}finally{for(const socket of sockets)socket.close();}
