import {openActionGroup} from './check-action-menus.mjs';
import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkTrafficPresentation(page,root) {
  const traffic=page.locator('traffic-workspace'),input=traffic.getByLabel('Filter or search traffic',{exact:true});
  const saved=await page.evaluate(()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;return {rows:s.sessions,preferences:t.preferences,directCommand:s.directCommand};});
  try {
    await traffic.getByLabel('Traffic search mode',{exact:true}).selectOption('filter');
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions=Array.from({length:240},(_,i)=>({...s.sessions[1],id:'presentation-'+i,path:'/api/catalog/'+i,url:'https://example.test/api/catalog/'+i,startedAt:i+1}));t.preferences={...t.preferences,layout:'stacked',listSplit:45};t.followLatest=true;await t.refreshSessions(undefined,true);});
    await traffic.locator('.session-link').first().click();
    await page.waitForFunction(()=>!!document.querySelector('app-shell').traffic.selectedDetail);
    const selected=await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId);
    assert.equal(await traffic.locator('.traffic-selection-bar').isVisible(),false,'Single selection added bulk action chrome');
    for(const width of [1440,960,800]) {
      await page.setViewportSize({width,height:width===800?600:800});
      const alignment=await page.evaluate(()=>{
        const r=document.querySelector('app-shell').shadowRoot,input=r.querySelector('.traffic-search input');
        const baseline=e=>{const b=e.getBoundingClientRect(),s=getComputedStyle(e);return {top:b.top,height:b.height,font:s.font,padding:s.padding,border:s.borderTopWidth};};
        return {input:baseline(input),buttons:['Filters','Files','View'].map(name=>baseline([...r.querySelectorAll('.traffic-tools > button')].find(e=>e.textContent.trim()===name)))};
      });
      for(const button of alignment.buttons){assert.equal(button.font,alignment.input.font);assert.equal(button.padding,alignment.input.padding);assert.ok(Math.abs(button.top-alignment.input.top)<1&&Math.abs(button.height-alignment.input.height)<1,'Filter text and toolbar buttons lost baseline alignment');}
      const geometry=await traffic.locator('.traffic-table').evaluate(table=>{const spacer=table.querySelector('.traffic-spacer td'),head=table.tHead.getBoundingClientRect(),row=table.querySelector('tr[data-session-id]').getBoundingClientRect();return {spacer:spacer.getBoundingClientRect().height,gap:row.top-head.bottom};});
      assert.equal(geometry.spacer,0,'A zero-size virtual spacer acquired row padding');assert.ok(Math.abs(geometry.gap)<1);
      assert.equal(await traffic.locator('message-inspector[side="request"]').isVisible(),width>1112);
      assert.equal(await traffic.locator('message-inspector[side="response"]').isVisible(),true);
      const toggle=traffic.getByRole('group',{name:'Message to inspect',exact:true});
      if(width<=1112) {
        const request=toggle.getByRole('radio',{name:'Request',exact:true}),response=toggle.getByRole('radio',{name:'Response',exact:true});
        await request.check();assert.equal(await request.isChecked(),true);
        assert.equal(await traffic.locator('message-inspector[side="request"]').isVisible(),true);
        assert.equal(await traffic.locator('message-inspector[side="response"]').isVisible(),false);
        await response.check();assert.equal(await response.isChecked(),true);
        await response.focus();await page.keyboard.press('ArrowLeft');
        assert.equal(await request.isChecked(),true);assert.equal(await traffic.locator('message-inspector[side="request"]').isVisible(),true);
        await page.keyboard.press('ArrowRight');assert.equal(await response.isChecked(),true);
        // Marker navigation changes the selected side programmatically after user input.
        await page.evaluate(()=>document.querySelector('app-shell').traffic.showInspectorSide('request'));
        assert.equal(await request.isChecked(),true);assert.equal(await response.isChecked(),false);
        await page.evaluate(()=>document.querySelector('app-shell').traffic.showInspectorSide('response'));
        assert.equal(await response.isChecked(),true);
        assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),selected);
        const bounds=await toggle.boundingBox();assert.ok(bounds.x>=0&&bounds.x+bounds.width<=width,'Message toggle clipped');
        await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-'+width+'.png')});
        await page.emulateMedia({colorScheme:'dark'});
        await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-dark-'+width+'.png')});
        await page.emulateMedia({colorScheme:'light',forcedColors:'active'});await request.check();
        await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-contrast-'+width+'.png')});
        await page.emulateMedia({forcedColors:'none'});await response.check();
        if(width===800) {
          await page.evaluate(()=>document.documentElement.style.zoom='1.5');
          const zoomed=await toggle.boundingBox();assert.ok(zoomed.x>=0&&zoomed.x+zoomed.width<=width,'Zoomed message toggle clipped');
          await request.check();assert.equal(await traffic.locator('message-inspector[side="request"]').isVisible(),true);
          await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-zoom-800.png')});
          await page.evaluate(()=>document.documentElement.style.zoom='');await response.check();
        }
      } else {assert.equal(await toggle.isVisible(),false);await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-'+width+'.png')});}
    }
    // A resized inspector uses the same control even in a wide window.
    await page.setViewportSize({width:1440,height:900});
    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'side-by-side',listSplit:55};});
    const narrowToggle=traffic.getByRole('group',{name:'Message to inspect',exact:true});
    assert.equal(await narrowToggle.isVisible(),true);
    await narrowToggle.getByRole('radio',{name:'Request',exact:true}).check();
    assert.equal(await traffic.locator('message-inspector[side="response"]').isVisible(),false);
    await narrowToggle.getByRole('radio',{name:'Response',exact:true}).check();
    await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-narrow-inspector.png')});
    const labels=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;const saved={method:t.selectedMethodText,status:t.selectedStatusText};t.preferences={...t.preferences,listSplit:75};t.selectedMethodText='PROPPATCH';t.selectedStatusText='No response';return saved;});
    try {
      const bounds=await narrowToggle.evaluate(e=>{const pane=e.closest('.details-pane').getBoundingClientRect();return {pane:{left:pane.left,right:pane.right},segments:[...e.querySelectorAll('label')].map(label=>{const b=label.getBoundingClientRect();return {name:label.textContent.trim(),left:b.left,right:b.right};})};});
      assert.ok(bounds.segments.every(segment=>segment.left>=bounds.pane.left&&segment.right<=bounds.pane.right),'Compact message pill clipped with a long method and pending status: '+JSON.stringify(bounds));
      const url=traffic.getByTitle('Show full URL',{exact:true});
      assert.ok((await url.boundingBox()).width>=48,'Minimum-width inspector lost the clickable URL');
      await url.click();await traffic.locator('#selected-url-full').waitFor({state:'visible'});await page.keyboard.press('Escape');
      await narrowToggle.getByRole('radio',{name:'Request',exact:true}).check();
      await narrowToggle.getByRole('radio',{name:'Response',exact:true}).check();
      await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-minimum-inspector.png')});
      const detail=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,original=t.selectedDetail;t.selectedDetail={...original,autoResponse:{ruleId:'layout-rule',ruleName:'Saved response with a long rule name',ruleRevision:1,position:0,assetReference:'layout-asset@1',status:200,bodyBytes:4}};return original;});
      try {
        assert.ok((await url.boundingBox()).width>=48,'Saved-response layout hid the full URL');
        await url.click();await traffic.locator('#selected-url-full').waitFor({state:'visible'});await page.keyboard.press('Escape');
        await narrowToggle.getByRole('radio',{name:'Request',exact:true}).check();
        assert.equal(await traffic.locator('message-inspector[side="request"]').isVisible(),true);
        await narrowToggle.getByRole('radio',{name:'Response',exact:true}).check();
        await page.screenshot({path:resolve(root,'../../../target/ui-check/traffic-minimum-saved-response.png')});
      } finally {await page.evaluate(detail=>document.querySelector('app-shell').traffic.selectedDetail=detail,detail);}
    } finally {await page.evaluate(saved=>{const t=document.querySelector('app-shell').traffic;t.selectedMethodText=saved.method;t.selectedStatusText=saved.status;},labels);}

    await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'stacked',listSplit:45};});
    const calls=await page.evaluate(()=>({search:globalThis.__workspaceFixture.calls.search_traffic??0,body:globalThis.__workspaceFixture.calls.inspect_body??0}));
    await input.fill('/api/catalog/12');
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===11);
    assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),selected,'Live filtering replaced the inspected request');
    assert.deepEqual(await page.evaluate(()=>({search:globalThis.__workspaceFixture.calls.search_traffic??0,body:globalThis.__workspaceFixture.calls.inspect_body??0})),calls,'Typing decoded captured contents');
    await page.evaluate(()=>globalThis.__workspaceFixture.deferRefresh=true);
    await input.fill('/api/catalog/20');await page.waitForFunction(()=>globalThis.__workspaceFixture.refreshPending);
    await input.fill('/api/catalog/239');await page.evaluate(()=>globalThis.__workspaceFixture.releaseRefresh());
    await page.waitForFunction(()=>{const t=document.querySelector('app-shell').traffic;return t.totalMatched===1&&t.sessions[0]?.id==='presentation-239';});
    await input.fill('no-such-request');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===0);
    assert.match(await traffic.locator('.empty-cell').textContent(),/No matching traffic/);
    await input.fill('');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===240);
    await traffic.getByLabel('Traffic search mode',{exact:true}).selectOption('contents');
    await input.fill('body');assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.search_traffic??0),calls.search);
    await traffic.locator('.filters').getByRole('button',{name:'Search',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.contentSearchActive);
    await traffic.getByLabel('Traffic search mode',{exact:true}).selectOption('filter');
    await input.fill('');await page.waitForFunction(()=>document.querySelector('app-shell').traffic.totalMatched===240);
    await traffic.locator('.session-link').first().click();
    const details=traffic.locator('.selection-actions').getByRole('button',{name:'Details…',exact:true});await details.click();
    const dialog=traffic.locator('.details-dialog[open]');
    await dialog.getByRole('tab',{name:'Timings',exact:true}).focus();await page.keyboard.press('ArrowRight');
    assert.equal(await dialog.getByRole('tab',{name:'Connection',exact:true}).getAttribute('aria-selected'),'true');
    await page.keyboard.press('End');assert.equal(await dialog.getByRole('tab',{name:'Certificates',exact:true}).getAttribute('aria-selected'),'true');
    await page.keyboard.press('Escape');assert.equal(await details.evaluate(e=>e===e.getRootNode().activeElement),true);
    // Menu actions must leave a visible keyboard focus target, including after dialogs.
    const actions=traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true});
    const focused=locator=>locator.evaluate(e=>e===e.getRootNode().activeElement);
    for(const name of ['Copy URL','Copy all headers']) {
      await actions.click();await openActionGroup(traffic.locator('#selected-request-actions'),'Copy');await traffic.locator('#selected-request-actions').getByRole('button',{name,exact:true}).click();
      assert.equal(await focused(actions),true,name+' left focus in a hidden menu');
    }
    await page.evaluate(()=>globalThis.__workspaceFixture.directCommand=false);
    await actions.click();await openActionGroup(traffic.locator('#selected-request-actions'),'Copy');await traffic.locator('#selected-request-actions').getByRole('button',{name:'Copy as cURL',exact:true}).click();
    await traffic.locator('.request-command-dialog[open]').waitFor();await page.keyboard.press('Escape');
    assert.equal(await focused(actions),true,'Closing a generated command lost focus');
    const detail=await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic,detail=structuredClone(t.selectedDetail),html=structuredClone(detail);html.storedBodies[0].mediaType='text/html';t.applyDetail(html);return detail;});
    try {
      for(const close of ['Escape','Cancel']) {
        await actions.click();await openActionGroup(traffic.locator('#selected-request-actions'),'Preview');await traffic.locator('#selected-request-actions').getByRole('button',{name:'Preview page…',exact:true}).click();
        if(close==='Escape')await page.keyboard.press('Escape');else await page.locator('captured-preview .captured-page-warning[open]').getByRole('button',{name:close,exact:true}).click();
        assert.equal(await focused(actions),true,'Closing preview lost focus');
      }
    } finally {await page.evaluate(detail=>document.querySelector('app-shell').traffic.applyDetail(detail),detail);}
    const files=traffic.getByRole('button',{name:'Files',exact:true});
    for(const close of ['Escape','Close']) {
      await files.click();await traffic.locator('#trace-files-menu').getByRole('button',{name:'Save copy…',exact:true}).click();
      if(close==='Escape')await page.keyboard.press('Escape');else await traffic.locator('.trace-save-dialog[open]').getByRole('button',{name:close,exact:true}).click();
      assert.equal(await focused(files),true,'Closing Save trace lost focus');
    }
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.queryPending);
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.deferRefresh=true;s.refreshPending=false;});
    await traffic.getByLabel('Traffic search mode',{exact:true}).selectOption('contents');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.refreshPending);
    const view=traffic.getByRole('button',{name:'View',exact:true});await view.click();
    await page.evaluate(()=>globalThis.__workspaceFixture.releaseRefresh());
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.queryPending);
    assert.equal(await focused(view),true,'A completed search-mode query stole focus');
    await page.keyboard.press('Escape');

  } finally {
    await page.evaluate(async saved=>{const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;s.sessions=saved.rows;s.directCommand=saved.directCommand;t.preferences=saved.preferences;await t.clearContentSearch(false);t.trafficSearchKind='filter';t.searchPlaceholder='Filter URLs and processes';t.followLatest=true;await t.refreshSessions(undefined,true);},saved);
    await page.setViewportSize({width:1280,height:800});
  }
}
