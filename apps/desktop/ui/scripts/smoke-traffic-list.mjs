import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

/** Real WebView2 probe using imported backend data and native input events. */
export async function smokeTrafficList(client,screenshot) {
  const t='document.querySelector("app-shell").traffic';
  const wait=async(expression,timeout=30000)=>{const deadline=Date.now()+timeout;while(Date.now()<deadline){if(await client.evaluate(expression))return;await new Promise(resolve=>setTimeout(resolve,50));}const state=await client.evaluate(`(()=>{const s=document.querySelector('app-shell'),w=s?.traffic;return {shell:!!s,total:w?.totalMatched,rows:w?.sessions.length,importing:w?.importingTrace,importStatus:w?.importStatus,queryError:w?.queryError,notice:s?.noticeMessageText};})()`);throw new Error('Traffic probe did not settle: '+expression+' · '+JSON.stringify(state));};
  process.stdout.write('Loading native 20,000-entry traffic fixture…\n');
  await wait(`document.querySelector("app-shell")?.traffic && ${t}.totalMatched===20000 && !${t}.importingTrace && ${t}.sessions.every(row=>!row.loading)`,120000);
  const ids=await client.evaluate(`(()=>{const w=${t};return [w.virtualList.ids[0],w.virtualList.ids[5000],w.virtualList.ids.at(-1)];})()`);
  const click=async(id,modifiers=0)=>{
    const box=await client.evaluate(`(()=>{const r=${t}.getRootNode().getElementById(${JSON.stringify('session-')}+${JSON.stringify(id)}).getBoundingClientRect();return {x:r.x+100,y:r.y+r.height/2};})()`);
    await client.call('Input.dispatchMouseEvent',{type:'mousePressed',...box,button:'left',clickCount:1,modifiers});
    await client.call('Input.dispatchMouseEvent',{type:'mouseReleased',...box,button:'left',clickCount:1,modifiers});
  };
  const key=async(key,code,virtualKey,modifiers=0)=>{
    const text=modifiers===0?(key==='Enter'?'\r':key===' '?' ':''):'';
    await client.call('Input.dispatchKeyEvent',{type:text?'keyDown':'rawKeyDown',key,code,windowsVirtualKeyCode:virtualKey,modifiers,text,unmodifiedText:text});
    await client.call('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode:virtualKey,modifiers});
  };
  for(const [value,code,virtualKey] of [['Enter','Enter',13],[' ','Space',32]]){
    await client.evaluate(`${t}.trafficTable.tHead.querySelector('.column-trigger').focus()`);
    await key(value,code,virtualKey);await wait(`${t}.columnMenu.matches(':popover-open')`);
    assert.equal(await client.evaluate(`${t}.selectedTrafficCount`),0,'Native header activation selected traffic');
    await key('Escape','Escape',27);await wait(`!${t}.columnMenu.matches(':popover-open')`);
  }
  // Request order uses real backend sorting and keeps its exact source association.
  await client.evaluate(`${t}.getRootNode().getElementById('header-request-number').querySelector('button').focus()`);
  await key('Enter','Enter',13);await wait(`${t}.columnMenu.matches(':popover-open')`);
  await key('Enter','Enter',13);
  await wait(`${t}.sort.column==='request-number' && ${t}.sort.direction==='ascending' && !${t}.queryPending`);
  // Sorting preserves the request anchor; deliberately reveal the first result.
  await client.evaluate(`${t}.revealCurrent(${t}.virtualList.ids[0])`);
  await wait(`${t}.sessions[0]?.requestNumber==='1' && ${t}.sessions.every(row=>!row.loading)`);
  assert.equal(await client.evaluate(`${t}.getRootNode().getElementById('header-request-number').getAttribute('aria-sort')`),'ascending');
  assert.deepEqual(await client.evaluate(`${t}.sessions.slice(0,3).map(row=>row.requestNumber)`),['1','2','3']);
  await client.evaluate(`${t}.applySort('descending','request-number')`);
  await wait(`!${t}.queryPending && ${t}.sort.direction==='descending'`);
  // Explicit sorting preserves the viewport's request anchor; inspect the full
  // ordering before deliberately revealing its leading entry for screenshots.
  const numberedId=await client.evaluate(`${t}.virtualList.ids[0]`);
  await client.evaluate(`${t}.revealCurrent(${JSON.stringify(numberedId)})`);
  await wait(`!${t}.queryPending && ${t}.sessions[0]?.requestNumber==='20000'`);
  const numberedDetail=await client.evaluate(`window.__TAURI_INTERNALS__.invoke('session_detail',{id:${JSON.stringify(numberedId)}})`);
  assert.equal(numberedDetail.requestNumber,'20000');assert.equal(numberedDetail.originalRequestNumber,'20000');
  if(screenshot)for(const [width,height] of [[1440,900],[960,720],[800,600]]){
    await client.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    const image=await client.call('Page.captureScreenshot',{format:'png'});
    await writeFile(screenshot.replace('.png','-request-order-'+width+'.png'),Buffer.from(image.data,'base64'));
  }
  // A 640x480 CSS viewport at 125% device scale occupies an 800x600 surface.
  await client.call('Emulation.setDeviceMetricsOverride',{width:640,height:480,deviceScaleFactor:1.25,mobile:false});
  assert.equal(await client.evaluate(`(()=>{const h=${t}.getRootNode().getElementById('header-request-number'),r=h.getBoundingClientRect();return h.querySelector('button').getAttribute('aria-label')==='Request order' && r.left>=0 && r.right<=innerWidth;})()`),true);
  if(screenshot){const image=await client.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-request-order-scaled.png'),Buffer.from(image.data,'base64'));}
  await client.call('Emulation.clearDeviceMetricsOverride');
  await client.evaluate(`${t}.applySort('descending','started-at')`);
  await client.evaluate(`${t}.revealCurrent(${JSON.stringify(ids[0])})`);
  await wait(`!${t}.queryPending && ${t}.sessions.every(row=>!row.loading)`);
  // Drag the actual native scrollbar thumb into the middle of the full capture.
  await client.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  const scrollbar=await client.evaluate(`(()=>{const s=${t}.sessionScroller,r=s.getBoundingClientRect(),gutter=s.offsetWidth-s.clientWidth,track=s.clientHeight-2*gutter,thumb=Math.max(17,track*s.clientHeight/s.scrollHeight),position=s.scrollTop/Math.max(1,s.scrollHeight-s.clientHeight);return {x:r.right-gutter/2,start:r.top+gutter+(track-thumb)*position+thumb/2,end:r.top+r.height/2};})()`);
  await client.call('Input.dispatchMouseEvent',{type:'mousePressed',x:scrollbar.x,y:scrollbar.start,button:'left',clickCount:1});
  await client.call('Input.dispatchMouseEvent',{type:'mouseMoved',x:scrollbar.x,y:scrollbar.end,button:'left',buttons:1});
  await client.call('Input.dispatchMouseEvent',{type:'mouseReleased',x:scrollbar.x,y:scrollbar.end,button:'left',clickCount:1});
  await wait(`${t}.sessionScroller.scrollTop>${t}.virtualList.total*.2 && ${t}.sessionScroller.scrollTop<${t}.virtualList.total*.8 && ${t}.sessions.every(row=>!row.loading)`);
  await client.evaluate(`${t}.revealCurrent(${JSON.stringify(ids[0])},true)`);
  await click(ids[0]);await wait(`${t}.selectedSessionId===${JSON.stringify(ids[0])}`);
  await client.evaluate(`${t}.revealCurrent(${JSON.stringify(ids[1])})`);
  await wait(`${t}.sessions.some(row=>row.id===${JSON.stringify(ids[1])}&&!row.loading)`);
  assert.equal(await client.evaluate(`!!${t}.getRootNode().getElementById(${JSON.stringify('session-'+ids[0])})`),false);
  await click(ids[1],8);await wait(`${t}.selectedTrafficCount===5001`);
  assert.equal(await client.evaluate('getSelection().toString()'),'','Native Shift-click selected row text');
  await key('Delete','Delete',46);await wait(`${t}.totalMatched===14999 && !${t}.removingTraffic`);
  await key('z','KeyZ',90,2);await wait(`${t}.totalMatched===20000 && ${t}.selectedTrafficCount===5001 && !${t}.removingTraffic`);
  assert.equal(await client.evaluate(`window.__TAURI_INTERNALS__.invoke('session_detail',{id:${JSON.stringify(numberedId)}}).then(detail=>detail.requestNumber)`),'20000');
  await client.evaluate(`${t}.trafficTable.focus()`);await key('a','KeyA',65,2);await wait(`${t}.selectedTrafficCount===20000`);
  await key('Delete','Delete',46);await wait(`${t}.totalMatched===0 && !${t}.removingTraffic`);
  await client.evaluate(`${t}.trafficTable.focus()`);await key('z','KeyZ',90,2);await wait(`${t}.totalMatched===20000 && ${t}.selectedTrafficCount===20000 && !${t}.removingTraffic`);
  await client.evaluate(`${t}.trafficTable.focus()`);await key('Escape','Escape',27);await key('End','End',35);await wait(`${t}.selectedSessionId===${JSON.stringify(ids[2])}`);
  await key('PageUp','PageUp',33);await wait(`${t}.virtualList.indexes.get(${t}.trafficSelection.focused)<19998 && ${t}.selectedDetail?.id===${t}.trafficSelection.focused`);
  await key('End','End',35);await wait(`${t}.selectedSessionId===${JSON.stringify(ids[2])}`);
  assert.ok(await client.evaluate(`${t}.sessions.length<150 && ${t}.rowCache.size<=1024`));
  await client.evaluate(`(async()=>{const w=${t};await w.changeTrafficSearchKind({currentTarget:{value:'contents'}});w.selectSearchMatches=false;w.searchMetadata=true;w.searchRequestHeaders=false;w.searchResponseHeaders=false;w.searchBodies=false;w.searchInput.value='virtual';await w.runContentSearch();await w.showMatches();})()`);
  await wait(`!${t}.matchBusy && ${t}.matchDialog.open`);
  const current=await client.evaluate(`${t}.matchEntryId`);
  await client.evaluate(`${t}.moveMatchEntry(-1)`);await wait(`!${t}.matchBusy && ${t}.matchEntryId!==${JSON.stringify(current)}`);
  await client.evaluate(`${t}.closeMatches()`);
  assert.equal(await client.evaluate(`${t}.getRootNode().activeElement?.dataset.sessionId===${t}.matchEntryId && ${t}.selectedSessionId===${t}.matchEntryId && ${t}.contentSearchActive`),true);
  if(screenshot){const image=await client.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-traffic.png'),Buffer.from(image.data,'base64'));}
  await client.evaluate(`${t}.clearTrafficSelection();${t}.setTimeline(true)`);
  await wait(`${t}.timelineEntries.length===20000 && !${t}.timelineBusy`,120000);
  assert.ok(await client.evaluate(`${t}.timelineEntries.some(row=>row.startMillis>0 && row.durationMicros>0)`),'Native fixture did not produce measured intervals');
  assert.ok(await client.evaluate(`${t}.timelineEntries.some(row=>row.startMillis===null)`),'Native fixture lost unavailable timing');
  assert.equal(await client.evaluate(`${t}.querySelector('.timeline-span').namespaceURI`),'http://www.w3.org/2000/svg');
  assert.ok(await client.evaluate(`${t}.querySelectorAll('.timeline-row').length<100`));
  // Starting on a button must not prevent Escape from canceling a native drag.
  const rangeWidth=await client.evaluate(`${t}.querySelector('.timeline-window').getAttribute('width')`);
  const handle=await client.evaluate(`(()=>{[...${t}.querySelectorAll('traffic-timeline button')].find(button=>button.textContent==='Reset range').focus();const r=${t}.querySelector('[aria-label="Timeline end"]').getBoundingClientRect();return {x:r.x+1,y:r.y+5};})()`);
  await client.call('Input.dispatchMouseEvent',{type:'mousePressed',...handle,button:'left',clickCount:1});
  await client.call('Input.dispatchMouseEvent',{type:'mouseMoved',x:handle.x-50,y:handle.y,button:'left',buttons:1});
  await key('Escape','Escape',27);
  await client.call('Input.dispatchMouseEvent',{type:'mouseReleased',x:handle.x-50,y:handle.y,button:'left',clickCount:1});
  assert.equal(await client.evaluate(`${t}.querySelector('.timeline-window').getAttribute('width')`),rangeWidth,'Native Escape failed to cancel a drag');
  await client.evaluate(`${t}.querySelector('.timeline-viewport').focus()`);await key('a','KeyA',65,2);
  assert.equal(await client.evaluate(`${t}.selectedTrafficCount`),20000);
  await client.evaluate(`${t}.querySelector('[aria-label="Timeline end"]').focus()`);await key('Escape','Escape',27);
  assert.equal(await client.evaluate(`${t}.selectedTrafficCount`),0,'Native Escape on a slider did not clear selection');
  await client.evaluate(`${t}.querySelector('[aria-label="Timeline end"]').focus()`);
  const before=await client.evaluate(`${t}.querySelector('.timeline-scale').textContent`);
  await key('Home','Home',36);
  assert.notEqual(await client.evaluate(`${t}.querySelector('.timeline-scale').textContent`),before,'Native range keyboard did not resize');
  await client.evaluate(`${t}.querySelector('traffic-timeline').resetRange();${t}.trafficSelection.replace(${JSON.stringify(ids[0])});${t}.trafficSelection.ids.add(${JSON.stringify(ids[1])});${t}.updateTrafficSelection()`);
  await wait(`${t}.querySelectorAll('.timeline-row').length===2`);
  const timelineRow=await client.evaluate(`(()=>{const row=${t}.querySelector('.timeline-row');row.scrollIntoView({block:'center'});const r=row.getBoundingClientRect(),x=r.x+20,y=r.y+r.height/2;return {id:row.dataset.id,x,y,hit:${t}.getRootNode().elementFromPoint(x,y)?.closest('.timeline-row')?.dataset.id};})()`);
  assert.equal(timelineRow.hit,timelineRow.id,'Native timeline pointer target is outside the visible rows');
  if(screenshot){const image=await client.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-timeline-pointer.png'),Buffer.from(image.data,'base64'));}
  for(const type of ['mousePressed','mouseReleased'])await client.call('Input.dispatchMouseEvent',{type,x:timelineRow.x,y:timelineRow.y,button:'left',clickCount:1});
  await wait(`${t}.selectedDetail?.id===${JSON.stringify(timelineRow.id)}`);
  assert.equal(await client.evaluate(`${t}.selectedTrafficCount`),2,'Native timeline inspection changed selected scope');
  assert.notEqual(await client.evaluate(`${t}.selectedUrlText`),'Loading request…');
  await client.evaluate(`${t}.actionsButton.focus()`);await key('Enter','Enter',13);
  assert.equal(await client.evaluate(`[...${t}.requestActionsMenu.querySelectorAll('button')].find(button=>button.textContent==='Set as comparison baseline').disabled`),false);
  assert.equal(await client.evaluate(`[...${t}.requestActionsMenu.querySelectorAll('button')].find(button=>button.textContent==='Copy request reference').disabled`),false);
  await key('Escape','Escape',27);
  const timelinePreferences=await client.evaluate(`${t}.preferences`);
  await client.evaluate(`${t}.preferences={...${t}.preferences,listSplit:65}`);
  if(screenshot)for(const [width,height] of [[1440,900],[960,720],[800,600]]){
    await client.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    assert.ok(await client.evaluate(`${t}.querySelector('.timeline-viewport').clientHeight>=96`),'Native compact Timeline lost its request rows');
    assert.ok(await client.evaluate(`(()=>{const a=${t}.querySelector('.timeline-ticks').getBoundingClientRect();return [...${t}.querySelectorAll('.timeline-row>svg')].every(svg=>{const r=svg.getBoundingClientRect();return Math.abs(a.left-r.left)<1&&Math.abs(a.right-r.right)<1;});})()`),'Native bar widths differ from the axis');
    const image=await client.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-timeline-'+width+'.png'),Buffer.from(image.data,'base64'));
  }
  await client.call('Emulation.clearDeviceMetricsOverride');
  await client.evaluate(`${t}.preferences=${JSON.stringify(timelinePreferences)}`);
  await client.evaluate(`${t}.liveFilter='no-native-timeline-match';${t}.queryRevision++;${t}.refreshSessions(undefined,true).then(()=>${t}.refreshTimeline())`);
  await wait(`${t}.timelineEntries.length===0 && !${t}.timelineBusy && ${t}.timelineTimer===undefined`);
  await client.evaluate(`${t}.revealSession(${JSON.stringify(ids[2])})`);
  await wait(`${t}.querySelector('.timeline-row')?.dataset.id===${JSON.stringify(ids[2])}`);
  assert.equal(await client.evaluate(`${t}.getRootNode().activeElement?.dataset.id`),ids[2],'Native source reveal left focus on hidden List');
  await client.evaluate(`${t}.setTimeline(false);${t}.clearTrafficSelection()`);
  process.stdout.write('WebView2 timeline: full 20,000-row metadata scope, native keyboard range, selected subset and pointer inspection verified.\n');
  process.stdout.write('WebView2 traffic: 20,000 entries, offscreen range, select all/delete/undo and search reveal verified.\n');
}
