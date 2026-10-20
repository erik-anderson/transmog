import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficClassifications(page,root) {
  const traffic=page.locator('traffic-workspace'),view=traffic.getByRole('button',{name:'View',exact:true});
  const saved=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,s=globalThis.__workspaceFixture;return {preferences:t.preferences,choices:s.extensionColumnChoices,values:s.classificationValues,starts:s.calls.activate_visualizer??0};});
  try {
    await page.locator('a[data-view="traffic"]').click();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;s.extensionColumnChoices=[{key:'office:protocol',name:'Office protocol',extensionName:'Office tools'}];document.querySelector('app-shell').traffic.refreshExtensionColumns();});
    await view.click();await traffic.locator('.list-column-settings summary').click();
    const check=traffic.getByRole('checkbox',{name:'Office protocol · Office tools',exact:true});
    await check.waitFor();assert.equal(await check.isChecked(),false);
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
    for(const width of [1440,800]) {
      await page.setViewportSize({width,height:720});
      await traffic.locator('.table-wrap').evaluate(element=>element.scrollLeft=element.scrollWidth);
      await page.screenshot({path:resolve(root,`../../../target/ui-check/classification-${width}.png`)});
    }
    await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/classification-contrast.png')});await page.emulateMedia({forcedColors:'none'});
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.extensionColumnChoices=[];s.emitNative('extensions-changed');});
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.extensionTableColumns.length===0);
    assert.equal(await traffic.locator('[data-extension-column]').count(),0);
    const listeners=await page.evaluate(()=>globalThis.__workspaceFixture.extensionListenerCount());
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;t.classifications.disconnect();s.holdCommand='plugin:event|listen';void t.classifications.connect();});
    await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishHeldCommand);
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.classifications.disconnect();void t.classifications.connect();globalThis.__workspaceFixture.finishHeldCommand();});
    await page.waitForFunction(count=>globalThis.__workspaceFixture.extensionListenerCount()===count,listeners);
    await page.locator('a[data-view="settings"]').click();
    await page.waitForFunction(()=>globalThis.__workspaceFixture.extensionListenerCount()===0);
  } finally {
    await page.emulateMedia({forcedColors:'none'});await page.setViewportSize({width:1280,height:800});
    await page.evaluate(saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.extensionColumnChoices=saved.choices;s.classificationValues=saved.values;s.deferClassifications=false;s.finishClassifications?.();t.preferences=saved.preferences;t.classifications.invalidate();},saved);
    await page.locator('a[data-view="traffic"]').click();
  }
}
