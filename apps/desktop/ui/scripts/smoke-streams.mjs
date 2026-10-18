import assert from 'node:assert/strict';
import {createServer, request} from 'node:http';
import {createHash} from 'node:crypto';
import {writeFile} from 'node:fs/promises';

// Real proxy traffic and native IPC; no fixture command overrides.
export async function smokeStreams(call, evaluate, screenshotPath) {
  const sockets=new Set();let events;
  const origin=createServer((incoming,response)=>{
    response.writeHead(200,{'Content-Type':'text/event-stream; charset=utf-8'});
    response.write('id: first\nevent: update\ndata: {"native":1}\n\n');events=response;
  });
  origin.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});
  origin.on('upgrade',(incoming,socket)=>{
    const accept=createHash('sha1').update(incoming.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: '+accept+'\r\n\r\n');
    socket.write(frame('native server',false));
  });
  await new Promise(resolve=>origin.listen(0,'127.0.0.1',resolve));
  const base='http://127.0.0.1:'+origin.address().port;
  const invoke=(name,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(name)},${JSON.stringify(args)})`);
  const wait=async(callback)=>{const deadline=Date.now()+15000;while(!await callback()){if(Date.now()>deadline)throw new Error('Native stream probe timed out');await new Promise(resolve=>setTimeout(resolve,50));}};
  let client,streamRequest,streamResponse;const saved=await invoke('product_state');
  try {
    const manual=structuredClone(saved);manual.preferences.configureSystemProxy=false;manual.preferences.decryptHttps=false;
    manual.privacy.retainRequestBodies=true;manual.privacy.retainResponseBodies=true;
    await invoke('save_product_state',{productState:manual});
    const bootstrap=await invoke('desktop_bootstrap');
    const status=await invoke('start_proxy',{request:{listen:'127.0.0.1:0',caCertificatePath:bootstrap.caCertificatePath,caPrivateKeyPath:bootstrap.caPrivateKeyPath}});
    const proxy=new URL('http://'+status.listener);
    const options=path=>({hostname:proxy.hostname,port:proxy.port,path:base+path,agent:false,headers:{Host:new URL(base).host}});
    const openSocket=()=>new Promise((resolve,reject)=>{
      const opts=options('/native-stream-socket');Object.assign(opts.headers,{Connection:'Upgrade',Upgrade:'websocket','Sec-WebSocket-Key':'dGhlIHNhbXBsZSBub25jZQ==','Sec-WebSocket-Version':'13'});
      const outgoing=request(opts);outgoing.on('upgrade',(_,socket)=>resolve(socket));outgoing.on('response',response=>reject(new Error('Upgrade rejected: '+response.statusCode)));outgoing.on('error',reject);outgoing.end();
    });
    client=await openSocket();
    client.on('error',()=>{});client.resume();client.write(frame('native client',true));
    streamResponse=await new Promise((resolve,reject)=>{streamRequest=request(options('/native-stream-events'),resolve);streamRequest.on('error',reject);streamRequest.end();});
    streamResponse.on('error',()=>{});streamResponse.resume();
    const active=async()=> (await invoke('query_sessions',{query:{filters:[{column:'state',operator:'contains',value:'active'}]}})).sessions.filter(row=>row.path.startsWith('/native-stream-'));
    await wait(async()=> (await active()).length===2);
    const rows=await active(),socketId=rows.find(row=>row.path.endsWith('socket')).id,sseId=rows.find(row=>row.path.endsWith('events')).id;
    assert.equal(rows.find(row=>row.id===socketId).streamProtocol,'web-socket');
    assert.equal(rows.find(row=>row.id===sseId).streamProtocol,'sse');
    const page=id=>invoke('stream_page',{query:{sessionId:id,offset:0,limit:100,tail:true,anchor:null,sender:null,controls:true,search:''}});
    await wait(async()=> (await page(socketId)).rows.length===2 && (await page(sseId)).rows.length===1);
    const facet=await invoke('traffic_facets',{request:{query:{filterGroups:[{dimension:'state',values:[{kind:'text',value:'active'}]}]},dimension:'traffic-type',search:null,offset:0,limit:100}});
    assert.ok(facet.options.some(option=>option.key==='text:websocket'&&option.count>=1));assert.ok(facet.options.some(option=>option.key==='text:sse'&&option.count>=1));
    const ws=await page(socketId);assert.deepEqual(ws.rows.map(row=>row.message.sender).sort(),['client','server']);
    const sse=await page(sseId),payload=await invoke('stream_message',{id:sseId,sequence:sse.rows[0].sequence});
    assert.equal(Buffer.from(payload.payload).toString(),'{"native":1}');assert.equal(payload.eventId,'first');
    await evaluate(`(async()=>{const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="traffic"]').click();await shell.traffic.revealSession(${JSON.stringify(socketId)});})()`);
    await wait(()=>evaluate(`document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamRows.length===2`));
    const badges=()=>evaluate(`Array.from(document.querySelector('app-shell').traffic.querySelectorAll('.stream-identity[data-active]'),el=>el.textContent)`);
    await wait(async()=> (await badges()).includes('WebSocket⌄') && (await badges()).includes('SSE⌄'));
    const byteRows=await invoke('query_sessions',{query:{search:'/native-stream-socket',sort:{column:'started-at',direction:'descending'}}});
    assert.equal(byteRows.sessions[0].streamSent,13);assert.equal(byteRows.sessions[0].streamReceived,13);assert.equal(byteRows.sessions[0].requestBytes,0);assert.equal(byteRows.sessions[0].responseBytes,0);
    const quietDuration=await evaluate(`document.querySelector('app-shell').traffic.rowCache.get(${JSON.stringify(socketId)}).durationMs`);
    await wait(()=>evaluate(`document.querySelector('app-shell').traffic.rowCache.get(${JSON.stringify(socketId)})?.durationMs>${quietDuration+500}`));
    const quietSettings=await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic;return {filters:t.filters,groups:t.filterGroups,search:t.liveFilter,text:t.searchText,follow:t.followLatest};})()`);
    try {
      await evaluate(`(async()=>{const t=document.querySelector('app-shell').traffic;t.liveFilter='/native-stream-socket';t.filterGroups=[{dimension:'traffic-type',values:[{kind:'text',value:'websocket'}]}];t.filters=[{column:'duration',operator:'minimum',value:String(t.rowCache.get(${JSON.stringify(socketId)}).durationMs+1800),label:''}];t.followLatest=true;t.queryRevision++;await t.refreshSessions(undefined,true);})()`);
      assert.equal(await evaluate(`document.querySelector('app-shell').traffic.virtualList.ids.includes(${JSON.stringify(socketId)})`),false);
      await wait(()=>evaluate(`document.querySelector('app-shell').traffic.virtualList.ids.includes(${JSON.stringify(socketId)})`));
      assert.equal(await evaluate(`document.querySelector('app-shell').traffic.selectedSessionId`),socketId);
    } finally {
      await evaluate(`(async()=>{const t=document.querySelector('app-shell').traffic,s=${JSON.stringify(quietSettings)};t.filters=s.filters;t.filterGroups=s.groups;t.liveFilter=s.search;t.searchText=s.text;t.followLatest=s.follow;t.queryRevision++;await t.refreshSessions(undefined,true);})()`);
    }
    await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic;t.querySelector('.stream-identity').click();t.streamDisclosure.querySelector('button:last-child').click();})()`);

    await evaluate(`(()=>{const inspector=document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector');inspector.select(inspector.streamRows[0]);})()`);
    await wait(()=>evaluate(`!!document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload`));
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-streams.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    const originalPreferences=await evaluate(`document.querySelector('app-shell').traffic.preferences`);
    const rowHeight=await evaluate(`document.querySelector('app-shell').traffic.querySelector('tr[data-session-id]').getBoundingClientRect().height`);
    try {
      await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,columns:t.preferences.columns.map(column=>column.id==='protocol'?{...column,width:72}:column)};})()`);
      await wait(()=>evaluate(`Array.from(document.querySelector('app-shell').traffic.querySelectorAll('.stream-identity')).some(el=>el.textContent==='WS⌄')`));
      assert.equal(await evaluate(`Array.from(document.querySelector('app-shell').traffic.querySelectorAll('.stream-identity')).every(el=>{const range=document.createRange();range.selectNodeContents(el);const cell=el.closest('td').getBoundingClientRect();return [...range.getClientRects()].every(rect=>rect.left>=cell.left&&rect.right<=cell.right);})`),true,'Compact stream identity lost its label or disclosure chevron');
      await evaluate(`document.querySelector('app-shell').traffic.querySelector('.stream-identity').scrollIntoView({block:'nearest',inline:'nearest'})`);
      if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-stream-compact-identity.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
      for(const width of [100,60,32]) {
        await evaluate(`(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,wrapCells:true,columns:t.preferences.columns.map(column=>column.id==='protocol'||column.id==='path'?{...column,width:${width}}:column)};})()`);
        await wait(()=>evaluate(`(()=>{
          const badges=Array.from(document.querySelector('app-shell').traffic.querySelectorAll('.stream-identity[data-active]'));
          return badges.length===2&&badges.every(badge=>{
            const cell=badge.closest('td'),text=document.createRange();text.selectNodeContents(badge);
            return cell.dataset.columnId==='protocol'&&Math.abs(cell.getBoundingClientRect().width-${width})<1&&getComputedStyle(cell).whiteSpace==='nowrap'&&new Set([...text.getClientRects()].map(rect=>Math.round(rect.top))).size===1&&Math.abs(cell.closest('tr').getBoundingClientRect().height-${rowHeight})<1&&cell.scrollWidth>cell.clientWidth&&cell.title.includes('In progress');
          });
        })()`));
        if(width===60&&screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-stream-protocol-narrow.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
      }
    } finally {
      await evaluate(`document.querySelector('app-shell').traffic.preferences=${JSON.stringify(originalPreferences)}`);
    }
    await evaluate(`document.querySelector('app-shell').traffic.revealSession(${JSON.stringify(sseId)})`);
    await wait(()=>evaluate(`document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamRows[0]?.message.kind==='event'`));
    await evaluate(`(()=>{const inspector=document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector');inspector.select(inspector.streamRows[0]);})()`);
    await wait(()=>evaluate(`document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload?.eventId==='first'`));
    assert.ok(await evaluate(`(()=>{const inspector=document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector'),select=inspector.querySelector('[aria-label="Message payload viewer"]');if(select.querySelector('[value="raw"]').disabled)return false;select.value='raw';select.dispatchEvent(new Event('change',{bubbles:true}));return inspector.streamText.includes('data: {"native":1}');})()`));
    if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-sse.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
    const draining=await invoke('stop_application');assert.equal(draining.lifecycle,'draining');
    assert.equal((await active()).length,2,'Quiet streams disappeared from Active during Stop');
    const resumed=await invoke('resume_application');assert.equal(resumed.lifecycle,'running');
    client.write(frame('after restart',true));events.write(': heartbeat\n\ndata: second\n\n');
    await wait(async()=> (await page(socketId)).rows.length===3 && (await page(sseId)).rows.length===3);
    // Closing the real peers must remove badges through the observed lifecycle.
    events.end();
    await wait(async()=> !(await active()).some(row=>row.id===sseId));
    await evaluate(`document.querySelector('app-shell').traffic.refreshSessions(undefined,true)`);
    await wait(async()=> !(await badges()).includes('SSE⌄'));
    assert.ok((await badges()).includes('WebSocket⌄'),'Quiet WebSocket lost its active badge');
    client.destroy();
    await wait(async()=> (await active()).length===0);
    await evaluate(`document.querySelector('app-shell').traffic.refreshSessions(undefined,true)`);
    await wait(async()=> (await badges()).length===0);
    assert.deepEqual(await badges(),[],'Ended native streams still show active badges');
    assert.equal(await evaluate(`document.querySelector('app-shell').traffic.querySelectorAll('.stream-identity').length`),2,'Closed native streams lost their identity');
    const ended=await invoke('query_sessions',{query:{search:'/native-stream-events',sort:{column:'started-at',direction:'descending'}}});const lifetime=ended.sessions[0].durationMs;await new Promise(resolve=>setTimeout(resolve,100));const endedAgain=await invoke('query_sessions',{query:{search:'/native-stream-events',sort:{column:'started-at',direction:'descending'}}});assert.equal(endedAgain.sessions[0].durationMs,lifetime,'Closed SSE lifetime kept aging');
    // Application exit must cancel a new quiet socket instead of waiting for a peer.
    client=await openSocket();client.on('error',()=>{});client.resume();
    await wait(async()=> (await active()).length===1);
    await invoke('prepare_update_handoff');
    assert.equal((await invoke('app_status')).lifecycle,'stopped');

    return {liveWebSocket:true,liveSse:true,quietActive:true,stopResume:true,boundedExit:true};
  } finally {
    client?.destroy();streamResponse?.destroy();streamRequest?.destroy();
    for(const socket of sockets)socket.destroy();
    await new Promise(resolve=>origin.close(resolve));
    try {await invoke('prepare_update_handoff');} finally {await invoke('save_product_state',{productState:saved});}
    const rows=await invoke('query_sessions',{query:{}}),ids=rows.sessions.filter(row=>row.path.startsWith('/native-stream-')).map(row=>row.id);
    if(ids.length)await invoke('remove_traffic_entries',{ids,restore:false});
  }
}
function frame(text,masked) {
  const bytes=Buffer.from(text),mask=Buffer.from([1,2,3,4]);
  assert.ok(bytes.length<126);
  return masked?Buffer.concat([Buffer.from([0x81,0x80|bytes.length]),mask,Buffer.from(bytes.map((value,index)=>value^mask[index%4]))]):Buffer.concat([Buffer.from([0x81,bytes.length]),bytes]);
}
