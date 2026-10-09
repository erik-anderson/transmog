import assert from 'node:assert/strict';

export async function checkTrafficList(page,outputDirectory) {
  const saved=await page.evaluate(()=>({rows:globalThis.__workspaceFixture.sessions,dismissed:globalThis.__workspaceFixture.dismissedIds,sort:document.querySelector('app-shell').traffic.sort,sortLabel:document.querySelector('app-shell').traffic.sortLabel}));
  const settle=async()=>page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.sessions.length&&t.sessions.every(row=>!row.loading)&&!t.rowRequests.size;});
  const stats=[];
  for(const count of [10_000,50_000,100_000]){
    await page.evaluate(async count=>{
      const state=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=state.sessions[0];
      state.sessions=Array.from({length:count},(_,index)=>({...seed,id:'virtual-'+index,startedAt:index,path:'/virtual/'+index,url:'http://example.test/virtual/'+index,searchBody:index%5000===0?'VirtualNeedle':'other',topLevelNavigation:false}));
      state.dismissedIds=[];t.sort={column:'started-at',direction:'ascending'};t.sortLabel='Oldest first';t.followLatest=true;t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);
    },count);
    await settle();
    const measured=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {count:t.virtualList.ids.length,dom:t.sessions.length,total:t.sessionScroller.scrollHeight,height:t.sessionScroller.clientHeight};});
    assert.equal(measured.count,count);assert.ok(measured.dom<150,'DOM grows with dataset size');assert.ok(measured.total>count*20,'Scrollbar does not span the entire list');
    // Native thumb jumps must reach arbitrary windows without full queries.
    const before=await page.evaluate(()=>globalThis.__workspaceFixture.calls.query_traffic_view);
    for(const fraction of [.5,1,0]){
      await page.evaluate(fraction=>{const t=document.querySelector('app-shell').traffic;t.sessionScroller.scrollTop=fraction*(t.sessionScroller.scrollHeight-t.sessionScroller.clientHeight);t.sessionScrolled();},fraction);
      await page.waitForFunction(fraction=>{const t=document.querySelector('app-shell').traffic;return fraction===0?t.sessions[0]?.id==='virtual-0':fraction===1?t.sessions.at(-1)?.id==='virtual-'+(t.totalMatched-1):t.sessions.some(row=>row.index>t.totalMatched*.49&&row.index<t.totalMatched*.51);},fraction);
      await settle();
    }
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.query_traffic_view),before,'Scrolling rebuilt the query projection');
    stats.push(measured);
  }
  await page.locator('#session-virtual-0').click();
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;await t.revealCurrent('virtual-5000');});
  await settle();assert.equal(await page.locator('#session-virtual-0').count(),0,'First row remained mounted');
  await page.locator('#session-virtual-5000').click({modifiers:['Shift']});
  const range=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {count:t.selectedTrafficCount,first:t.trafficSelection.ids.has('virtual-0'),last:t.trafficSelection.ids.has('virtual-5000'),beyond:t.trafficSelection.ids.has('virtual-5001')};});
  assert.deepEqual(range,{count:5001,first:true,last:true,beyond:false});
  await page.keyboard.press('Delete');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===94999);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.dismissedIds.length),5001);
  await page.keyboard.press('Control+z');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===100000&&document.querySelector('app-shell').traffic.selectedTrafficCount===5001);
  await settle();
  await page.evaluate(async()=>await document.querySelector('app-shell').traffic.revealCurrent('virtual-50000'));
  await settle();const selectAllTop=await page.evaluate(()=>document.querySelector('app-shell').traffic.sessionScroller.scrollTop);
  await page.locator('.traffic-table').focus();await page.keyboard.press('Control+a');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),100000);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.sessionScroller.scrollTop),selectAllTop,'Select all moved the reading position');
  await page.keyboard.press('Delete');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===0);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.dismissedIds.length),100000);
  await page.locator('.traffic-table').focus();await page.keyboard.press('Control+z');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===100000&&document.querySelector('app-shell').traffic.selectedTrafficCount===100000);
  await settle();
  // Focus remains usable after the focused row leaves the DOM.
  await page.locator('.traffic-table').focus();await page.keyboard.press('Escape');await page.keyboard.press('Home');await page.keyboard.press('End');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='virtual-99999');
  assert.equal(await page.locator('#session-virtual-99999').evaluate(el=>el===el.getRootNode().activeElement),true);
  await page.keyboard.press('Control+Home');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.trafficSelection.focused==='virtual-0');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.trafficSelection.ids.has('virtual-99999')),true,'Ctrl+Home changed selection');
  await page.locator('.traffic-table').focus();await page.keyboard.press('Escape');
  await page.getByLabel('Search traffic',{exact:true}).fill('VirtualNeedle');await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===20);
  await page.getByRole('button',{name:'View matches…',exact:true}).click();
  const dialog=page.locator('.search-match-dialog[open]');
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.matchBusy);
  for(let index=0;index<15;index++){await dialog.getByRole('button',{name:'Next entry',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.matchBusy);}
  const current=await page.evaluate(()=>document.querySelector('app-shell').traffic.matchEntryId);
  await dialog.getByRole('button',{name:'Close',exact:true}).click();
  assert.equal(await page.locator('tr[data-current]').getAttribute('data-session-id'),current);
  assert.equal(await page.locator('tr[data-current]').evaluate(el=>el===el.getRootNode().activeElement),true);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.contentSearchActive),true,'Match navigation cleared search');
  await page.getByRole('button',{name:'Clear search',exact:true}).click();await settle();
  // Wrapped heights, narrow split, zoom and layout changes preserve a logical anchor.
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;for(let index=4900;index<5100;index++)state.sessions[index].path='/'+('long path '+index+' ').repeat(index%7+1);t.preferences={...t.preferences,wrapCells:true};await t.refreshSessions(undefined,true);const row=await t.revealCurrent('virtual-5000',true);if(row)await t.inspectSession(row);});
  await settle();await page.setViewportSize({width:800,height:600});await settle();
  await page.screenshot({path:outputDirectory+'/traffic-virtual-narrow.png'});
  await page.setViewportSize({width:1280,height:800});await settle();
  await page.screenshot({path:outputDirectory+'/traffic-virtual-wide.png'});
  assert.ok(await page.evaluate(()=>document.querySelector('app-shell').traffic.sessions.length<150));
  await page.evaluate(async saved=>{const state=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;state.sessions=saved.rows;state.dismissedIds=saved.dismissed??[];t.sort=saved.sort;t.sortLabel=saved.sortLabel;t.preferences={...t.preferences,wrapCells:false};t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);},saved);
  process.stdout.write('Virtual traffic checks: '+JSON.stringify(stats)+'\n');
}
