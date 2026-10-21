import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficShortcuts(page,root) {
  await page.setViewportSize({width:1440,height:900});
  const traffic=page.locator('traffic-workspace'),table=traffic.locator('.traffic-table');
  const undo=traffic.getByRole('button',{name:'Undo traffic removal',exact:true});
  const redo=traffic.getByRole('button',{name:'Redo traffic removal',exact:true});
  const settled=async count=>page.waitForFunction(count=>{const t=document.querySelector('app-shell').traffic;return !t.removingTraffic&&!t.queryPending&&t.totalMatched===count;},count);
  const open=async(count=4)=>{
    await page.goto('https://workspace.test/');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
    await page.evaluate(async count=>{
      const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[0];
      s.sessions=Array.from({length:count},(_,index)=>({...seed,id:'shortcut-'+index,requestNumber:String(index+1),startedAt:1000+index,path:'/shortcut/'+index,url:'http://example.test/shortcut/'+index,terminal:'completed',traceId:null,streamProtocol:null}));
      await t.refreshSessions(undefined,true);
    },count);
    await settled(count);
  };
  const keyWasHandled=async(locator,key,modifiers={})=>locator.evaluate((node,{key,modifiers})=>!node.dispatchEvent(new KeyboardEvent('keydown',{key,bubbles:true,composed:true,cancelable:true,...modifiers})),{key,modifiers});

  await open();await table.focus();await page.keyboard.press('Control+Space');
  assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids]),['shortcut-3'],'Ctrl+Space from list focus selected an invalid entry');
  await open(0);await table.focus();await page.keyboard.press('Control+Space');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Ctrl+Space selected an entry in an empty list');

  await open();await traffic.locator('#session-shortcut-0').click();await page.keyboard.press('Delete');await settled(3);
  await undo.click();await settled(4);
  await traffic.locator('#session-shortcut-3').click();await traffic.locator('#session-shortcut-2').click({modifiers:['Control']});
  await page.keyboard.press('Control+y');await settled(3);
  assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids].sort()),['shortcut-2','shortcut-3'],'Redo collapsed an unrelated selection');
  await undo.click();await settled(4);
  await traffic.locator('#session-shortcut-3').click();await traffic.locator('#session-shortcut-0').click({modifiers:['Control']});
  await page.keyboard.press('Control+y');await settled(3);
  assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids]),['shortcut-3'],'Redo dropped the surviving part of a selection');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'shortcut-3');
  await traffic.locator('#session-shortcut-2').click({modifiers:['Shift']});
  assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids].sort()),['shortcut-2','shortcut-3'],'Redo lost the surviving range anchor');
  await undo.click();await settled(4);await page.keyboard.press('Escape');
  await page.keyboard.press('Control+y');await settled(3);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Redo selected a new request after Escape deselected everything');

  for(const change of ['selection','focus','escape','timeline-escape','follow']){
    await open();await traffic.locator('#session-shortcut-0').click();await page.keyboard.press('Delete');await settled(3);
    if(change==='timeline-escape'){
      await traffic.getByRole('button',{name:'Timeline',exact:true}).click();
      await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return !t.timelineBusy&&t.timeline.timelineDisplay.rows.length===t.selectedTrafficCount;});
    }
    await page.evaluate(()=>{globalThis.__workspaceFixture.holdCommand='remove_traffic_entries';});
    if(change==='follow'){await table.focus();await page.keyboard.press('Control+z');}else await undo.click();
    await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
    // Let the busy button render as disabled, including the browser's focus update.
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    if(change==='selection'){
      await traffic.locator('#session-shortcut-3').click();
      await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='shortcut-3');
    }else if(change==='focus')await traffic.getByLabel('Filter or search traffic',{exact:true}).focus();
    else await page.keyboard.press(change==='follow'?'F5':'Escape');
    if(change.endsWith('escape'))assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,`Escape did not clear ${change==='escape'?'list':'Timeline'} selection while Undo was pending`);
    const before=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {selected:[...t.trafficSelection.ids],inspected:t.selectedSessionId};});
    await page.evaluate(()=>globalThis.__workspaceFixture.finishHeldCommand());await settled(4);
    assert.deepEqual(await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {selected:[...t.trafficSelection.ids],inspected:t.selectedSessionId};}),before,`Late Undo replaced newer ${change}`);
    if(change==='focus')assert.equal(await traffic.getByLabel('Filter or search traffic',{exact:true}).evaluate(node=>node===node.getRootNode().activeElement),true,'Late Undo stole focus from a text field');
    if(change==='follow')assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.followLatest),true,'Late Undo paused newer F5 following');
  }

  await open();
  // Blank space beside and below narrow columns focuses the list without selection.
  await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,columns:t.preferences.columns.map(column=>({...column,visible:column.id==='request-number'||column.id==='method'}))};});
  const blank=async beside=>{
    const point=await traffic.locator('.table-wrap').evaluate((node,beside)=>{const r=node.getBoundingClientRect(),table=node.querySelector('table').getBoundingClientRect();return {x:r.right-30,y:beside?table.top+65:Math.min(r.bottom-20,table.bottom+15)};},beside);
    await page.mouse.click(point.x,point.y);
  };
  for(const beside of [true,false]){
    await traffic.getByRole('button',{name:'Filters',exact:true}).focus();await blank(beside);
    assert.equal(await table.evaluate(node=>node.getRootNode().activeElement===node),true,'Blank Traffic space did not focus the list');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Blank space selected a row');
  }
  for(const key of ['ArrowDown','ArrowUp']){
    await page.keyboard.press(key);
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='shortcut-3');
    assert.equal(await traffic.locator('#session-shortcut-3').getAttribute('aria-selected'),'true','Initial arrow skipped the first row');
    await page.keyboard.press('Escape');await blank(true);
  }
  await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowDown');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='shortcut-2');
  await blank(true);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Blank click retained the selection');
  const toggle=traffic.getByRole('button',{name:'Timeline',exact:true});
  for(const multiple of [false,true]){
    await traffic.locator('#session-shortcut-3').click();
    if(multiple)await traffic.locator('#session-shortcut-2').click({modifiers:['Control']});
    await toggle.click();
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return !t.timelineBusy&&t.timeline.timelineDisplay.rows.length===t.selectedTrafficCount;});
    await page.keyboard.press('Escape');
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.selectedTrafficCount===0&&t.timeline.timelineDisplay.rows.length===4;});
    await toggle.click();
  }
  await traffic.locator('#session-shortcut-3').click();
  await traffic.getByRole('button',{name:'Files',exact:true}).click();await page.keyboard.press('Escape');
  assert.equal(await traffic.locator('#trace-files-menu').evaluate(node=>node.matches(':popover-open')),false,'Escape did not dismiss the open Files menu');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),1,'Closing a menu also cleared selection');
  await traffic.getByLabel('Filter or search traffic',{exact:true}).focus();await page.keyboard.press('Escape');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),1,'Escape in a text field cleared Traffic selection');
  await traffic.locator('#session-shortcut-3').click();await page.keyboard.press('Escape');
  assert.equal(await traffic.locator('tr[aria-selected="true"]').count(),0,'Escape retained the single selection');
  await blank(true);
  await page.keyboard.press('Control+x');await settled(0);
  await blank(false);await page.keyboard.press('Control+z');await settled(4);
  await blank(true);await page.keyboard.press('Control+y');await settled(0);
  await open();
  const search=traffic.getByLabel('Filter or search traffic',{exact:true});
  await search.fill('example');await search.focus();
  for(const key of ['x','z','y'])assert.equal(await keyWasHandled(search,key,{ctrlKey:true}),false,`Ctrl+${key} intercepted text editing`);
  assert.equal(await keyWasHandled(search,'F5'),false,'F5 was intercepted outside the list');
  await search.fill('shortcut/0');await settled(1);
  await traffic.getByRole('button',{name:'Filters',exact:true}).focus();
  await page.keyboard.press('Control+x');await page.keyboard.press('Control+y');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.clear_traffic??0),0,'A toolbar control cleared Traffic');
  const header=table.locator('.column-trigger').first();await header.focus();await page.keyboard.press('Control+x');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.clear_traffic??0),0,'A column control cleared Traffic');
  await table.focus();
  assert.equal(await keyWasHandled(table,'x',{ctrlKey:true,repeat:true}),true);
  for(const modifiers of [{ctrlKey:true,shiftKey:true},{ctrlKey:true,altKey:true}])await keyWasHandled(table,'x',modifiers);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.clear_traffic??0),0,'Repeated or modified Ctrl+X cleared Traffic');
  await traffic.locator('#session-shortcut-0').click();await page.keyboard.press('Control+x');await settled(0);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.dismissedIds.length),4,'Ctrl+X cleared only filtered or selected rows');
  assert.equal(await table.evaluate(node=>node.getRootNode().activeElement===node),true,'Clear lost list focus');
  await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions.push({...s.sessions[0],id:'arrived-later',requestNumber:'5',startedAt:9999});await t.refreshSessions(undefined,true);});
  await undo.focus();await page.keyboard.press('Control+z');await settled(5);
  await page.keyboard.press('Control+y');await settled(1);
  assert.deepEqual(await page.evaluate(()=>document.querySelector('app-shell').traffic.virtualList.ids),['arrived-later'],'Redo removed newly arrived traffic');
  await undo.focus();await page.keyboard.press('Control+z');await settled(5);
  await redo.focus();await page.keyboard.press('Control+y');await settled(1);
  await undo.click();await settled(5);
  await redo.click();await settled(1);

  // Both stacks retain their order, and a new removal invalidates Redo.
  await open();
  for(const id of ['shortcut-0','shortcut-1']){await traffic.locator('#session-'+id).click();await page.keyboard.press('Delete');await settled(id==='shortcut-0'?3:2);}
  await undo.focus();await page.keyboard.press('Control+z');await settled(3);
  await page.keyboard.press('Control+z');await settled(4);
  await page.keyboard.press('Control+y');await settled(3);
  assert.equal(await traffic.locator('#session-shortcut-0').count(),0);
  await page.keyboard.press('Control+y');await settled(2);
  await undo.click();await settled(3);
  await traffic.locator('#session-shortcut-2').click();await page.keyboard.press('Delete');await settled(2);
  await table.focus();await page.keyboard.press('Control+y');await settled(2);
  assert.equal(await redo.isVisible(),false,'New removal retained stale Redo history');

  // A failed Redo stays retryable, and delayed Redo preserves newer focus/selection.
  await undo.click();await settled(3);
  await page.evaluate(()=>{const raw=window.__TAURI_INTERNALS__.invoke;globalThis.__redoInvoke=raw;window.__TAURI_INTERNALS__.invoke=async(command,args,options)=>{if(command==='remove_traffic_entries'&&!args.restore)throw new Error('Redo fixture failure');return raw(command,args,options);};});
  await redo.click();await settled(3);
  assert.equal(await redo.isVisible(),true);
  await page.evaluate(()=>{window.__TAURI_INTERNALS__.invoke=globalThis.__redoInvoke;delete globalThis.__redoInvoke;globalThis.__workspaceFixture.holdCommand='remove_traffic_entries';});
  await redo.click();await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
  await traffic.locator('#session-shortcut-3').click();
  await page.evaluate(()=>globalThis.__workspaceFixture.finishHeldCommand());await settled(2);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'shortcut-3');
  assert.equal(await traffic.locator('#session-shortcut-3').evaluate(node=>node.getRootNode().activeElement===node),true,'Late Redo stole focus');

  // Busy history and Clear buttons must leave keyboard focus inside Traffic.
  for(const operation of ['redo','clear']){
    await open();await traffic.locator('#session-shortcut-0').click();
    if(operation==='redo'){
      await page.keyboard.press('Delete');await settled(3);await undo.click();await settled(4);
    }
    await page.evaluate(command=>{globalThis.__workspaceFixture.holdCommand=command;},operation==='redo'?'remove_traffic_entries':'clear_traffic');
    await (operation==='redo'?redo:traffic.getByRole('button',{name:'Clear all',exact:true})).click();
    await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    assert.equal(await table.evaluate(node=>node.getRootNode().activeElement===node),true,`Pending ${operation} dropped list focus`);
    await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,`Escape did not clear selection while ${operation} was pending`);
    await page.evaluate(()=>globalThis.__workspaceFixture.finishHeldCommand());await settled(operation==='redo'?3:0);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,`Late ${operation} restored cleared selection`);
  }

  await open(250);await traffic.locator('#session-shortcut-249').click();await page.keyboard.press('Delete');await settled(249);
  await page.evaluate(()=>document.querySelector('app-shell').traffic.revealCurrent('shortcut-0',true));
  await traffic.locator('#session-shortcut-0').click();
  await page.keyboard.press('Control+z');await settled(250);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedDetail?.id),'shortcut-249','Keyboard Undo did not finish inspecting an offscreen restored entry');

  await open(250);await table.focus();
  await page.evaluate(()=>{globalThis.__workspaceFixture.holdCommand='traffic_view_rows';});
  await page.keyboard.press('End');
  await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
  await page.evaluate(()=>{globalThis.__workspaceFixture.queryDelay=80;});
  await page.keyboard.press('F5');
  // A queued scroll notification from End must not pause the newer F5 command.
  await page.evaluate(()=>document.querySelector('app-shell').traffic.sessionScroller.dispatchEvent(new Event('scroll')));
  await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.followLatest&&!t.queryPending&&t.sessionScroller.scrollTop===0;});
  await page.evaluate(()=>globalThis.__workspaceFixture.finishHeldCommand());
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.rowRequests.size===0);
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  assert.equal(await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return t.followLatest&&t.sessionScroller.scrollTop===0;}),true,'A late row navigation canceled F5 following');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='shortcut-0');

  await open(250);
  await traffic.locator('tr[data-session-id]').first().click();
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;await t.applySort('ascending','started-at');await t.revealCurrent('shortcut-100',true);});
  await page.keyboard.press('F5');
  await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.followLatest&&!t.queryPending&&t.virtualList.ids[0]==='shortcut-249'&&t.sessionScroller.scrollTop===0;});
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.sessions.push({...s.sessions[0],id:'newest-event',requestNumber:'251',startedAt:9999});s.channel.onmessage({exchangeId:'newest-event',sequence:1,lagged:false});});
  await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.followLatest&&t.sessions[0]?.id==='newest-event'&&t.sessionScroller.scrollTop===0;});
  for(const [width,height] of [[1440,900],[960,720],[800,600]]){
    await page.setViewportSize({width,height});await table.focus();await page.keyboard.press('Control+x');await settled(0);
    await undo.focus();await page.keyboard.press('Control+z');await settled(251);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/traffic-shortcuts-${width}.png`)});
    const box=await redo.boundingBox();assert.ok(box&&box.y>=0&&box.y+box.height<=height,'Redo is outside the viewport');
  }
  await page.setViewportSize({width:1440,height:900});
}
