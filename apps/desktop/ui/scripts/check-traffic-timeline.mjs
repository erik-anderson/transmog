import {openActionGroup} from './check-action-menus.mjs';
import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficTimeline(page,root) {
  await page.setViewportSize({width:1440,height:900});
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  const saved=await page.evaluate(()=>({rows:globalThis.__workspaceFixture.sessions,preferences:document.querySelector('app-shell').traffic.preferences,baseline:document.querySelector('app-shell').traffic.comparisonBaseline,baselineLabel:document.querySelector('app-shell').traffic.comparisonBaselineLabel}));
  const traffic=page.locator('traffic-workspace'),timeline=traffic.locator('traffic-timeline');
  const toggle=traffic.getByRole('button',{name:'Timeline',exact:true});
  const settle=()=>page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.timelineMode&&!t.timelineBusy&&!t.timelinePending&&t.timelineEntries.length>0;});
  const show=async enabled=>{if(await toggle.getAttribute('aria-pressed')!==String(enabled))await toggle.click();if(enabled)await settle();};
  try {
    const search=traffic.getByLabel('Filter or search traffic',{exact:true});
    await search.fill('example');await search.press('Control+a');await search.press('Backspace');
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[0],base=1800000000000;
      s.sessions=[['slow',0,1200],['fast',100,50],['long',300,700],['zero',600,0],['failed',700,200],['unknown',null,null]].map(([id,start,duration],index)=>({...seed,id:'tl-'+id,requestNumber:String(index+1),method:'GET',path:'/scenario/'+id,url:'https://example.test/scenario/'+id,startedAt:start==null?0:base+start,durationMs:duration,responseOffset:id==='slow'?400000:null,terminal:id==='failed'?'failed':id==='unknown'?'incomplete':'completed',contentType:'text/plain',traceId:id==='unknown'?'imported-fixture':null}));
      window.clearTimeout(t.searchTimer);t.searchText='';t.liveFilter='';t.preferences={...t.preferences,layout:'stacked',listSplit:55};t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);
    });
    const bodies=await page.evaluate(()=>globalThis.__workspaceFixture.calls.inspect_body??0);
    assert.equal(await toggle.getAttribute('aria-pressed'),'false');
    const toggleSize=await toggle.boundingBox();assert.ok(toggleSize.width<=40&&toggleSize.height<=40,'Timeline toggle takes more than a compact icon button');
    await toggle.focus();await page.keyboard.press('Space');await settle();
    assert.equal(await toggle.getAttribute('aria-pressed'),'true');assert.equal(await toggle.evaluate(el=>el===el.getRootNode().activeElement),true,'Turning Timeline on moved focus off its toggle');
    await page.keyboard.press('Enter');assert.equal(await toggle.getAttribute('aria-pressed'),'false');
    assert.equal(await toggle.evaluate(el=>el===el.getRootNode().activeElement),true,'Turning Timeline off lost toggle focus');
    await show(true);await timeline.locator('.timeline-overview > svg').focus();
    const helpButton=timeline.getByRole('button',{name:'Timeline help',exact:true});
    const help=timeline.getByRole('dialog',{name:'Timeline help',exact:true});
    const checkHelpShortcuts=async()=>{
      const state=()=>page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;return {selected:[...t.trafficSelection.ids].sort(),matched:t.totalMatched,filter:t.searchInput.value,undo:t.trafficUndo.length,removals:globalThis.__workspaceFixture.calls.remove_traffic_entries??0};});
      const before=await state();
      for(const name of ['Close Timeline help','Refresh timeline']){
        const control=help.getByRole('button',{name,exact:true});await control.focus();
        for(const key of ['Control+a','Delete','Control+z','Control+y','Control+Shift+z']){
          await page.keyboard.press(key);
          assert.deepEqual(await state(),before,`${key} in ${name} changed Traffic`);
          assert.equal(await control.evaluate(el=>el===el.getRootNode().activeElement),true,`${key} moved focus out of ${name}`);
        }
      }
      assert.equal(await help.isVisible(),true,'Traffic shortcuts closed help');
    };
    const chartTop=await timeline.locator('.timeline-viewport').evaluate(el=>el.getBoundingClientRect().top);
    await helpButton.hover();await help.waitFor({state:'visible'});
    await page.keyboard.press('Escape');await help.waitFor({state:'hidden'});
    assert.equal(await timeline.locator('.timeline-overview > svg').evaluate(el=>el===el.getRootNode().activeElement),true,'Hover help dismissal moved keyboard focus');
    await traffic.getByLabel('Filter or search traffic',{exact:true}).hover();await helpButton.hover();await help.waitFor({state:'visible'});
    await help.hover();assert.equal(await help.isVisible(),true,'Help disappeared while moving into it');
    assert.equal(await timeline.locator('.timeline-viewport').evaluate(el=>el.getBoundingClientRect().top),chartTop,'Help moved the chart');
    await traffic.getByLabel('Filter or search traffic',{exact:true}).hover();await help.waitFor({state:'hidden'});
    await helpButton.focus();await help.waitFor({state:'visible'});await page.keyboard.press('Tab');
    assert.equal(await help.getByRole('button',{name:'Close Timeline help',exact:true}).evaluate(el=>el===el.getRootNode().activeElement),true);
    await checkHelpShortcuts();
    await page.keyboard.press('Escape');await help.waitFor({state:'hidden'});
    assert.equal(await helpButton.evaluate(el=>el===el.getRootNode().activeElement),true,'Closing help lost keyboard focus');
    await page.keyboard.press('Shift+Tab');await page.keyboard.press('Tab');await help.waitFor({state:'visible'});
    await page.keyboard.press('Tab');await page.keyboard.press('Tab');await page.keyboard.press('Tab');await help.waitFor({state:'hidden'});
    await helpButton.click();await traffic.getByLabel('Filter or search traffic',{exact:true}).hover();
    await page.waitForTimeout(250);assert.equal(await help.isVisible(),true,'Clicked help did not stay open');
    const queries=await page.evaluate(()=>globalThis.__workspaceFixture.calls.traffic_timeline_rows??0);
    await help.getByRole('button',{name:'Refresh timeline',exact:true}).focus();await page.keyboard.press('Enter');
    await page.waitForFunction(queries=>(globalThis.__workspaceFixture.calls.traffic_timeline_rows??0)>queries,queries);
    await help.getByRole('button',{name:'Close Timeline help',exact:true}).click();await help.waitFor({state:'hidden'});
    await helpButton.click();await traffic.getByLabel('Filter or search traffic',{exact:true}).click();await help.waitFor({state:'hidden'});
    await helpButton.click();await toggle.focus();await page.keyboard.press('Space');await help.waitFor({state:'hidden'});
    assert.equal(await toggle.getAttribute('aria-pressed'),'false');await toggle.hover();await page.keyboard.press('Space');await settle();
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.timelineEntries.length),6);
    assert.equal(await timeline.locator('#timeline-tl-slow .timeline-span').evaluate(el=>el.namespaceURI),'http://www.w3.org/2000/svg');
    assert.equal(Number(await timeline.locator('#timeline-tl-slow .timeline-span').getAttribute('width')),1000);
    assert.equal(await timeline.locator('.timeline-density').evaluate(el=>el.namespaceURI),'http://www.w3.org/2000/svg');
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.inspect_body??0),bodies,'Opening the timeline read bodies');
    await timeline.locator('#timeline-tl-slow').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='tl-slow');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Inspection changed timeline scope');
    await timeline.locator('#timeline-tl-slow').focus();await page.keyboard.press('ArrowDown');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='tl-fast');
    assert.equal(await traffic.locator('.selected-url').textContent(),'https://example.test/scenario/fast');
    // Inspector actions follow the inspected request without narrowing Timeline.
    await traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true}).click();
    const actions=traffic.locator('#selected-request-actions');
    assert.equal(await actions.getByRole('button',{name:'Set as comparison baseline',exact:true}).isEnabled(),true);
    await openActionGroup(actions,'Compare');await actions.getByRole('button',{name:'Set as comparison baseline',exact:true}).click();
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.comparisonBaseline),'tl-fast');
    await traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true}).click();
    await openActionGroup(actions,'Copy');await actions.getByRole('button',{name:'Copy request summary',exact:true}).click();
    assert.match(await page.evaluate(()=>navigator.clipboard.readText()),/scenario\/fast/);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0);
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.clearComparisonBaseline();t.diagnostic='';});
    assert.equal(await timeline.locator('#timeline-tl-fast .timeline-request > span').textContent(),'#2 GET /scenario/fast');
    assert.match(await timeline.locator('#timeline-tl-fast small').textContent(),/example.test/);
    const checkAxis=async()=>{
      const geometry=await timeline.evaluate(el=>{const axis=el.querySelector('.timeline-ticks').getBoundingClientRect();return [...el.querySelectorAll('.timeline-row > svg')].map(svg=>{const box=svg.getBoundingClientRect();return {left:box.left-axis.left,right:box.right-axis.right};});});
      for(const row of geometry)assert.ok(Math.abs(row.left)<1&&Math.abs(row.right)<1,'Request bars do not share the time axis');
    };
    await checkAxis();
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'side-by-side',listSplit:45};});
    assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>0));
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-aside.png')});
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'stacked',listSplit:55};});
    for(const [width,height] of [[1440,900],[960,720],[800,600]]){
      await page.setViewportSize({width,height});
      assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>=96),'Compact layout cannot show several requests');
      await checkAxis();
      await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-'+width+'.png')});
      await helpButton.click();await help.waitFor({state:'visible'});
      const bounds=await help.boundingBox();
      assert.ok(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=width+1&&bounds.y+bounds.height<=height+1,'Timeline help extends outside the window');
      assert.equal(await help.evaluate(el=>el.scrollWidth<=el.clientWidth),true,'Timeline help clips text horizontally');
      await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-help-'+width+'.png')});
      await help.getByRole('button',{name:'Close Timeline help',exact:true}).click();
    }
    const paint=()=>page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    await page.emulateMedia({forcedColors:'active'});await paint();await page.screenshot({animations:'disabled',path:resolve(root,'../../../target/ui-check/traffic-timeline-contrast.png')});
    await helpButton.click();await page.screenshot({animations:'disabled',path:resolve(root,'../../../target/ui-check/traffic-timeline-help-contrast.png')});await help.getByRole('button',{name:'Close Timeline help',exact:true}).click();
    await page.emulateMedia({forcedColors:'none',colorScheme:'dark'});await paint();await page.screenshot({animations:'disabled',path:resolve(root,'../../../target/ui-check/traffic-timeline-dark.png')});
    await helpButton.click();await page.screenshot({animations:'disabled',path:resolve(root,'../../../target/ui-check/traffic-timeline-help-dark.png')});await help.getByRole('button',{name:'Close Timeline help',exact:true}).click();
    await page.emulateMedia({colorScheme:'light'});await paint();
    await page.evaluate(()=>document.documentElement.style.zoom='1.25');
    try{
      assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>0));await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-zoom.png')});
      await helpButton.click();
      const bounds=await help.boundingBox();assert.ok(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=801&&bounds.y+bounds.height<=601,'Zoomed help extends outside the window');
      const trigger=await helpButton.boundingBox();assert.ok(bounds.y>=trigger.y+trigger.height-1||bounds.y+bounds.height<=trigger.y+1,'Zoomed help covers its trigger');
      await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-help-zoom.png')});await help.getByRole('button',{name:'Close Timeline help',exact:true}).click();
    }finally{await page.evaluate(()=>document.documentElement.style.zoom='');}
    await page.setViewportSize({width:1440,height:900});
    await traffic.getByRole('button',{name:'View',exact:true}).click();
    const settings=traffic.locator('#table-settings');
    assert.equal(await toggle.getAttribute('aria-pressed'),'true');
    assert.equal(await settings.getByText('List columns',{exact:true}).count(),0,'Timeline exposes inactive List settings');
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-settings.png')});
    await settings.getByLabel('Compact rows',{exact:true}).uncheck();await page.keyboard.press('Escape');
    assert.equal(await timeline.locator('.timeline-row').first().evaluate(el=>el.getBoundingClientRect().height),48);
    await traffic.getByRole('button',{name:'View',exact:true}).click();await settings.getByLabel('Compact rows',{exact:true}).check();await page.keyboard.press('Escape');
    assert.equal(await timeline.locator('.timeline-row').first().evaluate(el=>el.getBoundingClientRect().height),32);
    await checkAxis();
    await timeline.getByLabel('Timeline order',{exact:true}).selectOption('duration');
    assert.equal(await timeline.locator('.timeline-row').first().getAttribute('data-id'),'tl-slow');
    const end=timeline.getByRole('slider',{name:'Timeline end',exact:true});await end.focus();await page.keyboard.press('Home');
    assert.equal(await timeline.locator('#timeline-tl-slow').count(),1,'Overlap filtering dropped a request starting before the window');
    assert.equal(await timeline.locator('#timeline-tl-fast').count(),0);
    assert.equal(await timeline.locator('#timeline-tl-unknown').count(),1,'Unavailable timing disappeared');
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    const endBox=await end.boundingBox(),overview=await timeline.locator('.timeline-overview > svg').boundingBox();
    await page.mouse.move(endBox.x+2,endBox.y+10);await page.mouse.down();await page.mouse.move(overview.x+overview.width*.5,endBox.y+10,{steps:6});await page.mouse.up();
    const beforeWidth=Number(await timeline.locator('.timeline-window').getAttribute('width'));
    assert.ok(beforeWidth<600&&beforeWidth>400,'End handle did not resize the window');
    const windowBox=await timeline.locator('.timeline-window').boundingBox();
    await page.mouse.move(windowBox.x+windowBox.width/2,windowBox.y+20);await page.mouse.down();await page.mouse.move(windowBox.x+windowBox.width/2+overview.width*.1,windowBox.y+20,{steps:6});await page.mouse.up();
    assert.ok(Math.abs(Number(await timeline.locator('.timeline-window').getAttribute('width'))-beforeWidth)<1,'Panning changed window width');
    const range=await timeline.locator('.timeline-scale > span').first().textContent();
    await show(false);await show(true);
    assert.equal(await timeline.locator('.timeline-scale > span').first().textContent(),range,'Toggling the Timeline reset the adjusted range');
    // Pointer cancellation must work even when focus started on another control.
    await timeline.getByRole('button',{name:'Reset range',exact:true}).focus();
    const box=await end.boundingBox();await page.mouse.move(box.x+2,box.y+10);await page.mouse.down();await page.mouse.move(box.x-50,box.y+10,{steps:4});
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.refreshTimeline());
    await page.keyboard.press('Escape');await page.mouse.up();
    assert.equal(await timeline.locator('.timeline-scale > span').first().textContent(),range,'Cancel did not restore the time window');
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    await show(false);await traffic.locator('#session-tl-slow').click();await traffic.locator('#session-tl-failed').click({modifiers:['Control']});
    await show(true);assert.equal(await timeline.locator('.timeline-row').count(),2);
    await helpButton.click();await checkHelpShortcuts();await page.keyboard.press('Escape');await help.waitFor({state:'hidden'});
    assert.equal(await timeline.locator('.timeline-row').count(),2,'Dismissing help cleared request selection');
    await timeline.locator('#timeline-tl-slow').click();assert.equal(await timeline.locator('.timeline-row').count(),2,'Inspecting collapsed multiselection');
    await timeline.getByRole('button',{name:'Show all matches',exact:true}).click();assert.equal(await timeline.locator('.timeline-row').count(),6);
    await timeline.getByRole('slider',{name:'Timeline window',exact:true}).focus();await page.keyboard.press('+');
    const liveRange=await timeline.locator('.timeline-scale > span').first().textContent();
    const input=traffic.getByLabel('Filter or search traffic',{exact:true});await input.focus();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions.push({...s.sessions[0],id:'tl-later',requestNumber:'7',startedAt:1800000050000});await t.refreshSessions();await t.refreshTimeline();});
    assert.equal(await timeline.locator('.timeline-scale > span').first().textContent(),liveRange,'Live traffic reset the adjusted window');
    assert.equal(await input.evaluate(el=>el===el.getRootNode().activeElement),true,'Timeline update stole focus');
    // List updates pause during inspection, while Timeline includes new requests.
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    await timeline.locator('#timeline-tl-later').click();await page.keyboard.press('Control+a');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.trafficSelection.ids.has('tl-later')),true,'Timeline select all omitted new traffic');
    assert.equal(await timeline.locator('#timeline-tl-later').count(),1);
    await timeline.getByRole('slider',{name:'Timeline end',exact:true}).focus();await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0,'Escape on a range control did not clear selection');
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.deferTimeline=true;void document.querySelector('app-shell').traffic.refreshTimeline();});
    await page.waitForFunction(()=>globalThis.__workspaceFixture.timelinePending);await input.fill('fast');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===1);await page.evaluate(()=>globalThis.__workspaceFixture.releaseTimeline());
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return !t.timelineBusy&&t.timelineEntries.length===1&&t.timelineEntries[0].id==='tl-fast';});
    await page.evaluate(async()=>{globalThis.__workspaceFixture.timelineFailure=true;await document.querySelector('app-shell').traffic.refreshTimeline();});
    await traffic.getByRole('button',{name:'Retry',exact:true}).waitFor();await page.evaluate(()=>globalThis.__workspaceFixture.timelineFailure=false);await traffic.getByRole('button',{name:'Retry',exact:true}).click();await settle();
    // Open source evidence only after the previous filter has fully settled.
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.timelineTimer===undefined);
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.revealSession('tl-slow'));
    await timeline.locator('#timeline-tl-slow').waitFor({state:'visible'});
    assert.equal(await timeline.locator('#timeline-tl-slow').evaluate(el=>el===el.getRootNode().activeElement),true,'Source navigation left focus on the hidden List');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedDetail?.id),'tl-slow');
    // Late source results update evidence without taking focus back from a field.
    await page.evaluate(()=>{globalThis.__workspaceFixture.deferTimeline=true;void document.querySelector('app-shell').traffic.revealSession('tl-fast');});
    await page.waitForFunction(()=>globalThis.__workspaceFixture.timelinePending);await input.focus();
    await page.evaluate(()=>globalThis.__workspaceFixture.releaseTimeline());
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.timelinePending);
    assert.equal(await input.evaluate(el=>el===el.getRootNode().activeElement),true,'Late source navigation stole focus');
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[0];s.sessions=Array.from({length:10000},(_,i)=>({...seed,id:'tl-load-'+i,requestNumber:String(i+1),startedAt:1800000000000+i*10,durationMs:50,url:'https://example.test/scenario/long-url/'+i+'?value='+('long-value-'.repeat(20))}));t.liveFilter='';t.searchText='';t.searchInput.value='';t.queryRevision++;await t.refreshSessions(undefined,true);await t.refreshTimeline();});
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.timelineEntries.length),10000);
    assert.ok(await timeline.locator('.timeline-row').count()<100,'Timeline DOM grew with capture size');
    await show(false);
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.sessionScroller.scrollTop=t.virtualList.offset(5000);t.renderViewport();});
    await traffic.locator('#session-tl-load-5000').waitFor({state:'visible'});
    const listScroll=await traffic.locator('.table-wrap').evaluate(el=>el.scrollTop);
    await show(true);await show(false);
    assert.ok(Math.abs(await traffic.locator('.table-wrap').evaluate(el=>el.scrollTop)-listScroll)<1,'Toggling Timeline lost the List scroll position');
    await show(true);
    await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic;globalThis.__workspaceFixture.sessions[0].url='https://example.test/api/../admin?';await t.refreshTimeline();});
    assert.equal(await timeline.locator('#timeline-tl-load-0 .timeline-request > span').textContent(),'#1 GET /api/../admin?');
    await timeline.getByRole('slider',{name:'Timeline end',exact:true}).focus();await page.keyboard.press('Home');
    await timeline.getByRole('slider',{name:'Timeline window',exact:true}).focus();await page.keyboard.press('End');
    const zoomedLabels=await timeline.locator('.timeline-ticks > span').allTextContents();
    assert.notEqual(zoomedLabels[0],zoomedLabels[1],'Narrow range endpoints collapsed to the same label');
    assert.equal(await timeline.getByRole('slider',{name:'Timeline window',exact:true}).getAttribute('aria-valuetext'),zoomedLabels.join(' – '));
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-narrow-range.png')});
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    await timeline.locator('.timeline-viewport').evaluate(el=>el.scrollTop=5000*32);
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,compactRows:false};});
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic.timeline;return t.rowHeight===48&&t.querySelector('.timeline-row').getBoundingClientRect().height===48;});
    assert.ok(Math.abs(await timeline.locator('.timeline-viewport').evaluate(el=>el.scrollTop/48)-5000)<1,'Density change lost the current request position');
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,compactRows:true};});
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.timeline.rowHeight===32);
    await timeline.locator('.timeline-viewport').evaluate(el=>el.scrollTop=el.scrollHeight);
    await timeline.locator('#timeline-tl-load-9999').waitFor();await timeline.locator('#timeline-tl-load-9999').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='tl-load-9999');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0);
    // Text sizing changes rem lengths without applying browser zoom to pixels.
    for(const size of ['20px','24px','17.5px']){
      await page.evaluate(size=>document.documentElement.style.fontSize=size,size);
      await timeline.locator('.timeline-viewport').focus();await page.keyboard.press('End');
      const geometry=await timeline.evaluate(el=>{const row=el.querySelector('#timeline-tl-load-9999').getBoundingClientRect(),viewport=el.viewport.getBoundingClientRect();return {height:row.height,model:el.rowHeight,top:row.top,bottom:row.bottom,viewTop:viewport.top,viewBottom:viewport.bottom};});
      assert.equal(geometry.height,geometry.model,'Text size changed rendered height without updating virtualization');
      assert.ok(geometry.top>=geometry.viewTop-1&&geometry.bottom<=geometry.viewBottom+1,'End navigation left the focused request outside the viewport');
      await checkAxis();
    }
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-large-text.png')});
    await page.evaluate(()=>document.documentElement.style.fontSize='');
    await page.evaluate(()=>document.documentElement.style.zoom='1.25');
    try{
      await timeline.locator('.timeline-viewport').focus();await page.keyboard.press('End');
      const geometry=await timeline.evaluate(el=>{const row=el.querySelector('#timeline-tl-load-9999'),box=row.getBoundingClientRect(),viewport=el.viewport.getBoundingClientRect();return {height:parseFloat(getComputedStyle(row).height),model:el.rowHeight,top:box.top,bottom:box.bottom,viewTop:viewport.top,viewBottom:viewport.bottom};});
      assert.equal(geometry.height,geometry.model,'Browser zoom changed virtualization units');
      assert.ok(geometry.top>=geometry.viewTop-1&&geometry.bottom<=geometry.viewBottom+1,'End navigation at zoom left the row outside the viewport');
    }finally{await page.evaluate(()=>document.documentElement.style.zoom='');}
  } finally {
    await page.evaluate(()=>{document.documentElement.style.fontSize='';document.documentElement.style.zoom='';});
    await page.evaluate(async saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.timelineFailure=false;s.deferTimeline=false;s.releaseTimeline?.();t.setTimeline(false);s.sessions=saved.rows;t.preferences=saved.preferences;t.comparisonBaseline=saved.baseline;t.comparisonBaselineLabel=saved.baselineLabel;t.liveFilter='';t.searchText='';t.searchInput.value='';t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);},saved);
    await page.setViewportSize({width:1280,height:800});
  }
}
