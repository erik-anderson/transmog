import assert from 'node:assert/strict';
import {createServer,request} from 'node:http';
import {gzipSync} from 'node:zlib';
import {writeFile} from 'node:fs/promises';

export async function smokeHttpVisualizers(call,evaluate,screenshotPath) {
  const text='Native HTTP decoded evidence',encoded=gzipSync(text),paths=['/native-http-extension','/native-http-malformed'];
  const sockets=new Set(),origin=createServer((incoming,response)=>{response.writeHead(200,{'Content-Type':'application/example','Content-Encoding':'gzip'});response.end(incoming.url===paths[1]?Buffer.from('not gzip'):encoded);});
  origin.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});
  await new Promise(resolve=>origin.listen(0,'127.0.0.1',resolve));
  const invoke=(command,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)}).catch(error=>{throw new Error(${JSON.stringify(command+': ')}+JSON.stringify(error));})`);
  const wait=async(condition,message)=>{const deadline=Date.now()+20000;while(!await condition()){if(Date.now()>deadline)throw new Error(message);await new Promise(resolve=>setTimeout(resolve,30));}};
  const saved=await invoke('product_state');let started=false,problem;
  const viewer='globalThis.__nativeHttpViewer';
  try {
    const product=structuredClone(saved);product.preferences.configureSystemProxy=false;product.preferences.decryptHttps=false;product.preferences.allowRemoteClients=false;
    product.privacy.retainRequestBodies=true;product.privacy.retainResponseBodies=true;await invoke('save_product_state',{productState:product});
    const bootstrap=await invoke('desktop_bootstrap');
    const status=await invoke('start_proxy',{request:{listen:'127.0.0.1:0',caCertificatePath:bootstrap.caCertificatePath,caPrivateKeyPath:bootstrap.caPrivateKeyPath}});started=true;
    const proxy=new URL('http://'+status.listener),address='127.0.0.1:'+origin.address().port;
    const send=path=>new Promise((resolve,reject)=>{const outgoing=request({hostname:proxy.hostname,port:proxy.port,path:'http://'+address+path,agent:false,headers:{Host:address}},response=>{const chunks=[];response.on('data',chunk=>chunks.push(chunk));response.on('end',()=>resolve(Buffer.concat(chunks)));response.on('error',reject);});outgoing.on('error',reject);outgoing.setTimeout(10000,()=>outgoing.destroy(new Error('HTTP fixture timed out')));outgoing.end();});
    const open=async path=>{
      let row;await wait(async()=>{row=(await invoke('query_sessions',{query:{search:path,limit:100,offset:0}})).sessions.find(row=>row.path===path&&row.responseBytes>0);return !!row;},'Encoded body was not retained');
      await evaluate(`(async()=>{const shell=document.querySelector('app-shell'),traffic=shell.traffic;await shell.refreshStatus();shell.shadowRoot.querySelector('a[data-view="traffic"]').click();await traffic.refreshSessions(undefined,true);await traffic.revealSession(${JSON.stringify(row.id)});})()`);
      await wait(()=>evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,r=(t.shadowRoot??t).querySelector('message-inspector[side=response]');return r.detail?.id===${JSON.stringify(row.id)}&&r.bodyChoices.some(choice=>choice.visualizerId==='native-http');})()`),'Declared header match did not offer HTTP visualizer');
      await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,r=(t.shadowRoot??t).querySelector('message-inspector[side=response]');r.showMode('body');const select=(r.shadowRoot??r).querySelector('select[aria-label="Body viewer"]');select.value=[...select.options].find(option=>option.textContent==='Native HTTP protocol').value;select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
      await wait(()=>evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,r=(t.shadowRoot??t).querySelector('message-inspector[side=response]');globalThis.__nativeHttpViewer=(r.shadowRoot??r).querySelector('extension-visualizer[slot$="body"]');return !!${viewer}?.view||!!${viewer}?.visualizerError;})()`),'HTTP decode handshake did not settle');
      assert.equal(await evaluate(`${viewer}.visualizerError`),'');
    };
    assert.deepEqual(await send(paths[0]),encoded);
    await open(paths[0]);
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),text);
    const result=await evaluate(`(()=>{const v=${viewer},source=v.view.sources.find(source=>source.httpDecoded);if(!source)throw new Error('Decoded provenance missing');v.selectNode('body-text');v.showSpan(v.view.document.roots[0].spans[0]);return {handle:v.view.handle,source};})()`);
    const decoded=await invoke('visualizer_source',{handle:result.handle,request:{sourceId:result.source.id,offset:'0',maxBytes:8}});
    assert.equal(Buffer.from(decoded.bytesBase64,'base64').toString(),'Native H');
    const raw=await invoke('visualizer_source',{handle:result.handle,request:{sourceId:result.source.httpDecoded.parent.sourceId,offset:'0',maxBytes:2}});
    assert.deepEqual(Buffer.from(raw.bytesBase64,'base64'),Buffer.from([0x1f,0x8b]));
    assert.equal(result.source.httpDecoded.parent.length,String(encoded.length));
    assert.equal(await evaluate(`${viewer}.visualizerSourceText.includes('enclosing parent regions only')&&${viewer}.visualizerSourceText.includes('Parents:')`),true);
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-visualizer-http.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await send(paths[1]);await open(paths[1]);
    assert.equal(await invoke('visualizer_active',{handle:result.handle}),false);
    assert.equal(await evaluate(`${viewer}.view.document.status`),'partial');
    assert.equal(await evaluate(`${viewer}.view.document.diagnostics[0].code`),'malformed');
    assert.equal(await evaluate(`${viewer}.view.sources.some(source=>source.httpDecoded)`),false,'Malformed data was labeled decoded');
    const handle=await evaluate(`${viewer}.view.handle`);
    await evaluate(`document.querySelector('app-shell').shadowRoot.querySelector('a[data-view="settings"]').click()`);
    await wait(async()=>!(await invoke('visualizer_active',{handle})),'HTTP visualizer did not unload');
    return {realProxy:true,declaredHeaders:true,decodedBytes:true,encodedPreserved:true,enclosingProvenance:true,malformedExplicit:true,unloaded:true};
  } catch(error) {problem=error;throw error;} finally {
    for(const socket of sockets)socket.destroy();await new Promise(resolve=>origin.close(resolve));
    const failures=[];
    for(const cleanup of [
      async()=>{if(started)await invoke('stop_application');},
      ()=>invoke('save_product_state',{productState:saved}),
      async()=>{const rows=await invoke('query_sessions',{query:{search:'/native-http-',limit:100,offset:0}}),ids=rows.sessions.filter(row=>paths.includes(row.path)).map(row=>row.id);if(ids.length)await invoke('remove_traffic_entries',{ids,restore:false});},
      ()=>evaluate(`(async()=>{const shell=document.querySelector('app-shell');await shell.refreshStatus();shell.shadowRoot.querySelector('a[data-view="settings"]').click();delete globalThis.__nativeHttpViewer;})()`),
    ]) {try {await cleanup();}catch(error){failures.push(error);}}
    if(failures.length){if(!problem)throw new AggregateError(failures,'Native HTTP fixture cleanup failed');process.stderr.write('Native HTTP cleanup: '+failures.map(error=>error.message).join('; ')+'\n');}
  }
}
