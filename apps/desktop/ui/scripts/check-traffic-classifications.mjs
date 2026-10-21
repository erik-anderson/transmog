import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficClassifications(page,root) {
  const traffic=page.locator('traffic-workspace'),view=traffic.getByRole('button',{name:'View',exact:true});
  const saved=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;return {preferences:t.preferences,choices:s.extensionColumnChoices,values:s.classificationValues,starts:s.calls.activate_visualizer??0};});
  try {
    await page.locator('a[data-view="traffic"]').click();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;s.extensionColumnChoices=[{key:'office:protocol',name:'Protocol',extensionName:'Office tools',values:['MS-FSSHTTP','Office']}];s.deferExtensionColumns=true;s.emitNative('extensions-changed');});
    await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishExtensionColumns);
    // Ordinary capture updates used to discard an in-flight discovery reply.
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.classifications.invalidate(t.sessions.map(row=>row.id));globalThis.__workspaceFixture.finishExtensionColumns();});
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.classifications.choices.length===1);
    await view.click();await traffic.locator('.list-column-settings summary').click();
    const check=traffic.getByRole('checkbox',{name:'Protocol · Office tools',exact:true});
    await check.waitFor();assert.equal(await check.isChecked(),false);
    assert.equal(await traffic.locator('.extension-column-options legend svg.extension-column-icon').count(),1);
    assert.equal(await traffic.locator('.extension-column-options label svg').count(),0);
    await traffic.locator('.extension-column-options legend').scrollIntoViewIfNeeded();
    await page.screenshot({path:resolve(root,'../../../target/ui-check/classification-chooser.png')});
    const before=await page.evaluate(()=>globalThis.__workspaceFixture.calls.classify_traffic_rows??0);
    await check.check();await page.keyboard.press('Escape');
    await traffic.locator('[data-extension-column="office:protocol"]').first().getByText('MS-FSSHTTP',{exact:true}).waitFor();
    const requests=await page.evaluate(()=>globalThis.__workspaceFixture.classificationRequests);
    assert.ok(requests.length);assert.ok(requests.every(request=>request.ids.length<=100&&request.columns.join()==='office:protocol'));
    assert.ok((await page.evaluate(()=>globalThis.__workspaceFixture.calls.classify_traffic_rows))>before);
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer??0),saved.starts,'A static column started extension code');
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;globalThis.__ordinaryCells=t.sessions.map(row=>({id:row.id,cells:row.cells}));s.classificationValues=['Updated headers'];t.trafficTable.focus();t.classifications.invalidate(t.sessions.map(row=>row.id));t.classifications.schedule(t.sessions.map(row=>row.id));});
    await traffic.locator('[data-extension-column="office:protocol"]').first().getByText('Updated headers',{exact:true}).waitFor();
    assert.equal(await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;const unchanged=globalThis.__ordinaryCells.every(previous=>t.sessions.find(row=>row.id===previous.id)?.cells===previous.cells);delete globalThis.__ordinaryCells;return unchanged&&t.getRootNode().activeElement===t.trafficTable;}),true,'A label reply rebuilt ordinary traffic cells or moved focus');
    // Same exchange IDs with newer header evidence must reject the earlier reply.
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.classificationValues=['Old'];s.deferClassifications=true;t.classifications.invalidate();t.scheduleViewport();});
    await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishClassifications);
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.classificationValues=['New <img src=x onerror=alert(1)>'];await t.refreshSessions(undefined,true);});
    const pending=await page.evaluate(()=>globalThis.__workspaceFixture.classificationRequests.length);
    await page.evaluate(()=>globalThis.__workspaceFixture.finishClassifications());
    await traffic.locator('[data-extension-column="office:protocol"]').first().getByText('New <img src=x onerror=alert(1)>',{exact:true}).waitFor();
    assert.ok((await page.evaluate(()=>globalThis.__workspaceFixture.classificationRequests.length))>pending,'The newest viewport was not retried after stale work');
    assert.equal(await traffic.locator('[data-extension-column="office:protocol"] img').count(),0);
    assert.equal(await traffic.locator('[data-extension-column="office:protocol"]').first().textContent(),'New <img src=x onerror=alert(1)>');
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture, t = document.querySelector('app-shell').traffic;
      globalThis.__ordinaryCells = t.sessions.map(row => ({id: row.id, cells: row.cells}));
      t.trafficTable.focus();
      s.classificationValues = ['Refreshed metadata'];
      s.emitNative('extensions-changed');
    });
    await traffic.locator('[data-extension-column="office:protocol"]').first().getByText('Refreshed metadata', {exact: true}).waitFor();
    assert.equal(await page.evaluate(() => {
      const t = document.querySelector('app-shell').traffic;
      const unchanged = globalThis.__ordinaryCells.every(previous => t.sessions.find(row => row.id === previous.id)?.cells === previous.cells);
      delete globalThis.__ordinaryCells;
      return unchanged && t.getRootNode().activeElement === t.trafficTable;
    }), true, 'Catalog refresh rebuilt ordinary Traffic cells or moved focus');
    const heading=traffic.getByRole('button',{name:'Filter Protocol · Extension: Office tools',exact:true});
    assert.equal(await heading.locator('svg[aria-hidden="true"]').count(),1);
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.classificationsById=Object.fromEntries(s.sessions.map((row,index)=>[row.id,{'office:protocol':index===0?['MS-FSSHTTP','Office']:[]}]))});
    const firstId=await page.evaluate(()=>globalThis.__workspaceFixture.sessions[0].id);
    await heading.click();
    const picker=traffic.locator('#traffic-filter-picker');
    await picker.getByRole('checkbox',{name:/MS-FSSHTTP/}).waitFor();
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.pickerLoading);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.pickerCounts.get('text:ms-fsshttp')),1);
    await page.screenshot({path:resolve(root,'../../../target/ui-check/classification-filter.png')});
    await picker.getByRole('checkbox',{name:/MS-FSSHTTP/}).check();
    await picker.getByRole('button',{name:'Apply filter',exact:true}).click();
    await page.waitForFunction(id=>{const t=document.querySelector('app-shell').traffic;return t.totalMatched===1&&t.virtualList.ids[0]===id;},firstId);
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastQuery.filterGroups.length),0,'Dynamic keys leaked into built-in dimensions');
    assert.equal(await traffic.locator('.filter-chip').filter({hasText:'Protocol · Office tools'}).count(),1);
    await heading.click();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.pickerLoading);
    assert.ok((await page.evaluate(()=>document.querySelector('app-shell').traffic.pickerCounts.get('unavailable')))>0,'Own filter constrained No match counts');
    await picker.getByRole('checkbox',{name:/MS-FSSHTTP/}).uncheck();
    await picker.getByRole('checkbox',{name:/No match/}).check();
    await picker.getByRole('button',{name:'Apply filter',exact:true}).click();
    await page.waitForFunction(id=>{const t=document.querySelector('app-shell').traffic;return t.totalMatched>0&&!t.virtualList.indexes.has(id);},firstId);
    // A catalog change invalidates counts for every field when an extension
    // filter is active, including when the replacement Traffic query fails.
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture, t = document.querySelector('app-shell').traffic;
      s.deferFacets = true;
      t.openFilterPicker('host');
    });
    await page.waitForFunction(() => globalThis.__workspaceFixture.facetPending);
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.reviewColumnChoices = s.extensionColumnChoices;
      s.extensionColumnChoices = [];
      s.queryError = true;
      s.facetError = true;
      s.emitNative('extensions-changed');
    });
    await page.waitForFunction(() => document.querySelector('app-shell').traffic.classifications.choices.length === 0);
    await page.evaluate(async () => {
      const s = globalThis.__workspaceFixture, t = document.querySelector('app-shell').traffic;
      await t.queryPending;
      s.releaseFacets();
      await t.facetPending;
    });
    assert.equal(await page.evaluate(() => document.querySelector('app-shell').traffic.pickerLoading), false, 'Catalog change left an unrelated field loading forever');
    await picker.getByText(/Counts unavailable:/).waitFor();
    await page.keyboard.press('Escape');
    await page.evaluate(() => {
      const s = globalThis.__workspaceFixture;
      s.extensionColumnChoices = s.reviewColumnChoices;
      delete s.reviewColumnChoices;
      s.queryError = false;
      s.facetError = false;
      s.emitNative('extensions-changed');
    });
    await page.waitForFunction(() => document.querySelector('app-shell').traffic.extensionTableColumns.length === 1);
    await traffic.getByRole('button',{name:/Remove Protocol · Office tools/}).click();
    await page.evaluate(()=>{delete globalThis.__workspaceFixture.classificationsById;});
    await page.evaluate(async()=>document.querySelector('app-shell').traffic.refreshSessions(undefined,true));

    for(const width of [1440,800]) {
      await page.setViewportSize({width,height:720});
      await traffic.locator('.table-wrap').evaluate(element=>element.scrollLeft=element.scrollWidth);
      await page.screenshot({path:resolve(root,`../../../target/ui-check/classification-${width}.png`)});
    }
    await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/classification-contrast.png')});await page.emulateMedia({forcedColors:'none'});
    await heading.click();await page.keyboard.press('Escape');
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.extensionColumnChoices=[];s.emitNative('extensions-changed');});
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.extensionTableColumns.length===0);
    assert.equal(await traffic.locator('[data-extension-column]').count(),0);
    await page.evaluate(()=>document.querySelector('app-shell').traffic.openFilters(new Event('click')));
    assert.equal(await traffic.getByRole('combobox',{name:'Filter field',exact:true}).inputValue(),'traffic-type','Removed extension left Filters on a missing field');
    await page.keyboard.press('Escape');
    const listeners=await page.evaluate(()=>globalThis.__workspaceFixture.extensionListenerCount());
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;t.classifications.disconnect();s.holdCommand='plugin:event|listen';void t.classifications.connect();});
    await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishHeldCommand);
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.classifications.disconnect();void t.classifications.connect();globalThis.__workspaceFixture.finishHeldCommand();});
    await page.waitForFunction(count=>globalThis.__workspaceFixture.extensionListenerCount()===count,listeners);
    await page.locator('a[data-view="settings"]').click();
    await page.waitForFunction(()=>globalThis.__workspaceFixture.extensionListenerCount()===0);
  } finally {
    await page.emulateMedia({forcedColors:'none'});await page.setViewportSize({width:1280,height:800});
    await page.evaluate(saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.extensionColumnChoices=saved.choices;s.classificationValues=saved.values;s.deferClassifications=false;s.finishClassifications?.();s.finishExtensionColumns?.();s.releaseFacets?.();delete s.classificationsById;delete s.reviewColumnChoices;s.queryError=false;s.facetError=false;t.preferences=saved.preferences;t.classifications.invalidate();},saved);
    await page.locator('a[data-view="traffic"]').click();
  }
}
