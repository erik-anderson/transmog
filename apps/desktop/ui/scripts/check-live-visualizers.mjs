import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkLiveVisualizers(page,root,visualizer,selector,tree) {
  const live=visualizer.getByRole('checkbox',{name:'Live updates',exact:true});
  assert.equal(await live.count(),0,'An undeclared live capability was offered');
  await selector.selectOption('auto');
  await page.evaluate(()=>{globalThis.__workspaceFixture.visualizerLiveSupported=true;});
  await selector.selectOption({label:'Protocol structure'});
  await live.waitFor();assert.equal(await live.isChecked(),false);
  const idlePolls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live??0);
  await page.waitForTimeout(600);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live??0),idlePolls,'Snapshot mode polled for traffic');
  await live.check();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.visualizerStarts.at(-1).live===true);
  await page.waitForFunction(()=>Object.values(globalThis.__workspaceFixture.visualizerActive).some(view=>view.live.enabled));
  await tree.focus();await page.keyboard.press('End');
  await visualizer.getByRole('combobox',{name:'Structure view',exact:true}).selectOption('text');
  await tree.focus();
  const position=await tree.evaluate(element=>({top:element.scrollTop,selected:element.getAttribute('aria-activedescendant')}));
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);s.nextLiveDocument.roots.at(-1).value={kind:'string',value:'Updated last field'};});
  await visualizer.locator('pre').filter({hasText:'Updated last field'}).waitFor();
  assert.deepEqual(await tree.evaluate(element=>({top:element.scrollTop,selected:element.getAttribute('aria-activedescendant')})),position,'Live delivery moved the inspected structure');
  assert.equal(await tree.evaluate(element=>element.matches(':focus')),true,'Live delivery stole focus');
  assert.equal(await visualizer.getByRole('combobox',{name:'Structure view',exact:true}).inputValue(),'text');

  const liveStarts=await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer);
  await page.evaluate(()=>{
    const s=globalThis.__workspaceFixture,document=structuredClone(s.visualizerDocument),last=document.roots.pop();
    last.value={kind:'string',value:'Moved last field'};
    document.roots.unshift({id:'new-group',name:'New group',value:{kind:'object'},children:[last],spans:[]});
    s.nextLiveDocument=document;
  });
  await visualizer.locator('pre').filter({hasText:'Moved last field'}).waitFor();
  const moved=tree.getByRole('treeitem',{selected:true});
  assert.equal(await moved.count(),1,'Live restructuring left the selected field hidden under a new ancestor');
  assert.ok((await moved.textContent()).includes('Field 4999'));
  await tree.getByRole('button',{name:'Collapse New group',exact:true}).waitFor();
  assert.equal(await tree.evaluate(element=>element===element.getRootNode().activeElement),true,'Live restructuring stole tree focus');
  assert.equal(await moved.evaluate(element=>{
    const row=element.getBoundingClientRect(),viewport=element.parentElement.parentElement.getBoundingClientRect();
    return row.top>=viewport.top-1&&row.bottom<=viewport.bottom+1;
  }),true,'The moved field remained outside the tree viewport');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer),liveStarts,'Revealing a moved field restarted the worker');
  await tree.evaluate(element=>{element.scrollTop=80000;element.dispatchEvent(new Event('scroll'));});
  const manualScroll=await tree.evaluate(element=>element.scrollTop);
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);});
  await visualizer.locator('.visualizer-value pre').filter({hasText:/^4999$/}).waitFor();
  assert.equal(await tree.evaluate(element=>element.scrollTop),manualScroll,'Live restructuring overrode a manual scroll away from the selection');

  await visualizer.locator('.visualizer-value pre').focus();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);s.nextLiveDocument.roots.pop();});
  await visualizer.getByRole('region',{name:'Selected structure',exact:true}).waitFor({state:'hidden'});
  assert.equal(await tree.evaluate(element=>element===element.getRootNode().activeElement),true,'Removing the inspected field left focus in hidden details');
  assert.equal(await tree.getByRole('treeitem',{selected:true}).count(),1,'Removing a selected field left the replacement selection outside the tree window');
  await page.keyboard.press('ArrowDown');
  await visualizer.locator('.visualizer-node-heading > strong').filter({hasText:'Field 0'}).waitFor();
  await selector.focus();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);s.nextLiveDocument.roots.shift();});
  await visualizer.getByRole('region',{name:'Selected structure',exact:true}).waitFor({state:'hidden'});
  assert.equal(await selector.evaluate(element=>element===element.getRootNode().activeElement),true,'Removing a field stole focus from outside its details');
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);});
  await page.waitForFunction(()=>!globalThis.__workspaceFixture.nextLiveDocument);

  await page.evaluate(()=>{globalThis.__workspaceFixture.holdVisualizerExpansion=true;});
  await tree.focus();await page.keyboard.press('Home');await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowRight');
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishVisualizerExpansion);
  try {
    const expanding=await page.evaluate(()=>globalThis.__workspaceFixture.calls.expand_visualizer);
    await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowRight');
    assert.equal(await tree.getByRole('button',{name:'Expand Field 2',exact:true}).count(),1,'A busy deferred branch was marked expanded without fetching its children');
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.expand_visualizer),expanding,'A second deferred expansion ran concurrently');
    await page.keyboard.press('ArrowUp');
  } finally {
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.finishVisualizerExpansion();delete s.finishVisualizerExpansion;});
  }
  await tree.getByText('Nested text',{exact:true}).waitFor();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.expandedVisualizerDocument);s.nextLiveDocument.roots[1].children[0].value.value='Updated nested value';});
  await tree.getByText('Updated nested value',{exact:true}).waitFor();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);});
  await tree.getByRole('button',{name:'Expand Field 1',exact:true}).waitFor();
  await tree.getByRole('button',{name:'Expand Field 1',exact:true}).click();
  await tree.getByText('Nested text',{exact:true}).waitFor();

  // A new deferred token requires explicit expansion; do not reparse many
  // expanded structures automatically on every live event.
  // Off waits for a pending update, then removes admission without reopening.
  await page.evaluate(()=>{globalThis.__workspaceFixture.holdLivePoll=true;});
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishLivePoll);
  const starts=await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer);
  await live.uncheck();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.finishLivePoll();delete s.finishLivePoll;});
  await page.waitForFunction(()=>globalThis.__workspaceFixture.liveStops?.length>0);
  const stoppedPolls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live);
  await page.waitForTimeout(600);
  assert.equal(await live.isChecked(),false);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live),stoppedPolls,'Live continued polling after Off');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer),starts,'Off restarted the worker');

  // An old Off reply must not disable a new explicitly opened live worker.
  await live.check();await live.click({trial:true});
  const stopping=await visualizer.evaluate(element=>element.view.handle.instanceId);
  await page.evaluate(()=>{globalThis.__workspaceFixture.holdLiveStop=true;});
  await live.uncheck();
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishLiveStop);
  await live.check();await live.click({trial:true});
  const restarted=await visualizer.evaluate(element=>element.view.handle.instanceId);
  assert.notEqual(restarted,stopping);
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.finishLiveStop();delete s.finishLiveStop;});
  await page.waitForTimeout(80);
  assert.equal(await live.isChecked(),true,'A delayed Off reply disabled a newer live visualization');
  assert.equal(await visualizer.evaluate(element=>element.view.live.enabled),true,'An old reply overwrote the new worker status');
  const resumedPolls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live);
  await page.waitForFunction(previous=>globalThis.__workspaceFixture.calls.poll_visualizer_live>previous,resumedPolls);
  await live.uncheck();
  await page.waitForFunction(()=>Object.values(globalThis.__workspaceFixture.visualizerActive).every(view=>!view.live.enabled));

  // Retained immutable bytes keep the user's range and scroll across a document update.
  await live.check();await page.waitForFunction(()=>Object.values(globalThis.__workspaceFixture.visualizerActive).some(view=>view.live.enabled));
  await tree.focus();await page.keyboard.press('Home');await page.keyboard.press('ArrowDown');
  if(!await visualizer.locator('.visualizer-ranges').evaluate(element=>element.open))await visualizer.getByText('Source ranges (1)',{exact:true}).click();
  await visualizer.getByRole('button',{name:'Original response body · offset 9007199254741000 · 8 bytes',exact:true}).click();
  await visualizer.locator('hex-viewer').getByText('8 bytes selected',{exact:false}).waitFor();
  const preview=await visualizer.locator('hex-viewer').evaluate(element=>({first:element.rangeStart,last:element.rangeEnd}));
  const polls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live);
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextLiveDocument=structuredClone(s.visualizerDocument);s.nextLiveDocument.status='complete';s.nextLiveDocument.summary='Updated summary';});
  await page.waitForFunction(previous=>globalThis.__workspaceFixture.calls.poll_visualizer_live>previous,polls);
  await page.waitForFunction(()=>!globalThis.__workspaceFixture.nextLiveDocument);
  assert.equal(await visualizer.evaluate(element=>element.visualizerStatus),'complete');
  assert.deepEqual(await visualizer.locator('hex-viewer').evaluate(element=>({first:element.rangeStart,last:element.rangeEnd})),preview);

  await page.evaluate(()=>{globalThis.__workspaceFixture.liveMissedHints='2';});
  await visualizer.getByText('Some updates may be missing. Reopen the visualizer to rebuild context.',{exact:true}).waitFor();
  assert.equal(await visualizer.evaluate(element=>element.visualizerStatus),'partial','An empty poll reported missed traffic but retained a Complete badge');
  for(const [width,height] of [[1440,900],[800,600]]){
    await page.setViewportSize({width,height});
    assert.equal(await visualizer.evaluate(element=>element.scrollWidth<=element.clientWidth+1),true);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/visualizer-live-${width}.png`)});
  }
  await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/visualizer-live-forced-colors.png')});await page.emulateMedia({forcedColors:'none'});
  await page.setViewportSize({width:1440,height:900});
  const beforeReopen=await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer);
  await visualizer.getByRole('button',{name:'Reopen visualizer',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.liveMissedHints='0';});
  await page.waitForFunction(previous=>globalThis.__workspaceFixture.calls.activate_visualizer>previous,beforeReopen);
  await live.waitFor();
  await visualizer.getByText('Some updates may be missing. Reopen the visualizer to rebuild context.',{exact:true}).waitFor({state:'hidden'});
  await page.evaluate(()=>{globalThis.__workspaceFixture.liveError=true;});
  await visualizer.getByText('Synthetic live parser failed',{exact:true}).waitFor();
  await visualizer.getByText('Showing the last result.',{exact:true}).waitFor();
  assert.equal(await visualizer.locator('.visualizer-heading > strong').count(),1);
  assert.equal(await visualizer.locator('hex-viewer').count(),0);
  const failed=await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer);
  await page.waitForTimeout(600);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.activate_visualizer),failed,'A failed feed restarted itself');
  await page.evaluate(()=>{globalThis.__workspaceFixture.liveError=false;});
  await visualizer.getByRole('button',{name:'Retry visualizer',exact:true}).click();
  await page.waitForFunction(()=>Object.values(globalThis.__workspaceFixture.visualizerActive).some(view=>view.live.enabled));

  // Navigation reaps the feed; late delivery cannot restore data or restart polling.
  await page.evaluate(()=>{globalThis.__workspaceFixture.holdLivePoll=true;});
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishLivePoll);
  await page.locator('a[data-view="settings"]').click();
  await page.waitForFunction(()=>Object.keys(globalThis.__workspaceFixture.visualizerActive).length===0);
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.finishLivePoll();delete s.finishLivePoll;});
  await page.waitForTimeout(350);
  assert.equal(await visualizer.locator('.visualizer-heading > strong').count(),0);
  const hiddenPolls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live);
  await page.waitForTimeout(350);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live),hiddenPolls);
  await page.locator('a[data-view="traffic"]').click();
  await visualizer.getByRole('button',{name:'Open visualizer',exact:true}).click();
  await live.waitFor();assert.equal(await live.isChecked(),false,'Reopening a hidden pane restored Live without consent');

  await selector.selectOption('auto');
  await page.evaluate(()=>{globalThis.__workspaceFixture.visualizerLiveAvailable=false;});
  await selector.selectOption({label:'Protocol structure'});
  await visualizer.locator('.visualizer-heading > strong').waitFor();
  assert.equal(await live.count(),0,'Imported/unavailable traffic displayed Live updates');
  await selector.selectOption('auto');
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;delete s.visualizerLiveAvailable;s.visualizerDocument.updates='final';});
  await selector.selectOption({label:'Protocol structure'});
  await visualizer.locator('.visualizer-heading > strong').waitFor();
  assert.equal(await live.count(),0,'A final snapshot displayed Live updates');
  await selector.selectOption('auto');
  await page.evaluate(()=>{delete globalThis.__workspaceFixture.visualizerDocument.updates;});
  await selector.selectOption({label:'Protocol structure'});
  await live.check();
  await page.waitForFunction(()=>Object.values(globalThis.__workspaceFixture.visualizerActive).some(view=>view.live.enabled));
  await live.focus();
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.liveMissedHints='2';s.nextLiveDocument=structuredClone(s.visualizerDocument);s.nextLiveDocument.status='complete';s.nextLiveDocument.updates='final';});
  await live.waitFor({state:'detached'});
  assert.equal(await tree.evaluate(element=>element===element.getRootNode().activeElement),true,'Finalization left keyboard focus on a removed Live updates control');
  await visualizer.getByText('Some updates may be missing. Reopen the visualizer to rebuild context.',{exact:true}).waitFor();
  assert.equal(await visualizer.evaluate(element=>element.view.document.status),'partial','A finalized result lost the host partial status after missed traffic');
  const finalPolls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live);
  await page.waitForTimeout(600);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.poll_visualizer_live),finalPolls,'A final view kept polling');
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.nextExpandedDocument=structuredClone(s.expandedVisualizerDocument);s.nextExpandedDocument.status='complete';});
  await tree.focus();await page.keyboard.press('Home');await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowDown');await page.keyboard.press('ArrowRight');
  await tree.getByText('Nested text',{exact:true}).waitFor();
  assert.equal(await live.count(),0,'Deferred expansion resurrected a final view feed');
  assert.equal(await visualizer.evaluate(element=>element.view.document.status),'partial','Expansion changed a result with missing live data to complete');
  await selector.selectOption('auto');
  await selector.selectOption({label:'Protocol structure'});
  await live.waitFor();
  assert.equal(await live.isChecked(),false,'A fresh activation inherited the old update policy');
  await selector.selectOption('auto');
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;delete s.visualizerLiveAvailable;delete s.visualizerLiveSupported;delete s.liveMissedHints;});
  await selector.selectOption({label:'Protocol structure'});
  await visualizer.locator('.visualizer-heading > strong').waitFor();
  assert.equal(await live.count(),0);
}
