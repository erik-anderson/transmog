import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficTimeline(page,root) {
  await page.setViewportSize({width:1440,height:900});
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  const saved=await page.evaluate(()=>({rows:globalThis.__workspaceFixture.sessions,preferences:document.querySelector('app-shell').traffic.preferences}));
  const traffic=page.locator('traffic-workspace'),timeline=traffic.locator('traffic-timeline');
  const settle=()=>page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.timelineMode&&!t.timelineBusy&&!t.timelinePending&&t.timelineEntries.length>0;});
  const show=async enabled=>{await traffic.getByRole('button',{name:'View',exact:true}).click();await traffic.locator('#table-settings').getByRole('button',{name:enabled?'Timeline':'List',exact:true}).click();if(enabled)await settle();};
  try {
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[0],base=1800000000000;
      s.sessions=[['slow',0,1200],['fast',100,50],['long',300,700],['zero',600,0],['failed',700,200],['unknown',null,null]].map(([id,start,duration],index)=>({...seed,id:'tl-'+id,requestNumber:String(index+1),method:'GET',path:'/scenario/'+id,url:'https://example.test/scenario/'+id,startedAt:start==null?0:base+start,durationMs:duration,responseOffset:id==='slow'?400000:null,terminal:id==='failed'?'failed':id==='unknown'?'incomplete':'completed',contentType:'text/plain',traceId:id==='unknown'?'imported-fixture':null}));
      t.preferences={...t.preferences,layout:'stacked',listSplit:55};t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);
    });
    const bodies=await page.evaluate(()=>globalThis.__workspaceFixture.calls.inspect_body??0);
    await show(true);
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
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'side-by-side',listSplit:45};});
    assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>0));
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-aside.png')});
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'stacked',listSplit:55};});
    for(const [width,height] of [[1440,900],[960,720],[800,600]]){
      await page.setViewportSize({width,height});
      assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>0),'Timeline rows lost all space');
      await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-'+width+'.png')});
    }
    await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-contrast.png')});await page.emulateMedia({forcedColors:'none',colorScheme:'dark'});await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-dark.png')});await page.emulateMedia({colorScheme:'light'});
    await page.evaluate(()=>document.documentElement.style.zoom='1.25');
    try{assert.ok(await timeline.locator('.timeline-viewport').evaluate(el=>el.clientHeight>0));await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-timeline-zoom.png')});}finally{await page.evaluate(()=>document.documentElement.style.zoom='');}
    await page.setViewportSize({width:1440,height:900});
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
    await timeline.getByRole('group',{name:'Timeline overview',exact:true}).focus();
    const box=await end.boundingBox();await page.mouse.move(box.x+2,box.y+10);await page.mouse.down();await page.mouse.move(box.x-50,box.y+10,{steps:4});
    await page.evaluate(async()=>await document.querySelector('app-shell').traffic.refreshTimeline());
    await page.keyboard.press('Escape');await page.mouse.up();
    assert.equal(await timeline.locator('.timeline-scale > span').first().textContent(),range,'Cancel did not restore the time window');
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    await show(false);await traffic.locator('#session-tl-slow').click();await traffic.locator('#session-tl-failed').click({modifiers:['Control']});
    await show(true);assert.equal(await timeline.locator('.timeline-row').count(),2);
    await timeline.locator('#timeline-tl-slow').click();assert.equal(await timeline.locator('.timeline-row').count(),2,'Inspecting collapsed multiselection');
    await timeline.getByRole('button',{name:'All matching requests',exact:true}).click();assert.equal(await timeline.locator('.timeline-row').count(),6);
    await timeline.getByRole('slider',{name:'Timeline window',exact:true}).focus();await page.keyboard.press('+');
    const liveRange=await timeline.locator('.timeline-scale > span').first().textContent();
    const input=traffic.getByLabel('Filter or search traffic',{exact:true});await input.focus();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions.push({...s.sessions[0],id:'tl-later',requestNumber:'7',startedAt:1800000050000});await t.refreshSessions(undefined,true);await t.refreshTimeline();});
    assert.equal(await timeline.locator('.timeline-scale > span').first().textContent(),liveRange,'Live traffic reset the adjusted window');
    assert.equal(await input.evaluate(el=>el===el.getRootNode().activeElement),true,'Timeline update stole focus');
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.deferTimeline=true;void document.querySelector('app-shell').traffic.refreshTimeline();});
    await page.waitForFunction(()=>globalThis.__workspaceFixture.timelinePending);await input.fill('fast');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===1);await page.evaluate(()=>globalThis.__workspaceFixture.releaseTimeline());
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return !t.timelineBusy&&t.timelineEntries.length===1&&t.timelineEntries[0].id==='tl-fast';});
    await page.evaluate(async()=>{globalThis.__workspaceFixture.timelineFailure=true;await document.querySelector('app-shell').traffic.refreshTimeline();});
    await traffic.getByRole('button',{name:'Retry',exact:true}).waitFor();await page.evaluate(()=>globalThis.__workspaceFixture.timelineFailure=false);await traffic.getByRole('button',{name:'Retry',exact:true}).click();await settle();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic,seed=s.sessions[0];s.sessions=Array.from({length:10000},(_,i)=>({...seed,id:'tl-load-'+i,requestNumber:String(i+1),startedAt:1800000000000+i*10,durationMs:50,url:'https://example.test/scenario/long-url/'+i+'?value='+('long-value-'.repeat(20))}));t.liveFilter='';t.searchText='';t.searchInput.value='';t.queryRevision++;await t.refreshSessions(undefined,true);await t.refreshTimeline();});
    await timeline.getByRole('button',{name:'Reset range',exact:true}).click();
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.timelineEntries.length),10000);
    assert.ok(await timeline.locator('.timeline-row').count()<100,'Timeline DOM grew with capture size');
    await timeline.locator('.timeline-viewport').evaluate(el=>el.scrollTop=el.scrollHeight);
    await timeline.locator('#timeline-tl-load-9999').waitFor();await timeline.locator('#timeline-tl-load-9999').click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='tl-load-9999');
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedTrafficCount),0);
  } finally {
    await page.evaluate(async saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.timelineFailure=false;s.deferTimeline=false;s.releaseTimeline?.();t.setTimeline(false);s.sessions=saved.rows;t.preferences=saved.preferences;t.liveFilter='';t.searchText='';t.searchInput.value='';t.clearTrafficSelection();t.queryRevision++;await t.refreshSessions(undefined,true);},saved);
    await page.setViewportSize({width:1280,height:800});
  }
}
