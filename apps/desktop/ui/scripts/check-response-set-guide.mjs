import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkResponseSetGuide(page,root) {
  await page.addInitScript(()=>sessionStorage.setItem('startup-update-tested','true'));
  await page.goto('https://workspace.test/');
  await page.evaluate(()=>localStorage.removeItem('workspace'));
  await page.reload();
  await page.waitForFunction(()=>document.querySelector('app-shell').workspaceLoaded);
  const manager=page.locator('experiment-variants'),help=manager.getByRole('button',{name:'How response sets work',exact:true});
  const tabs=page.getByRole('navigation',{name:'Automation tools'}),panel=manager.locator('response-set-tour');
  const open=async()=>{await page.evaluate(()=>document.querySelector('app-shell').activateView('automation'));await tabs.getByRole('button',{name:'Response sets',exact:true}).click();};
  const seed=async()=>page.evaluate(()=>{
    const state=globalThis.__workspaceFixture,a=document.querySelector('app-shell').automation;
    const action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
    const rule={id:'guide-rule',displayName:'Checkout response',enabled:true,revision:1,priority:1,matcher:{method:'GET',url:{kind:'exact',value:'https://example.test/checkout.js'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null},request:{...action,responseAsset:'guide-response@1',allowNonIdempotentBodyReplacement:false},response:action};
    state.automation={schemaVersion:1,generation:9,rules:[],autoresponsesEnabled:false,diagnostics:[],experiments:[{id:'guide-set',name:'Checkout changes',archived:false,paused:true,active:null,undo:{selection:null,generation:9},variants:[{id:'baseline',name:'Baseline',archived:false,revisions:[{revision:1,rules:[]}]},{id:'edited',name:'Edited response',archived:false,revisions:[{revision:1,rules:[rule]},{revision:2,rules:[rule]}]}]}]};
    a.renderAutomation(structuredClone(state.automation));a.experimentVariants.reveal({experimentId:'guide-set',variantId:'edited',revision:2},false);
  });
  await open();
  await help.click();
  await manager.getByText('Save your first response set to take the tour.',{exact:true}).waitFor();
  assert.equal(await manager.getByRole('button',{name:'Take a quick tour',exact:true}).isDisabled(),true);
  await manager.getByRole('button',{name:'Close',exact:true}).click();
  assert.equal(await help.evaluate(node=>node.getRootNode().activeElement===node),true);
  assert.equal(await manager.locator('.response-set-invitation').count(),0);
  await page.evaluate(()=>document.querySelector('app-shell').automation.preferencesReady=false);
  await seed();
  assert.equal(await manager.locator('.response-set-invitation').count(),0,'Invitation ran before native preferences were ready');
  await page.evaluate(()=>document.querySelector('app-shell').automation.preferencesReady=true);
  await manager.getByText('New to response sets?',{exact:true}).waitFor();
  assert.equal(await panel.count(),0,'Offering the tour opened it automatically');
  assert.match(await manager.locator('.variant-state').textContent(),/No changes from this set · Response set paused · Saved responses paused/);
  const before=await page.evaluate(()=>structuredClone(globalThis.__workspaceFixture.automation));
  const changesBefore=await page.evaluate(()=>globalThis.__workspaceFixture.calls.change_experiment??0);
  await manager.getByRole('button',{name:'Not now',exact:true}).click();
  assert.equal(await help.evaluate(node=>node.getRootNode().activeElement===node),true);
  await tabs.getByRole('button',{name:'Rules',exact:true}).click();await open();
  assert.equal(await manager.locator('.response-set-invitation').count(),0);
  await page.reload();await page.waitForFunction(()=>document.querySelector('app-shell').workspaceLoaded);await open();await seed();
  assert.equal(await manager.locator('.response-set-invitation').count(),0,'Dismissal did not survive reload');
  await help.click();
  // Two starts share one native preference operation. A late result cannot reopen hidden UI.
  const startsBefore=await page.evaluate(()=>globalThis.__workspaceFixture.calls.update_response_set_guide??0);
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=true;const v=document.querySelector('app-shell').automation.experimentVariants;void v.startGuide();void v.startGuide();});
  await page.waitForFunction(()=>globalThis.__workspaceFixture.finishGuide);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.update_response_set_guide),startsBefore+1,'Concurrent starts duplicated native commands');
  await tabs.getByRole('button',{name:'Rules',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=false;globalThis.__workspaceFixture.finishGuide();});
  await page.waitForFunction(()=>!document.querySelector('app-shell').automation.experimentVariants.guidePending);
  assert.equal(await panel.count(),0);
  assert.equal(await tabs.getByRole('button',{name:'Rules',exact:true}).evaluate(node=>node.getRootNode().activeElement===node),true,'Late tour start stole focus');
  await open();await help.click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=true;globalThis.__workspaceFixture.finishGuide=null;});
  await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.finishGuide);
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=false;globalThis.__workspaceFixture.finishGuide();});
  await page.waitForFunction(()=>!document.querySelector('app-shell').automation.experimentVariants.guidePending);
  await panel.getByText('1 of 4',{exact:true}).waitFor();
  await page.keyboard.press('Escape');await help.click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=true;globalThis.__workspaceFixture.finishGuide=null;});
  await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.finishGuide);
  const closeHelp=manager.getByRole('button',{name:'Close',exact:true});await closeHelp.focus();
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferGuide=false;globalThis.__workspaceFixture.finishGuide();});
  await page.waitForFunction(()=>!document.querySelector('app-shell').automation.experimentVariants.guidePending);
  assert.equal(await panel.count(),0,'Late start removed help after focus moved');
  assert.equal(await closeHelp.evaluate(node=>node.getRootNode().activeElement===node),true,'Late start removed the newly focused help control');
  await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  await panel.getByText('1 of 4',{exact:true}).waitFor();
  assert.equal(await panel.getByRole('button',{name:'Next',exact:true}).evaluate(node=>node.getRootNode().activeElement===node),true);
  assert.equal(await manager.getByRole('button',{name:'Preview saved revision…',exact:true}).isDisabled(),true);
  assert.equal(await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).isDisabled(),true);
  // Selection is passive; refresh preserves both selection and unsaved rename text.
  await manager.getByLabel('Variant',{exact:true}).selectOption('baseline');
  await panel.getByRole('button',{name:'Next',exact:true}).click();
  await panel.getByText('No editable response is available in this selection. You can finish the tour without making changes.',{exact:true}).waitFor();
  await manager.getByLabel('Variant',{exact:true}).selectOption('edited');
  await manager.getByLabel('Variant revision',{exact:true}).selectOption('1');
  await panel.getByText('No editable response is available in this selection. You can finish the tour without making changes.',{exact:true}).waitFor();
  await manager.getByLabel('Variant revision',{exact:true}).selectOption('2');
  await page.evaluate(()=>{const a=document.querySelector('app-shell').automation,s=structuredClone(globalThis.__workspaceFixture.automation);s.experiments[0].archived=true;a.renderAutomation(s);});
  await panel.getByText('No editable response is available in this selection. You can finish the tour without making changes.',{exact:true}).waitFor();
  await page.evaluate(()=>document.querySelector('app-shell').automation.renderAutomation(structuredClone(globalThis.__workspaceFixture.automation)));
  await panel.getByRole('button',{name:'Next',exact:true}).focus();
  await page.evaluate(()=>{
    const a=document.querySelector('app-shell').automation,v=a.experimentVariants,s=structuredClone(globalThis.__workspaceFixture.automation);
    v.experimentName.value='Unfinished rename';
    s.experiments[0].variants[1].revisions[1].rules=[];a.renderAutomation(s);
  });
  await panel.getByRole('button',{name:'Next',exact:true}).waitFor();
  assert.equal(await panel.getByRole('button',{name:'Next',exact:true}).evaluate(node=>node.getRootNode().activeElement===node),true,'Refresh removing the target lost callout focus');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').automation.experimentVariants.experimentName.value),'Unfinished rename','Guide refresh lost a rename draft');
  await page.evaluate(()=>document.querySelector('app-shell').automation.renderAutomation(structuredClone(globalThis.__workspaceFixture.automation)));
  await panel.getByRole('button',{name:'Back',exact:true}).click();
  await panel.getByText('1 of 4',{exact:true}).waitFor();
  await page.keyboard.press('Escape');
  assert.equal(await help.evaluate(node=>node.getRootNode().activeElement===node),true);
  // Every step can end with Escape; replay always starts at the beginning.
  for(let step=1;step<=4;step++){
    await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
    for(let n=1;n<step;n++)await panel.getByRole('button',{name:'Next',exact:true}).click();
    await panel.getByText(`${step} of 4`,{exact:true}).waitFor();
    assert.equal(await manager.locator('[data-tour-target]').count(),1);
    await page.keyboard.press('Escape');assert.equal(await panel.count(),0);
  }
  for(const [width,height] of [[1440,900],[960,720],[800,600]]){
    await page.setViewportSize({width,height});await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
    for(let step=1;step<=4;step++){
      await panel.getByText(`${step} of 4`,{exact:true}).waitFor();
      const next=panel.getByRole('button',{name:step===4?'Done':'Next',exact:true});
      await next.scrollIntoViewIfNeeded();
      assert.equal(await next.evaluate(node=>{const r=node.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;}),true);
      await page.screenshot({path:resolve(root,`../../../target/ui-check/response-set-tour-${width}-step-${step}.png`)});
      await next.click();
    }
  }
  await page.evaluate(()=>document.documentElement.style.zoom='1.5');
  await page.emulateMedia({forcedColors:'active'});
  await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  for(let step=1;step<=4;step++){
    const next=panel.getByRole('button',{name:step===4?'Done':'Next',exact:true});await next.scrollIntoViewIfNeeded();
    assert.equal(await next.evaluate(node=>{const r=node.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;}),true);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/response-set-tour-zoom-contrast-step-${step}.png`)});await next.click();
  }
  await page.evaluate(()=>document.documentElement.style.zoom='');await page.emulateMedia({forcedColors:'none',colorScheme:'dark'});
  await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();await page.screenshot({path:resolve(root,'../../../target/ui-check/response-set-tour-dark.png')});
  // Editor opening and workspace navigation close without refocusing hidden controls.
  await page.evaluate(()=>{const a=document.querySelector('app-shell').automation;a.responseExperiment=true;a.editorHidden=false;});
  await panel.waitFor({state:'hidden'});
  await page.evaluate(()=>{const a=document.querySelector('app-shell').automation;a.editorHidden=true;});
  await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  await page.evaluate(()=>document.querySelector('app-shell').activateView('traffic'));
  await panel.waitFor({state:'hidden'});await open();
  await help.click();await manager.getByRole('button',{name:'Replay tour',exact:true}).click();
  await page.evaluate(()=>{const a=document.querySelector('app-shell').automation,s=structuredClone(globalThis.__workspaceFixture.automation);s.experiments=[];a.renderAutomation(s);});
  await panel.waitFor({state:'hidden'});
  await seed();
  // Already-busy operations prevent new guide commands.
  const calls=await page.evaluate(()=>globalThis.__workspaceFixture.calls.update_response_set_guide);
  await page.evaluate(()=>{const v=document.querySelector('app-shell').automation.experimentVariants;v.variantBusy=true;void v.startGuide();v.variantBusy=false;});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.update_response_set_guide),calls);
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.automation),before,'Tour changed content, live state, generation or Undo');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.change_experiment??0),changesBefore);
  await page.evaluate(()=>document.querySelector('app-shell').resetWorkspace());
  await page.waitForTimeout(350);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.workspace.responseSetGuide),'started','Reset layout reset tour history');
  await page.reload();await page.waitForFunction(()=>document.querySelector('app-shell').workspaceLoaded);await open();await seed();await help.click();
  await manager.getByRole('button',{name:'Replay tour',exact:true}).waitFor();
  await manager.getByRole('button',{name:'Close',exact:true}).click();
  await page.emulateMedia({colorScheme:'light'});await page.setViewportSize({width:1440,height:900});
}
