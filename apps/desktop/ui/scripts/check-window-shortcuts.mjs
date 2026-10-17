import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkWindowShortcuts(page,root) {
  const traffic=page.locator('traffic-workspace'),filter=traffic.getByRole('searchbox'),view=traffic.getByRole('button',{name:'View',exact:true});
  const count=command=>page.evaluate(command=>globalThis.__workspaceFixture.calls[command]??0,command);
  for(const viewer of [false,true]) {
    await page.goto('https://workspace.test/'+(viewer?'?viewer':''));
    await traffic.locator('.session-link').first().waitFor();
    // No focused app control is required for a window command.
    await page.keyboard.press('Control+n');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.openedViewer);
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.openedViewer),[]);
    const opens=await count('open_trace_viewer');
    await filter.evaluate(node=>node.dispatchEvent(new KeyboardEvent('keydown',{key:'n',ctrlKey:true,repeat:true,bubbles:true,composed:true,cancelable:true})));
    for(const shortcut of ['Control+Shift+n','Control+Alt+n'])await page.keyboard.press(shortcut);
    assert.equal(await count('open_trace_viewer'),opens,'Repeated or modified shortcut opened a viewer');
    await view.click();await page.keyboard.press('Control+f');
    assert.equal(await filter.evaluate(node=>node===node.getRootNode().activeElement),true);
    assert.equal(await traffic.locator('#table-settings').isVisible(),false,'Filter shortcut left the flyout open');
    await filter.fill('example');await view.focus();await page.keyboard.press('Control+f');
    assert.deepEqual(await filter.evaluate(node=>[node.selectionStart,node.selectionEnd]),[0,7]);
    await page.keyboard.press('Control+Shift+f');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.trafficSearchKind==='contents');
    assert.equal(await filter.inputValue(),'example');
    assert.equal(await filter.evaluate(node=>node===node.getRootNode().activeElement),true);
    await page.keyboard.press('Control+s');
    await traffic.locator('.trace-save-dialog').waitFor();
    const beforeModal={open:await count('open_trace_viewer'),pick:await count('pick_trace_path'),close:await count('close_trace_window')};
    for(const shortcut of ['Control+n','Control+o','Control+f','Control+w','Control+,'])await page.keyboard.press(shortcut);
    assert.deepEqual({open:await count('open_trace_viewer'),pick:await count('pick_trace_path'),close:await count('close_trace_window')},beforeModal);
    assert.equal(await traffic.locator('.trace-save-dialog').isVisible(),true);
    await page.keyboard.press('Escape');
    // A canceled picker is harmless and cannot be opened twice while pending.
    await page.evaluate(()=>globalThis.__workspaceFixture.deferTracePicker=true);
    await page.keyboard.press('Control+o');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.releaseTracePicker);
    const picks=await count('pick_trace_path');await page.keyboard.press('Control+o');assert.equal(await count('pick_trace_path'),picks);
    await page.evaluate(()=>{globalThis.__workspaceFixture.deferTracePicker=false;globalThis.__workspaceFixture.releaseTracePicker();});
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.pickingTrace);
    await page.locator('a[data-view="composer"]').click();
    await page.keyboard.press('Control+o');
    await page.waitForFunction(()=>document.querySelector('app-shell').activeView==='traffic'&&!document.querySelector('app-shell').traffic.pickingTrace);
    assert.equal(await count('pick_trace_path'),picks+1);
    await page.evaluate(()=>globalThis.__workspaceFixture.pickedTrace='C:/captures/shortcut.saz');
    await page.keyboard.press('Control+o');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.lastImport?.path==='C:/captures/shortcut.saz'&&!document.querySelector('app-shell').traffic.pickingTrace);
    await page.keyboard.press('Control+w');
    assert.equal(await count('close_trace_window'),viewer?1:0,'Close shortcut did not respect the window role');
    await page.keyboard.press('Control+,');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').activeView),viewer?'traffic':'settings');
    if(!viewer) {
      // Form saves must still belong to Settings; Traffic cannot capture them.
      const reviews=await count('review_traffic_export');
      await page.keyboard.press('Control+s');assert.equal(await count('review_traffic_export'),reviews);
      await page.locator('a[data-view="traffic"]').click();
    }
    // A child that handles its own Find must win over the application shortcut.
    await view.focus();
    await page.keyboard.down('Control');
    await view.evaluate(node=>node.addEventListener('keydown',event=>event.preventDefault(),{once:true}));
    await page.keyboard.press('f');await page.keyboard.up('Control');
    assert.equal(await view.evaluate(node=>node===node.getRootNode().activeElement),true);
  }
  await page.goto('https://workspace.test/');await traffic.locator('.session-link').first().click();
  for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
    await page.setViewportSize({width,height});await view.click();
    const button=traffic.getByRole('button',{name:'Open new viewer window',exact:true});
    const box=await button.boundingBox();assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=width&&box.y+box.height<=height);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/view-shortcuts-${width}.png`)});
    const opens=await count('open_trace_viewer');await button.click();assert.equal(await count('open_trace_viewer'),opens+1);
    assert.equal(await view.evaluate(node=>node===node.getRootNode().activeElement),true);
  }
  await page.emulateMedia({forcedColors:'active'});await view.click();
  await page.screenshot({path:resolve(root,'../../../target/ui-check/view-shortcuts-forced-colors.png')});await page.keyboard.press('Escape');
  await page.emulateMedia({forcedColors:'none'});
  try {
    await page.evaluate(()=>document.documentElement.style.zoom='2');await view.click();
    const box=await traffic.getByRole('button',{name:'Open new viewer window',exact:true}).boundingBox();
    assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=800&&box.y+box.height<=600,'New viewer action was clipped at 200% zoom');
    await page.screenshot({path:resolve(root,'../../../target/ui-check/view-shortcuts-zoom.png')});await page.keyboard.press('Escape');
  } finally {await page.evaluate(()=>document.documentElement.style.zoom='');}
  await page.setViewportSize({width:1280,height:800});
}
