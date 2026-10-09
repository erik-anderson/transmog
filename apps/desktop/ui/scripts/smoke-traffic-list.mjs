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
  // Drag the actual native scrollbar thumb into the middle of the full capture.
  const scrollbar=await client.evaluate(`(()=>{const s=${t}.sessionScroller,r=s.getBoundingClientRect();return {x:r.right-(s.offsetWidth-s.clientWidth)/2,start:r.top+(s.offsetWidth-s.clientWidth)*1.5,end:r.top+r.height/2};})()`);
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
  await key('Delete','Delete',46);await wait(`${t}.totalMatched===14999 && !${t}.removingTraffic`);
  await key('z','KeyZ',90,2);await wait(`${t}.totalMatched===20000 && ${t}.selectedTrafficCount===5001 && !${t}.removingTraffic`);
  await client.evaluate(`${t}.trafficTable.focus()`);await key('a','KeyA',65,2);await wait(`${t}.selectedTrafficCount===20000`);
  await key('Delete','Delete',46);await wait(`${t}.totalMatched===0 && !${t}.removingTraffic`);
  await client.evaluate(`${t}.trafficTable.focus()`);await key('z','KeyZ',90,2);await wait(`${t}.totalMatched===20000 && ${t}.selectedTrafficCount===20000 && !${t}.removingTraffic`);
  await client.evaluate(`${t}.trafficTable.focus()`);await key('Escape','Escape',27);await key('End','End',35);await wait(`${t}.selectedSessionId===${JSON.stringify(ids[2])}`);
  await key('PageUp','PageUp',33);await wait(`${t}.virtualList.indexes.get(${t}.trafficSelection.focused)<19998 && ${t}.selectedDetail?.id===${t}.trafficSelection.focused`);
  await key('End','End',35);await wait(`${t}.selectedSessionId===${JSON.stringify(ids[2])}`);
  assert.ok(await client.evaluate(`${t}.sessions.length<150 && ${t}.rowCache.size<=1024`));
  await client.evaluate(`(async()=>{const w=${t};w.selectSearchMatches=false;w.searchMetadata=true;w.searchRequestHeaders=false;w.searchResponseHeaders=false;w.searchBodies=false;w.searchInput.value='virtual';await w.runContentSearch();await w.showMatches();})()`);
  await wait(`!${t}.matchBusy && ${t}.matchDialog.open`);
  const current=await client.evaluate(`${t}.matchEntryId`);
  await client.evaluate(`${t}.moveMatchEntry(-1)`);await wait(`!${t}.matchBusy && ${t}.matchEntryId!==${JSON.stringify(current)}`);
  await client.evaluate(`${t}.closeMatches()`);
  assert.equal(await client.evaluate(`${t}.getRootNode().activeElement?.dataset.sessionId===${t}.matchEntryId && ${t}.selectedSessionId===${t}.matchEntryId && ${t}.contentSearchActive`),true);
  if(screenshot){const image=await client.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-traffic.png'),Buffer.from(image.data,'base64'));}
  process.stdout.write('WebView2 traffic: 20,000 entries, offscreen range, select all/delete/undo and search reveal verified.\n');
}
