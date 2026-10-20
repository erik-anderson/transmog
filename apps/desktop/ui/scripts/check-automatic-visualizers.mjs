import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkAutomaticVisualizers(page, root) {
  await page.goto('https://workspace.test/');
  await page.setViewportSize({width:1440,height:900});
  const traffic=page.locator('traffic-workspace');
  const request=traffic.locator('message-inspector[side="request"]');
  const response=traffic.locator('message-inspector[side="response"]');
  const viewer=response.locator('extension-visualizer');
  await page.evaluate(async()=>{
    const s=globalThis.__workspaceFixture,t=document.querySelector('app-shell').traffic;
    t.preferences={...t.preferences,layout:'stacked',listSplit:30};
    const automatic={extensionId:'example.auto',visualizerId:'body',name:'Automatic protocol',automatic:true};
    s.visualizerChoicesBySession={first:{body:[automatic]},second:{body:[]}};
    s.visualizerDocument={title:'Parsed protocol',status:'complete',presentation:'tree',roots:[{id:'field',name:'Decoded field',spans:[],children:[],value:{kind:'integer',value:'42'}}],sources:[],diagnostics:[],summary:null};
    await t.refreshSessions(undefined,true);
  });
  await traffic.locator('tr[data-session-id="first"]').click();
  await viewer.locator('.visualizer-heading > strong').filter({hasText:'Parsed protocol'}).waitFor();
  assert.equal(await response.getByRole('combobox',{name:'Body viewer',exact:true}).inputValue(),'auto');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.visualizerStarts.length),1,'Hidden request headers started an automatic body parser');
  await request.getByRole('button',{name:'Body',exact:true}).click();
  await request.locator('extension-visualizer').locator('.visualizer-heading > strong').filter({hasText:'Parsed protocol'}).waitFor();
  const requestSelector=request.getByRole('combobox',{name:'Body viewer',exact:true});
  await traffic.locator('tr[data-session-id="second"]').click();
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.querySelector('message-inspector[side="request"]').mode==='headers');
  await traffic.locator('tr[data-session-id="first"]').click();
  await request.locator('extension-visualizer .visualizer-heading > strong').waitFor();
  assert.equal(await requestSelector.inputValue(),'auto','The request Auto preference was lost across unmatched traffic');
  await requestSelector.selectOption({label:'Automatic protocol'});
  await page.waitForFunction(()=>globalThis.__workspaceFixture.visualizerStarts.at(-1).automatic===false);
  await traffic.locator('tr[data-session-id="second"]').click();
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='second');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.querySelector('message-inspector[side="request"]').mode==='headers');
  assert.equal(await request.locator('extension-visualizer').count(),0);
  await traffic.locator('tr[data-session-id="first"]').click();
  await request.locator('extension-visualizer').locator('.visualizer-heading > strong').filter({hasText:'Parsed protocol'}).waitFor();
  assert.equal(await requestSelector.inputValue(),'extension:example.auto:body','Matching traffic did not restore the selected viewer');
  assert.equal(await request.getByRole('button',{name:'Body',exact:true}).getAttribute('data-selected'),'');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/automatic-visualizers-wide.png')});
  await page.setViewportSize({width:800,height:600});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/automatic-visualizers-compact.png')});
  await page.setViewportSize({width:1440,height:900});
  await request.getByRole('button',{name:'Headers',exact:true}).click();

  // Failed or partial decoders remain available explicitly. Auto tries only four.
  const before=await page.evaluate(()=>globalThis.__workspaceFixture.visualizerStarts.length);
  await page.evaluate(()=>{
    const s=globalThis.__workspaceFixture;
    s.visualizerChoicesBySession.first.body=Array.from({length:8},(_,i)=>({extensionId:'example.auto',visualizerId:'candidate-'+i,name:'Candidate '+i,automatic:true}));
    s.visualizerDocument.status='partial';s.emitNative('extensions-changed');
  });
  await page.waitForFunction(count=>globalThis.__workspaceFixture.visualizerStarts.length>=count+4,before);
  await viewer.waitFor({state:'detached'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.visualizerStarts.length),before+4);
  await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.selectedDetail={...t.selectedDetail};});
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.visualizerStarts.length),before+4,'Unrelated traffic retried failed Auto parsers');
  const responseSelector=response.getByRole('combobox',{name:'Body viewer',exact:true});
  await responseSelector.selectOption({label:'Candidate 0'});
  await viewer.getByText('partial',{exact:true}).waitFor();
  await responseSelector.selectOption('bytes');
  await page.waitForFunction(()=>Object.keys(globalThis.__workspaceFixture.visualizerActive).length===0);

  // An Auto result that finishes after changing selection must remain unloaded.
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.holdVisualizer=true;s.visualizerDocument.status='complete';});
  await responseSelector.selectOption('auto');
  await page.waitForFunction(()=>!!globalThis.__workspaceFixture.finishVisualizer);
  await traffic.locator('tr[data-session-id="second"]').click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.finishVisualizer();delete globalThis.__workspaceFixture.finishVisualizer;});
  await page.waitForFunction(()=>Object.keys(globalThis.__workspaceFixture.visualizerActive).length===0);
  assert.equal(await viewer.count(),0,'A stale automatic parse restored the old message');
}
