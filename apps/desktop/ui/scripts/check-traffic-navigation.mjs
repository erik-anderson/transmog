import assert from 'node:assert/strict';

export async function checkTrafficNavigation(page) {
  const viewport=page.viewportSize();
  const saved=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;return {rows:s.sessions,dismissed:s.dismissedIds,evicted:s.evictedIds,preferences:t.preferences,sort:t.sort,sortLabel:t.sortLabel};});
  const reset=async(search=false)=>{
    await page.evaluate(async()=>{
      const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;
      await t.clearContentSearch(false);t.clearTrafficSelection();t.inspectionGeneration++;t.selectedSessionId=null;t.selectedDetail=null;
      t.selectedMethodText='—';t.selectedUrlText='Select a request';t.selectedStatusText='No response';t.$emit('selection-changed',null);
      const seed=s.sessions[0];s.sessions=Array.from({length:1000},(_,index)=>({...seed,id:'navigation-'+index,method:'GET',path:'/navigation-'+index,url:'http://example.test/navigation-'+index,startedAt:index,contentType:'text/plain',topLevelNavigation:false}));
      s.dismissedIds=[];s.evictedIds=[];t.preferences={...t.preferences,wrapCells:false,compactRows:true,listSplit:45};
      t.sort={column:'started-at',direction:'ascending'};t.sortLabel='Oldest first';t.followLatest=true;t.queryRevision++;t.selectSearchMatches=false;
      await t.refreshSessions(undefined,true);
    });
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.sessions.every(row=>!row.loading));
    if(search)await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;t.searchInput.value='navigation-';t.searchMetadata=true;t.searchBodies=false;await t.runContentSearch();});
  };
  const hold=async(mode='after',command='traffic_view_rows')=>{
    await page.evaluate(({mode,command})=>{
      const raw=window.__TAURI_INTERNALS__.invoke;globalThis.__navigationDelay={raw,held:[],blocking:true};
      window.__TAURI_INTERNALS__.invoke=async(name,args,options)=>{
        const delay=globalThis.__navigationDelay,blocked=delay.blocking&&name===command&&(command!=='traffic_view_rows'||args.offset>=100);
        if(blocked&&mode==='before')await new Promise(resolve=>delay.held.push(resolve));
        const result=await raw(name,args,options);
        if(blocked&&mode==='after')await new Promise(resolve=>delay.held.push(resolve));
        return result;
      };
    },{mode,command});
  };
  const release=async()=>page.evaluate(()=>{const delay=globalThis.__navigationDelay;if(!delay)return;delay.blocking=false;for(const resolve of delay.held)resolve();delay.held=[];window.__TAURI_INTERNALS__.invoke=delay.raw;});
  const held=async()=>page.waitForFunction(()=>globalThis.__navigationDelay?.held.length>0);
  try {
    await page.setViewportSize({width:1280,height:800});await reset();
    for(const key of ['Enter','Space']){
      await page.locator('#header-method .column-trigger').focus();await page.keyboard.press(key);
      await page.locator('#column-actions').waitFor({state:'visible'});
      assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Header activation selected traffic');
      await page.keyboard.press('Escape');await page.locator('#column-actions').waitFor({state:'hidden'});
    }
    await page.locator('.traffic-table').focus();await page.keyboard.press('End');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='navigation-999');
    await page.keyboard.press('PageUp');
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.selectedDetail?.id===t.trafficSelection.focused;});
    assert.ok(await page.evaluate(()=>document.querySelector('app-shell').traffic.virtualList.indexes.get(document.querySelector('app-shell').traffic.trafficSelection.focused)<998),'PageUp moved only one entry from the end');
    await page.keyboard.press('Control+Home');await page.keyboard.press('PageDown');
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.selectedDetail?.id===t.trafficSelection.focused;});
    assert.ok(await page.evaluate(()=>document.querySelector('app-shell').traffic.virtualList.indexes.get(document.querySelector('app-shell').traffic.trafficSelection.focused)>1),'PageDown did not advance a viewport');

    // Cold row selection disables actions on the previous request immediately.
    for(const action of ['click','context','keyboard']){
      await reset();await page.locator('#session-navigation-10').click();
      await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='navigation-10');
      await hold();
      const id=action==='keyboard'?'navigation-999':'navigation-500';
      if(action==='keyboard'){await page.locator('.traffic-table').focus();await page.keyboard.press('End');}
      else {
        await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.sessionScroller.scrollTop=t.virtualList.offset(500);t.renderViewport();});
        await page.locator('#session-navigation-500').click({button:action==='context'?'right':'left'});
      }
      await held();
      const pending=await page.evaluate(()=>{
        const t=document.querySelector('app-shell').traffic;let emitted=null;
        const listener=event=>{emitted=event.detail.sessionId;event.stopPropagation();};
        t.addEventListener('autoresponse-request',listener);t.beginAutoResponseFromSelected();t.removeEventListener('autoresponse-request',listener);
        return {id:t.selectedSessionId,detail:t.selectedDetail,reuseDisabled:t.reuseDisabled,emitted};
      });
      assert.deepEqual(pending,{id,detail:null,reuseDisabled:true,emitted:null},'Cold selection retained actions on the previous inspector');
      await page.evaluate(async()=>{await navigator.clipboard.writeText('pending request');await document.querySelector('app-shell').traffic.copyUrl();});
      assert.equal(await page.evaluate(async()=>await navigator.clipboard.readText()),'pending request','Copy URL copied a loading placeholder');
      await release();await page.waitForFunction(id=>document.querySelector('app-shell').traffic.selectedDetail?.id===id,id);
      assert.ok(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedUrlText.endsWith(document.querySelector('app-shell').traffic.selectedSessionId)));
      if(action==='context')await page.keyboard.press('Escape');
    }

    // Selecting all matches in a cold window must use authoritative metadata.
    await reset(true);await hold();
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;globalThis.__navigationReveal=t.revealCurrent('navigation-500');});await held();
    await page.evaluate(()=>{globalThis.__navigationSelect=document.querySelector('app-shell').traffic.selectAllSearchMatches();});
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedTrafficCount===1000);
    await release();await page.evaluate(async()=>{await globalThis.__navigationReveal;await globalThis.__navigationSelect;});
    const inspector=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {method:t.selectedMethodText,url:t.selectedUrlText,actual:t.selectedDetail.requests[0].target};});
    assert.equal(inspector.method,'GET');assert.equal(inspector.url,inspector.actual);assert.ok(inspector.url);
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.copyUrl());
    assert.equal(await page.evaluate(async()=>await navigator.clipboard.readText()),inspector.actual);

    // Closing review and Delete must affect the newly reviewed request after a refresh.
    for(const mode of ['before','after']){
      await reset(true);await page.locator('#session-navigation-10').click();
      await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedSessionId==='navigation-10');
      await page.evaluate(async()=>await document.querySelector('app-shell').traffic.showMatches());await hold(mode);
      await page.evaluate(()=>{globalThis.__navigationMatch=document.querySelector('app-shell').traffic.loadMatchEntry('navigation-500');});await held();
      await page.evaluate(async()=>{globalThis.__workspaceFixture.dismissedIds=['navigation-0'];await document.querySelector('app-shell').traffic.refreshSessions();});
      await release();await page.evaluate(async()=>await globalThis.__navigationMatch);
      assert.deepEqual(await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {match:t.matchEntryId,selected:t.selectedSessionId,inspector:t.selectedDetail?.id,error:t.matchError};}),{match:'navigation-500',selected:'navigation-500',inspector:'navigation-500',error:''});
      await page.locator('.search-match-dialog').getByRole('button',{name:'Close',exact:true}).click();
      await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.activeElement?.dataset.sessionId==='navigation-500');
      await page.keyboard.press('Delete');await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
      assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.dismissedIds),['navigation-0','navigation-500']);
    }

    // Closing while a later entry is loading returns to the last committed request.
    await reset(true);await page.locator('#session-navigation-10').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedSessionId==='navigation-10');
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.showMatches());await hold();
    await page.evaluate(()=>{globalThis.__navigationMatch=document.querySelector('app-shell').traffic.loadMatchEntry('navigation-500');});await held();
    await page.locator('.search-match-dialog').getByRole('button',{name:'Close',exact:true}).click();await release();
    await page.evaluate(async()=>await globalThis.__navigationMatch);
    await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.activeElement?.dataset.sessionId==='navigation-10');
    assert.equal(await page.locator('tr[data-current]').getAttribute('data-session-id'),'navigation-10');

    // A delayed selection result cannot override a newer selection or cleared search.
    await reset(true);await hold('after','matching_traffic_ids');
    await page.evaluate(()=>{globalThis.__navigationSelect=document.querySelector('app-shell').traffic.selectAllSearchMatches();});await held();
    await page.locator('#session-navigation-10').click();await release();await page.evaluate(async()=>await globalThis.__navigationSelect);
    assert.deepEqual(await page.evaluate(()=>[...document.querySelector('app-shell').traffic.trafficSelection.ids]),['navigation-10']);
    await hold('after','matching_traffic_ids');await page.evaluate(()=>{globalThis.__navigationSelect=document.querySelector('app-shell').traffic.selectAllSearchMatches();});await held();
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.clearContentSearch());await release();await page.evaluate(async()=>await globalThis.__navigationSelect);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0);
    process.stdout.write('Traffic navigation regressions: header keys, paging, cold metadata/actions, refresh races, cancellation and stale selection verified.\n');
  } finally {
    await release();
    await page.evaluate(async saved=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;await t.clearContentSearch(false);t.clearTrafficSelection();s.sessions=saved.rows;s.dismissedIds=saved.dismissed??[];s.evictedIds=saved.evicted??[];t.preferences=saved.preferences;t.sort=saved.sort;t.sortLabel=saved.sortLabel;t.queryRevision++;await t.refreshSessions(undefined,true);},saved);
    if(viewport)await page.setViewportSize(viewport);
  }
}
