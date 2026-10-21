import assert from 'node:assert/strict';
import { resolve } from 'node:path';
export async function checkStreams(page,root) {
  const saved=await page.evaluate(()=>({preferences:document.querySelector('app-shell').traffic.preferences,sessions:globalThis.__workspaceFixture.sessions,evictedIds:globalThis.__workspaceFixture.evictedIds,dismissedIds:globalThis.__workspaceFixture.dismissedIds}));
  await page.setViewportSize({width:1440,height:900});
  await page.evaluate(async()=>{
    const shell=document.querySelector('app-shell'),workspace=shell.traffic,state=globalThis.__workspaceFixture;
    const summary={state:{protocol:'web-socket',status:'open',atMillis:1000,reason:null},imported:false,revision:1200,observed:1200,evicted:5,sequenceLoss:0,retained:1200,clientBytes:600,serverBytes:20000,openedAt:1000,lastActivity:2200};
    state.streams={socket:summary};state.streamItems=Array.from({length:1200},(_,i)=>({sequence:i+1,message:{atMillis:1000+i,sender:i%2?'server':'client',kind:i%10===0?'ping':'text',payload:[...new TextEncoder().encode(JSON.stringify({index:i,body:'<img src=x onerror=alert(1)>'}))],byteCount:40,truncated:false,compressed:i%2===0,event:null,eventId:null,retry:null,raw:null,closeCode:null}}));
    state.sessions=[{id:'socket',caller:{kind:'local-process',processName:'Browser',processId:42},method:'GET',host:'example.test',path:'/socket',url:'https://example.test/socket',contentType:'',protocol:'HTTP/1.1',streamProtocol:'web-socket',status:101,durationMs:3,requestBytes:0,responseBytes:0,terminal:'active',loss:false,capturing:false,autoResponse:null}];
    state.evictedIds=[];state.dismissedIds=[];workspace.preferences={...workspace.preferences,columns:workspace.preferences.columns.map(column=>column.id==='protocol'?{...column,visible:true,width:210}:column)};await workspace.revealSession('socket');
  });
  const inspector=page.locator('stream-inspector');await inspector.locator('.stream-row').last().waitFor();
  const socketRow=page.locator('#session-socket');
  await socketRow.locator('.stream-identity').waitFor();
  assert.equal(await socketRow.locator('.stream-identity').textContent(),'WebSocket⌄');
  assert.match(await socketRow.getAttribute('aria-label'),/WebSocket · In progress/);
  assert.equal(await socketRow.locator('[data-column-id="path"] .stream-identity').count(),0);
  assert.equal(await socketRow.locator('[data-column-id="protocol"] .stream-identity').textContent(),'WebSocket⌄');
  assert.equal(await page.getByLabel('Wrap cells',{exact:true}).count(),0);
  assert.ok(await inspector.locator('.stream-row').count()<=100);
  assert.match(await inspector.locator('.stream-status').textContent(),/WebSocket.*open.*1,200/);
  await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.applyDetail({...t.selectedDetail,stream:{...t.selectedDetail.stream,clientBytes:null,serverBytes:0}});});assert.match(await inspector.locator('.stream-status').textContent(),/Sent Unavailable · Received 0 B/);
  await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.applyDetail({...t.selectedDetail,stream:{...t.selectedDetail.stream,clientBytes:600,serverBytes:20000}});});
  assert.match(await inspector.locator('.stream-notice').first().textContent(),/5 older/);
  await inspector.locator('.stream-row').last().click();await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload!==null);await inspector.locator('pre').filter({hasText:/"index":1199/}).waitFor();
  assert.equal(await inspector.locator('.stream-payload img').count(),0);
  await inspector.getByLabel('Message payload viewer').selectOption('json');
  assert.match(await inspector.locator('pre').textContent(),/"index": 1199/);
  assert.equal(await inspector.locator('[aria-selected="true"]').count(),1);
  const selectedMessage=await inspector.evaluate(element=>element.streamSelected);
  const otherMessage=inspector.locator('.stream-row').first();
  await otherMessage.click({button:'right'});
  await page.getByRole('menu',{name:'Copy inspection value',exact:true}).getByRole('menuitem',{name:'Copy value',exact:true}).click();
  assert.equal(await inspector.evaluate(element=>element.streamSelected),selectedMessage,'Copying another retained message changed selection');
  assert.match(await page.evaluate(()=>navigator.clipboard.readText()),/"index":/);
  await inspector.getByRole('button',{name:'Copy text',exact:true}).click();
  assert.match(await page.evaluate(()=>navigator.clipboard.readText()),/"index": 1199/);
  await inspector.getByRole('button',{name:'Save retained message…',exact:true}).click();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.savedStream.sequence),1200);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-wide.png')});
  await page.setViewportSize({width:800,height:600});await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-compact.png')});
  assert.ok((await inspector.getByRole('button',{name:'Save retained message…'}).boundingBox()).y<600);
  assert.equal(await inspector.locator('.stream-tools').isVisible(),false);
  await inspector.locator('.stream-payload pre').scrollIntoViewIfNeeded();
  const payloadBox=await inspector.locator('.stream-payload').boundingBox();assert.ok(payloadBox.height>30&&payloadBox.y<600);
  await inspector.getByRole('button',{name:'Messages',exact:true}).scrollIntoViewIfNeeded();
  await inspector.getByRole('button',{name:'Messages',exact:true}).click();assert.equal(await inspector.locator('.stream-list').isVisible(),true);
  await inspector.locator('.stream-row').last().click();await page.waitForFunction(()=>!!document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload);await inspector.getByLabel('Message payload viewer').selectOption('json');
  // Updates preserve the selected message and text; scrolling away pauses following.
  await page.evaluate(()=>{const workspace=document.querySelector('app-shell').traffic;workspace.applyDetail({...workspace.selectedDetail,stream:{...workspace.selectedDetail.stream,revision:1300,observed:1300}});});
  assert.match(await inspector.locator('pre').textContent(),/"index": 1199/);
  await page.setViewportSize({width:1440,height:1000});
  await inspector.getByLabel('Message sender').selectOption('client');await page.waitForFunction(()=>[...document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamRows].every(row=>row.message.sender==='client'));
  await inspector.locator('.stream-row').first().focus();await page.keyboard.press('Home');await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamSelected===1);
  await page.keyboard.press('End');await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamSelected===1199);
  // Late payload fetches cannot replace the newly selected message.
  await page.evaluate(()=>globalThis.__workspaceFixture.deferStream=true);await inspector.locator('.stream-row').first().click();
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.releaseStream);
  await page.evaluate(()=>globalThis.__workspaceFixture.deferStream=false);await inspector.locator('.stream-row').nth(1).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload!==null);
  const selectedText=await inspector.locator('pre').textContent();await page.evaluate(()=>globalThis.__workspaceFixture.releaseStream());
  assert.equal(await inspector.locator('pre').textContent(),selectedText);
  await page.getByRole('button',{name:'HTTP handshake',exact:true}).click();assert.equal(await inspector.isVisible(),false);
  await page.getByRole('button',{name:'Messages / Events',exact:true}).click();assert.equal(await inspector.isVisible(),true);
  await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-contrast.png')});await page.emulateMedia({forcedColors:'none'});
  await page.evaluate(async()=>{
    const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;
    s.sessions.push({...s.sessions[0],id:'events',path:'/events',url:'https://example.test/events',status:200,streamProtocol:'sse'});
    s.streams.events={...s.streams.socket,state:{protocol:'sse',status:'open',atMillis:1000,reason:null},observed:1,evicted:0};
    s.streamItems=[{sequence:10,message:{...s.streamItems[1].message,kind:'event',event:'update',eventId:'next-id',retry:1500,payload:[...new TextEncoder().encode('first\nsecond')],raw:'id: next-id\nretry: 1500\nevent: update\ndata: first\ndata: second\n\n'}}];
    await t.revealSession('events');
  });
  await page.locator('#session-events .stream-identity').waitFor();
  assert.equal(await page.locator('#session-events .stream-identity').textContent(),'SSE⌄');
  await inspector.getByLabel('Message sender').selectOption('');
  await inspector.locator('.stream-row').filter({hasText:'update'}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('stream-inspector').streamPayload?.eventId==='next-id');
  assert.match(await inspector.locator('.stream-facts').textContent(),/ID: next-id.*Retry: 1500 ms/);
  await inspector.getByLabel('Message payload viewer').selectOption('raw');
  assert.match(await inspector.locator('pre').textContent(),/data: first\ndata: second/);
  await inspector.getByRole('button',{name:'Save retained message…',exact:true}).click();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.savedStream.raw),true);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-sse.png')});
  // Both protocols remain visible while quiet, without changing selection on refresh.
  for(const viewport of [{width:1440,height:900},{width:960,height:720},{width:800,height:600}]) {
    await page.setViewportSize(viewport);
    for(const compact of [false,true]) {
      await page.evaluate(compact=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,compactRows:compact};},compact);
      for(const id of ['socket','events']) {
        const badge=page.locator('#session-'+id+' .stream-identity');
        const box=await badge.boundingBox();assert.ok(box.width>0&&box.height>0);
        assert.equal(await badge.evaluate(el=>{
          const cell=el.closest('td').getBoundingClientRect(),box=el.getBoundingClientRect();
          const text=document.createRange();text.selectNodeContents(el);
          return [box,...text.getClientRects()].every(rect=>rect.left>=cell.left&&rect.right<=cell.right&&rect.top>=cell.top&&rect.bottom<=cell.bottom);
        }),true,'Containing cell clips the active badge or its label');
      }
    }
    await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-active-'+viewport.width+'.png')});
  }
  await page.emulateMedia({forcedColors:'active'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-active-contrast.png')});
  await page.emulateMedia({forcedColors:'none'});
  await page.locator('#header-protocol .column-resizer').dblclick();
  for(const id of ['socket','events']) {
    assert.equal(await page.locator('#session-'+id+' [data-column-id="protocol"]').evaluate(el=>el.scrollWidth<=el.clientWidth+1),true,'Fit column clips a stream badge or HTTP version');
  }
  const fittedPreferences=await page.evaluate(()=>document.querySelector('app-shell').traffic.preferences);
  const baseline=await page.locator('#session-socket').evaluate(row=>({row:row.getBoundingClientRect().height,header:row.closest('table').tHead.getBoundingClientRect().height}));
  for(const width of [100,60,32]) {
    await page.evaluate(width=>{
      const t=document.querySelector('app-shell').traffic;
      // Old persisted wrap settings must not re-enable wrapping.
      t.preferences={...t.preferences,wrapCells:true,columns:t.preferences.columns.map(column=>column.id==='protocol'||column.id==='path'?{...column,width}:column)};
    },width);
    for(const id of ['socket','events']) {
      const cell=page.locator('#session-'+id+' [data-column-id="protocol"]');
      await cell.scrollIntoViewIfNeeded();
      await page.waitForFunction(({id,width})=>{
        const cell=document.querySelector('app-shell').traffic.querySelector('#session-'+id+' [data-column-id="protocol"]');
        return cell&&Math.abs(cell.getBoundingClientRect().width-width)<1;
      },{id,width});
      const layout=await cell.evaluate(cell=>{
        const badge=cell.querySelector('.stream-identity'),text=document.createRange();text.selectNodeContents(badge);
        return {row:cell.closest('tr').getBoundingClientRect().height,header:cell.closest('table').tHead.getBoundingClientRect().height,cellSpace:getComputedStyle(cell).whiteSpace,badgeSpace:getComputedStyle(badge).whiteSpace,lines:new Set([...text.getClientRects()].map(rect=>Math.round(rect.top))).size,clips:cell.scrollWidth>cell.clientWidth,title:cell.title};
      });
      assert.equal(layout.cellSpace,'nowrap');assert.equal(layout.badgeSpace,'nowrap');assert.equal(layout.lines,1);
      assert.ok(Math.abs(layout.row-baseline.row)<1&&Math.abs(layout.header-baseline.header)<1,'Narrow columns changed row or header height');
      assert.equal(layout.clips,true,'Narrow Protocol cell should clip before wrapping');
      assert.match(layout.title,id==='socket'?/WebSocket/:/SSE/);
    }
    await page.screenshot({path:resolve(root,'../../../target/ui-check/stream-active-protocol-'+width+'.png')});
  }
  await page.evaluate(preferences=>document.querySelector('app-shell').traffic.preferences=preferences,fittedPreferences);
  for(const terminal of ['completed','failed','interrupted','open-at-capture','incomplete']) {
    await page.evaluate(async terminal=>{
      const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;
      s.sessions=s.sessions.map(row=>({...row,terminal}));await t.refreshSessions(undefined,true);
    },terminal);
    assert.equal(await page.locator('.stream-identity').count(),2,'Ended or saved streams lost their identity');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'events');
  }
  await page.evaluate(async()=>{
    const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;
    s.sessions=s.sessions.map(row=>({...row,terminal:'active',streamProtocol:null}));await t.refreshSessions(undefined,true);
  });
  assert.equal(await page.locator('.stream-identity').count(),0,'Ordinary active HTTP appears as a stream');
  await page.evaluate(async(saved)=>{Object.assign(globalThis.__workspaceFixture,saved,{streams:{},streamItems:[]});const t=document.querySelector('app-shell').traffic;t.preferences=saved.preferences;t.showStream(false);await t.refreshSessions(undefined,true);},saved);
  await page.setViewportSize({width:1280,height:800});
}
