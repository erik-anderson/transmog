import assert from 'node:assert/strict';

export async function checkRequestControls(page) {
  await page.goto('https://workspace.test/');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  const traffic=page.locator('traffic-workspace');
  await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture,w=document.querySelector('app-shell').traffic,seed=state.sessions[0];
    state.sessions=Array.from({length:1500},(_,index)=>({...seed,id:'request-'+String(index).padStart(4,'0'),requestNumber:String(index+1),startedAt:1000+index,path:'/request-'+index,url:'http://example.test/request-'+index,terminal:index===1499?'active':'completed',terminalText:index===1499?'Cancelled. Request canceled in Transmog.':'completed'}));
    w.sort={column:'started-at',direction:'ascending'};w.followLatest=false;w.queryRevision++;
    await w.refreshSessions(undefined,true);
  });
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.rowCache.has('request-1499')),false,'Active request was unexpectedly present in the viewport cache');
  await page.evaluate(()=>document.querySelector('app-shell').traffic.trafficTable.focus());
  await page.keyboard.press('Control+a');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedTrafficCount===1500);
  await traffic.locator('.traffic-selection-bar').getByRole('button',{name:'More',exact:true}).click();
  const cancel=traffic.locator('#traffic-entry-menu').getByRole('button',{name:'Cancel requests',exact:true});
  assert.equal(await cancel.isEnabled(),true,'An uncached active selected request disabled cancellation');
  await cancel.click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.cancelingRequests);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.sessions.find(row=>row.id==='request-1499').terminal),'cancelled');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.sessions.filter(row=>row.terminal==='completed').length),1499);
  await page.evaluate(()=>document.querySelector('app-shell').traffic.revealSession('request-1499'));
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='request-1499');
  assert.equal(await traffic.locator('.selection-bar .status-badge').textContent(),'200 · Cancelled');
  await page.evaluate(()=>document.querySelector('app-shell').traffic.copyRequestSummary(true));
  assert.doesNotMatch(await page.evaluate(()=>navigator.clipboard.readText()),/caller/i);

  for(const change of ['selection','focus']) {
    await page.goto('https://workspace.test/');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
    await traffic.locator('tr[data-session-id="first"]').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='first');
    await page.evaluate(()=>{globalThis.__workspaceFixture.holdCommand='remove_traffic_entries';});
    await traffic.locator('tr[data-session-id="first"]').click({button:'right'});
    await traffic.locator('#traffic-entry-menu').getByRole('button',{name:/^Remove selected entries/}).click();
    await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
    assert.equal(await traffic.locator('#traffic-entry-menu').evaluate(menu=>menu.matches(':popover-open')),false,'Removal menu stayed open while awaiting transport cleanup');
    assert.equal(await traffic.locator('table').first().evaluate(table=>table.getRootNode().activeElement===table),true,'Closing the removal menu left focus on a hidden item');
    if(change==='selection') {
      await traffic.locator('tr[data-session-id="image"]').click();
      await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='image');
    } else {
      await traffic.getByRole('button',{name:'Filters',exact:true}).focus();
    }
    await page.evaluate(()=>globalThis.__workspaceFixture.finishHeldCommand());
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
    assert.equal(await traffic.locator('tr[data-session-id="first"]').count(),0);
    if(change==='selection') {
      assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'image','Late removal replaced a newer inspector');
      assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids]),['image'],'Late removal replaced a newer selection');
      assert.equal(await traffic.locator('tr[data-session-id="image"]').evaluate(row=>row.getRootNode().activeElement===row),true,'Late removal moved focus from the newer row');
    } else {
      assert.equal(await traffic.getByRole('button',{name:'Filters',exact:true}).evaluate(button=>button.getRootNode().activeElement===button),true,'Late removal stole focus from a new control');
    }
  }
  await page.goto('https://workspace.test/');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  const first=traffic.locator('tr[data-session-id="first"]');
  await first.click();await first.focus();await page.keyboard.press('Delete');
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
  assert.equal(await page.evaluate(()=>{const w=document.querySelector('app-shell').traffic;return w.getRootNode().activeElement?.id==='session-'+w.trafficSelection.focused;}),true,'Keyboard removal did not move focus to the surviving request');
  await page.goto('https://workspace.test/');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture,w=document.querySelector('app-shell').traffic;
    state.sessions=[{...state.sessions[0],terminal:'active',streamProtocol:'web-socket'}];
    await w.refreshSessions(undefined,true);
  });
  await traffic.locator('tr[data-session-id="first"]').click();
  await traffic.locator('tr[data-session-id="first"]').focus();await page.keyboard.press('Delete');
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
  const kept=await page.evaluate(()=>document.querySelector('app-shell').diagnosticText);
  assert.match(kept,/Open or connecting WebSockets were kept/);assert.doesNotMatch(kept,/unless|enabled|Enable/);
}
