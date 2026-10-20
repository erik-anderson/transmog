import assert from 'node:assert/strict';
import {Agent,createServer,request} from 'node:http';
import {writeFile} from 'node:fs/promises';

// Owned loopback servers feed the real proxy. Extension code runs in the regular
// network-denied AppContainer and can only consume the broker's retained input.
export async function smokeLiveVisualizers(call,evaluate,screenshotPath) {
  let body='Native live first',contentType='text/plain';
  const serve=(_incoming,response)=>{response.writeHead(200,{'Content-Type':contentType});response.end(body);};
  const origins=[createServer(serve),createServer(serve)],sockets=new Set();
  for(const origin of origins){origin.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});await new Promise(resolve=>origin.listen(0,'127.0.0.1',resolve));}
  const agent=new Agent({keepAlive:true,maxSockets:1});
  const invoke=(command,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)}).catch(error=>{throw new Error(${JSON.stringify(command+': ')}+JSON.stringify(error));})`);
  const wait=async(condition,message)=>{const deadline=Date.now()+20000;while(!await condition()){if(Date.now()>deadline)throw new Error(message);await new Promise(resolve=>setTimeout(resolve,40));}};
  const saved=await invoke('product_state');let started=false,problem;
  const viewer='globalThis.__nativeLiveViewer';
  try {
    const product=structuredClone(saved);product.preferences.configureSystemProxy=false;product.preferences.decryptHttps=false;product.preferences.allowRemoteClients=false;
    product.privacy.retainRequestBodies=true;product.privacy.retainResponseBodies=true;
    await invoke('save_product_state',{productState:product});
    const bootstrap=await invoke('desktop_bootstrap');
    const status=await invoke('start_proxy',{request:{listen:'127.0.0.1:0',caCertificatePath:bootstrap.caCertificatePath,caPrivateKeyPath:bootstrap.caPrivateKeyPath}});started=true;
    const proxy=new URL('http://'+status.listener),path='/native-live-extension';
    const send=(index=0)=>new Promise((resolve,reject)=>{
      const address='127.0.0.1:'+origins[index].address().port;
      const outgoing=request({hostname:proxy.hostname,port:proxy.port,path:'http://'+address+path,agent,headers:{Host:address}},response=>{let received='';response.setEncoding('utf8');response.on('data',chunk=>received+=chunk);response.on('end',()=>resolve(received));response.on('error',reject);});
      outgoing.setTimeout(10000,()=>outgoing.destroy(new Error('Native live fixture timed out')));outgoing.on('error',reject);outgoing.end();
    });
    assert.equal(await send(),'Native live first');
    let row;
    await wait(async()=>{row=(await invoke('query_sessions',{query:{search:path,limit:100,offset:0}})).sessions.find(row=>row.path===path&&row.responseBytes===17);return !!row;},'Live anchor was not retained');
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),traffic=shell.traffic;globalThis.__nativeLivePreferences=traffic.preferences;
      await shell.refreshStatus();
      shell.shadowRoot.querySelector('a[data-view="traffic"]').click();traffic.preferences={...traffic.preferences,layout:'stacked',listSplit:30};
      await traffic.refreshSessions(undefined,true);await traffic.revealSession(${JSON.stringify(row.id)});
    })()`);
    await wait(()=>evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,response=(t.shadowRoot??t).querySelector('message-inspector[side=response]');return response.detail?.id===${JSON.stringify(row.id)}&&response.bodyChoices.some(choice=>choice.visualizerId==='native-live');})()`),'Live contribution was not offered');
    await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,response=(t.shadowRoot??t).querySelector('message-inspector[side=response]');response.showMode('body');const select=(response.shadowRoot??response).querySelector('select[aria-label="Body viewer"]');select.value=[...select.options].find(option=>option.textContent==='Native live protocol').value;select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await wait(()=>evaluate(`(()=>{const t=document.querySelector('app-shell').traffic,response=(t.shadowRoot??t).querySelector('message-inspector[side=response]');globalThis.__nativeLiveViewer=(response.shadowRoot??response).querySelector('extension-visualizer[slot$="body"]');return !!${viewer}?.view||!!${viewer}?.visualizerError;})()`),'Live snapshot did not initialize');
    assert.equal(await evaluate(`${viewer}.visualizerError`),'');
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),'Native live first');
    assert.equal(await evaluate(`${viewer}.view.live.available`),true);
    assert.equal(await evaluate(`${viewer}.view.live.enabled`),false);
    const initial=await evaluate(`${viewer}.view.handle`);
    await evaluate(`(()=>{const v=${viewer};(v.shadowRoot??v).querySelector('input[type=checkbox]').click();})()`);
    await wait(()=>evaluate(`!!${viewer}.view?.live.enabled||!!${viewer}.visualizerError`),'Live enable did not settle');
    assert.equal(await evaluate(`${viewer}.visualizerError`),'');
    const active=await evaluate(`${viewer}.view.handle`);
    assert.notEqual(active.instanceId,initial.instanceId,'Live On reused parser heap');
    assert.equal(await invoke('visualizer_active',{handle:initial}),false);
    body='Native wrong port';assert.equal(await send(1),body);
    await new Promise(resolve=>setTimeout(resolve,500));
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),'Native live first','Another origin reached the active feed');
    body='Native live second';assert.equal(await send(),body);
    await wait(()=>evaluate(`${viewer}.view?.document.roots[0]?.value?.value==='Native live second'||!!${viewer}.visualizerError`),'Admitted live response did not reach sandbox');
    assert.equal(await evaluate(`${viewer}.visualizerError`),'');
    assert.equal(await evaluate(`(${viewer}.shadowRoot??${viewer}).textContent.includes('Native live second')`),true,'The host did not render live parser output');
    contentType='application/octet-stream';body='Native wrong content type';assert.equal(await send(),body);
    await new Promise(resolve=>setTimeout(resolve,500));
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),'Native live second','An unrelated content type reached the active feed');
    await call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:1,mobile:false});
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-visualizer-live.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    await evaluate(`(()=>{const v=${viewer};(v.shadowRoot??v).querySelector('input[type=checkbox]').click();})()`);
    await wait(()=>evaluate(`!${viewer}.view?.live.enabled`),'Live Off did not remove admission');
    contentType='text/plain';body='Native live stopped';assert.equal(await send(),body);
    await new Promise(resolve=>setTimeout(resolve,500));
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),'Native live second','Live Off continued parsing');
    assert.equal(await invoke('visualizer_active',{handle:active}),true,'Live Off discarded the visible worker');
    await evaluate(`(()=>{const v=${viewer};(v.shadowRoot??v).querySelector('input[type=checkbox]').click();})()`);
    await wait(()=>evaluate(`!!${viewer}.view?.live.enabled`),'Fresh live instance did not start');
    const finalHandle=await evaluate(`${viewer}.view.handle`);
    body='Native final response';assert.equal(await send(),body);
    await wait(()=>evaluate(`${viewer}.view?.document.updates==='final'&&!${viewer}.view.live.available`),'Final result did not stop native admission');
    assert.equal(await evaluate(`!!(${viewer}.shadowRoot??${viewer}).querySelector('.visualizer-live')`),false,'Final result still offered Live updates');
    assert.equal((await invoke('poll_visualizer_live',{handle:finalHandle})).view,null);
    body='Native after final';assert.equal(await send(),body);
    await new Promise(resolve=>setTimeout(resolve,500));
    const finalPoll=await invoke('poll_visualizer_live',{handle:finalHandle});
    assert.equal(finalPoll.view,null,'A final view continued delivering traffic');
    assert.equal(finalPoll.status.available,false);
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].value.value`),'Native final response');
    await evaluate(`${viewer}.toggleNode('body-text')`);
    assert.equal(await evaluate(`${viewer}.view.document.roots[0].children[0].value.value`),'9007199254740993','Finalization revoked deferred expansion');
    assert.equal(await evaluate(`${viewer}.view.live.available`),false,'Expansion revived a final feed');
    const finalSource=await evaluate(`${viewer}.view.document.roots[0].spans[0].sourceId`);
    const finalBytes=await invoke('visualizer_source',{handle:finalHandle,request:{sourceId:finalSource,offset:'0',maxBytes:5}});
    assert.equal(Buffer.from(finalBytes.bytesBase64,'base64').toString(),'Nativ');
    await evaluate(`document.querySelector('app-shell').shadowRoot.querySelector('a[data-view="settings"]').click()`);
    await wait(async()=>!(await invoke('visualizer_active',{handle:finalHandle})),'Inactive final viewer was not reaped');
    assert.equal(await evaluate(`!!${viewer}.view||!!${viewer}.visualizerHex||${viewer}.visualizerRows.length>0`),false);
    return {realProxy:true,defaultOff:true,freshWorker:true,bodyContinuation:true,sameOrigin:true,contentTypeFiltered:true,stopped:true,finalized:true,finalExpansion:true,finalSourceReads:true,unloaded:true};
  } catch(error) {problem=error;throw error;} finally {
    agent.destroy();for(const socket of sockets)socket.destroy();for(const origin of origins)await new Promise(resolve=>origin.close(resolve));
    const failures=[];
    for(const cleanup of [
      async()=>{if(started)await invoke('stop_application');},
      ()=>invoke('save_product_state',{productState:saved}),
      async()=>{const rows=await invoke('query_sessions',{query:{search:'/native-live-extension',limit:100,offset:0}}),ids=rows.sessions.filter(row=>row.path==='/native-live-extension').map(row=>row.id);if(ids.length)await invoke('remove_traffic_entries',{ids,restore:false});},
      ()=>evaluate(`(async()=>{const shell=document.querySelector('app-shell'),t=shell.traffic;await shell.refreshStatus();shell.shadowRoot.querySelector('a[data-view="settings"]').click();if(globalThis.__nativeLivePreferences)t.preferences=globalThis.__nativeLivePreferences;delete globalThis.__nativeLivePreferences;delete globalThis.__nativeLiveViewer;})()`),
      ()=>call('Emulation.clearDeviceMetricsOverride'),
    ]) {try {await cleanup();}catch(error){failures.push(error);}}
    if(failures.length){if(!problem)throw new AggregateError(failures,'Native live fixture cleanup failed');process.stderr.write('Native live cleanup: '+failures.map(error=>error.message).join('; ')+'\n');}
  }
}
