import {openActionGroup} from './check-action-menus.mjs';
import assert from 'node:assert/strict';
import {resolve} from 'node:path';
import {build} from 'esbuild';

export async function checkRequestOrderModel(root) {
  const compiled=await build({entryPoints:[resolve(root,'src/table-model.ts')],bundle:true,write:false,format:'esm',platform:'node'});
  const {defaultWorkspace,normalizeWorkspace,cellText}=await import('data:text/javascript;base64,'+Buffer.from(compiled.outputFiles[0].text).toString('base64'));
  const sixteen=defaultWorkspace();sixteen.columns=sixteen.columns.filter(c=>!['stream-sent','stream-received'].includes(c.id));sixteen.columns.reverse();sixteen.columns[0].width=399;sixteen.columns[1].pinned=true;sixteen.columns[2].visible=false;
  const streamUpgrade=normalizeWorkspace(sixteen);assert.deepEqual(streamUpgrade.columns.slice(0,16),sixteen.columns);assert.ok(streamUpgrade.columns.slice(16).every(c=>!c.visible&&!c.pinned));
  const legacy=defaultWorkspace();legacy.columns.shift();legacy.columns.reverse();legacy.columns[0].width=311;legacy.columns[1].visible=false;legacy.listSplit=67;
  const upgraded=normalizeWorkspace(legacy);
  assert.deepEqual(upgraded.columns.slice(1),legacy.columns);
  assert.equal(upgraded.columns[0].id,'request-number');assert.equal(upgraded.columns[0].pinned,true);assert.equal(upgraded.listSplit,67);
  upgraded.columns[0].visible=false;
  assert.deepEqual(normalizeWorkspace(JSON.parse(JSON.stringify(upgraded))),upgraded);
  assert.equal(cellText({requestNumber:'18446744073709551615'},'request-number'),'18446744073709551615');
  for(const invalid of [Object.assign(structuredClone(legacy),{columns:[...legacy.columns.slice(1),legacy.columns[1]]}),Object.assign(structuredClone(legacy),{columns:legacy.columns.map(column=>({...column,width:1}))})])assert.deepEqual(normalizeWorkspace(invalid),defaultWorkspace());
}

export async function checkRequestOrder(page,root) {
  const traffic=page.locator('traffic-workspace'),header=traffic.locator('#header-request-number'),menu=traffic.locator('#column-actions'),picker=traffic.locator('#traffic-filter-picker');
  const saved=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {rows:globalThis.__workspaceFixture.sessions,preferences:t.preferences,sort:t.sort,filters:t.filters};});
  const settle=()=>page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return !t.queryPending&&t.sessions.length&&t.sessions.every(row=>!row.loading);});
  try {
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[1];
      s.sessions=['100','2','10','1','9','9007199254740993','18446744073709551615'].map((number,index)=>({...seed,id:'request-order-'+index,requestNumber:number,startedAt:100-index,path:'/order/'+number,url:'https://example.test/order/'+number}));
      t.filters=[];t.trafficSearchKind='filter';t.liveFilter='';t.preferences={...t.preferences,columns:t.preferences.columns.map(column=>column.id==='request-number'?{...column,visible:true,pinned:true}:column)};t.clearTrafficSelection();await t.refreshSessions(undefined,true);
    });
    await settle();
    assert.equal(await header.getByRole('button',{name:'Request order',exact:true}).textContent(),'#⌄');
    await header.getByRole('button',{name:'Request order',exact:true}).click();
    assert.match(await menu.textContent(),/Order requests were added to this workspace/);
    await menu.getByRole('button',{name:'Lowest first'}).click();await settle();
    assert.equal(await header.getAttribute('aria-sort'),'ascending');
    assert.deepEqual((await traffic.locator('tr[data-session-id] td[data-column-id="request-number"]').allTextContents()).map(text=>text.trim()),['1','2','9','10','100','9007199254740993','18446744073709551615']);
    await traffic.locator('tr[data-session-id="request-order-5"] .session-link').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='request-order-5');
    const actions=traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true});
    await actions.click();await openActionGroup(traffic.locator('#selected-request-actions'),'Copy');await traffic.locator('#selected-request-actions').getByRole('button',{name:'Copy request summary',exact:true}).click();
    assert.match(await page.evaluate(()=>navigator.clipboard.readText()),/Request #9007199254740993 in this workspace/);
    assert.equal(await actions.evaluate(node=>node===node.getRootNode().activeElement),true);
    // A late query must not take focus or change the selected opaque ID.
    await page.evaluate(()=>{globalThis.__workspaceFixture.deferRefresh=true;globalThis.__workspaceFixture.refreshPending=false;});
    await header.getByRole('button',{name:'Request order',exact:true}).click();await menu.getByRole('button',{name:'Highest first'}).click();
    await page.waitForFunction(()=>globalThis.__workspaceFixture.refreshPending);
    const view=traffic.getByRole('button',{name:'View',exact:true});await view.focus();await page.evaluate(()=>globalThis.__workspaceFixture.releaseRefresh());await settle();
    assert.equal(await view.evaluate(node=>node===node.getRootNode().activeElement),true);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'request-order-5');
    assert.equal(await header.getAttribute('aria-sort'),'descending');
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.sessionScroller.scrollTop=0;t.sessionScrolled();});
    for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
      await page.setViewportSize({width,height});
      const number=traffic.locator('tr[data-session-id="request-order-6"] td[data-column-id="request-number"]');
      assert.equal((await number.textContent()).trim(),'18446744073709551615');
      assert.equal(await number.evaluate(node=>node.scrollWidth<=node.clientWidth),true,'Exact request number clipped');
      await page.screenshot({path:resolve(root,'../../../target/ui-check/request-order-'+width+'.png')});
    }
    await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/request-order-contrast.png')});await page.emulateMedia({forcedColors:'none'});
    const resize=traffic.getByRole('separator',{name:'Resize Request order column',exact:true});
    const beforeResize=await header.evaluate(node=>node.getBoundingClientRect().width);
    await resize.press('ArrowLeft');
    assert.ok(Math.abs(await header.evaluate(node=>node.getBoundingClientRect().width)-beforeResize+10)<1,'Automatic digit sizing overrode manual resize');
    await resize.press('Home');
    await header.getByRole('button',{name:'Request order',exact:true}).focus();await page.keyboard.press('Enter');
    await menu.getByRole('button',{name:'Filter Request order…',exact:true}).click();
    await picker.getByLabel('Filter comparison').selectOption('minimum');
    await picker.getByLabel('Column filter value').fill('18446744073709551616');await picker.getByRole('button',{name:'Apply filter',exact:true}).click();
    assert.equal(await picker.getByLabel('Column filter value').evaluate(node=>node.validity.valid),false);
    await picker.getByLabel('Column filter value').fill('9007199254740993');await picker.getByRole('button',{name:'Apply filter',exact:true}).click();await settle();
    assert.deepEqual((await traffic.locator('tr[data-session-id] td[data-column-id="request-number"]').allTextContents()).map(text=>text.trim()),['18446744073709551615','9007199254740993']);
    // Arrival updates existing references without deriving numbers from row positions.
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions[5].status=201;s.sessions.push({...s.sessions[0],id:'arrived',requestNumber:'9007199254740994'});await t.refreshSessions(undefined,true);});await settle();
    assert.deepEqual((await traffic.locator('tr[data-session-id] td[data-column-id="request-number"]').allTextContents()).map(text=>text.trim()),['18446744073709551615','9007199254740994','9007199254740993']);
    await header.getByRole('button',{name:'Request order',exact:true}).click();await menu.getByRole('button',{name:'Hide column',exact:true}).click();
    await page.waitForFunction(()=>globalThis.__workspaceFixture.workspace.columns.find(column=>column.id==='request-number')?.visible===false);
    await page.reload();await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
    assert.equal(await traffic.locator('#header-request-number').count(),0,'Hidden Request order reappeared after restart');
  } finally {
    await page.emulateMedia({forcedColors:'none'});
    await page.evaluate(async saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;await window.__TAURI_INTERNALS__.invoke('save_workspace_preferences',{preferences:saved.preferences});s.sessions=saved.rows;t.preferences=saved.preferences;t.sort=saved.sort;t.filters=saved.filters;t.liveFilter='';t.clearTrafficSelection();t.followLatest=true;await t.refreshSessions(undefined,true);},saved);
    await page.setViewportSize({width:1280,height:800});
  }
}
