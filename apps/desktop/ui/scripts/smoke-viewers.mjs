import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {spawn} from 'node:child_process';

const argument=name=>process.argv[process.argv.indexOf(name)+1];
const port=argument('--port'), source=argument('--source'), executable=argument('--executable'), screenshot=argument('--screenshot');
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
const targets=()=>fetch(`http://127.0.0.1:${port}/json/list`).then(response=>response.json()).then(targets=>targets.filter(target=>target.type==='page'));
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
  const before=(await targets()).map(target=>target.id);
  await main.evaluate(`window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(source)}]})`);
  const additional=await waitFor(async()=>{const all=await targets();return all.find(target=>!before.includes(target.id));},'Additional viewer did not open');
  const second=await connect(additional);
  await waitFor(()=>second.evaluate(`${root}?.viewerMode && ${traffic}?.sessions.length===1 && !${traffic}.importingTrace`),'Additional viewer did not load its saved capture');
  process.stdout.write(JSON.stringify({viewerLaunch:true,mainChoice:true,isolatedCatalogs:true,proxyPermissionDenied:true,additionalViewer:true,traceMetadata:true})+'\n');
}finally{for(const socket of sockets)socket.close();}
