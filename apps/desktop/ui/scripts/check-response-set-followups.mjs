import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import {resolve} from 'node:path';

export async function checkResponseSetFollowups(page,root) {
  const evidence=resolve(root,'../../../target/response-set-followups');await mkdir(evidence,{recursive:true});
  await page.goto('https://workspace.test/');
  const traffic=page.locator('traffic-workspace');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  await page.evaluate(async()=>{
    const fixture=globalThis.__workspaceFixture;
    const supplied={ruleId:'demo-rule',ruleName:'Checkout demo response',ruleRevision:7,position:1,assetReference:'demo@1',status:200,bodyBytes:4,snapshot:null};
    fixture.sessions.find(row=>row.id==='first').autoResponse=supplied;
    const download=fixture.sessions.find(row=>row.id==='second');download.terminal='active';
    const websocket=fixture.sessions.find(row=>row.id==='image');websocket.terminal='active';websocket.streamProtocol='web-socket';
    await document.querySelector('app-shell').traffic.refreshSessions(undefined,true);
  });
  await traffic.locator('tr[data-session-id="first"]').click();
  await page.waitForFunction(()=>!!document.querySelector('app-shell').traffic.selectedDetail?.autoResponse);
  assert.match(await traffic.locator('.auto-response-notice').innerText(),/Auto-responded.*Checkout demo response/);
  await traffic.getByRole('button',{name:'Details…',exact:true}).click();
  assert.match(await traffic.locator('.auto-response-evidence').innerText(),/Recorded rule revision 7/);
  for(const width of [1440,960,800]){await page.setViewportSize({width,height:900});await page.screenshot({path:resolve(evidence,`auto-response-${width}.png`)});}
  await page.keyboard.press('Escape');
  await checkAutoResponseInspectorLayout(page,traffic,evidence);
  await traffic.locator('tr[data-session-id="second"]').click();
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='second');
  await traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true}).click();
  await traffic.locator('#selected-request-actions').getByRole('button',{name:'Cancel request',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.sessions.find(row=>row.id==='second').terminal==='cancelled');
  assert.equal(await traffic.locator('tr[data-session-id="second"]').count(),1,'Cancel removed inspection evidence');
  await traffic.locator('tr[data-session-id="image"]').click();
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.selectedDetail?.id==='image');
  await traffic.getByRole('button',{name:'Clear all',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
  assert.equal(await traffic.locator('tr[data-session-id="image"]').count(),1,'Clear hid an open WebSocket');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedSessionId),'image','Clear discarded the protected inspector');
  assert.match(await traffic.innerText(),/1 open or connecting WebSocket was kept for inspection/);
  await page.getByRole('link',{name:'Settings',exact:true}).click();
  const setting=page.getByRole('checkbox',{name:'Terminate open WebSockets when clearing Traffic',exact:true});
  assert.equal(await setting.isChecked(),false);
  await setting.check();await page.getByRole('button',{name:'Save settings',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.savedProduct?.privacy.terminateWebsocketsOnClear===true);
  await page.getByRole('link',{name:'Traffic',exact:true}).click();
  await traffic.getByRole('button',{name:'Clear all',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.removingTraffic);
  assert.equal(await traffic.locator('tr[data-session-id="image"]').count(),0);

  await page.getByRole('link',{name:/Automation/}).click();
  await page.waitForFunction(()=>!!document.querySelector('app-shell').automation.autoresponseState);
  await checkGlobalResponseCompletion(page);
  await page.evaluate(()=>{
    const fixture=globalThis.__workspaceFixture,w=document.querySelector('app-shell').automation;
    const asset={id:'followup-body',revision:1,status:200,headers:[],display:'body',bodyBytes:4,sha256:'0'.repeat(64),mediaType:'text/plain',provenance:{kind:'authored'}};
    fixture.assets=[asset];
    const action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
    const rule=(index)=>({id:'followup-'+index,displayName:'Response '+index,enabled:true,revision:1,priority:index,matcher:{method:'GET',url:{kind:'exact',value:'https://example.test/'+index},requestHeaders:[],responseHeaders:[]},request:{...action,responseAsset:'followup-body@1',allowNonIdempotentBodyReplacement:false},response:{...action}});
    const owned=rule('paused');owned.priority=-1;
    fixture.automation={schemaVersion:2,generation:(w.autoresponseState?.generation??0)+1,autoresponsesEnabled:true,rules:[owned,...Array.from({length:1000},(_,index)=>rule(index))],responseSets:[{id:'followup-set',incarnation:'followup-owner',name:'Saved checkout workspace',notes:'A long note\nwith a second line',revision:1,modifiedAt:Date.now(),rules:[owned],versions:[],applied:{revision:1,rules:[owned]},undoApply:null,paused:true,previewPage:null,previewPageRevision:0}]};
    w.renderAutomation(structuredClone(fixture.automation));
  });
  const workspace=page.locator('automation-workspace'),sets=workspace.locator('response-set-workspace');
  assert.match(await workspace.locator('autoresponse-switch').innerText(),/1,?000 active/);
  await workspace.locator('.workspace-tabs').getByRole('button',{name:'Response sets',exact:true}).click();
  for(const width of [1440,960,800]){
    await page.setViewportSize({width,height:900});
    const cells=sets.locator('.response-set-library tbody tr').first().locator('td');
    for(const cell of await cells.all())assert.equal(await cell.evaluate(el=>getComputedStyle(el).verticalAlign),'middle');
    await page.screenshot({path:resolve(evidence,`library-${width}.png`)});
  }
  const row=sets.locator('.response-set-library tbody tr').first();
  await row.locator('td').nth(4).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').automation.collectionOpen);
  assert.equal(await sets.locator('.response-set-current').getByLabel('Response set name',{exact:true}).inputValue(),'Saved checkout workspace');
  await sets.getByRole('button',{name:'Add responses…',exact:true}).click();
  await sets.getByRole('button',{name:'From existing rules…',exact:true}).click();
  const dialog=page.locator('.response-set-intake-dialog');
  await dialog.waitFor({state:'visible'});
  await page.waitForFunction(()=>{const intake=document.querySelector('app-shell').responseSetIntake;return intake.intakeOpen&&!intake.intakeLoading&&intake.intakeSource==='rules';});
  await dialog.locator('tbody tr').nth(999).waitFor({state:'attached'});
  assert.equal(await dialog.locator('tbody tr').count(),1000);
  await dialog.locator('#intake-source-followup-0 strong').click();
  await dialog.locator('#intake-source-followup-2 strong').click({modifiers:['Shift']});
  assert.equal(await page.evaluate(()=>String(getSelection())), '');
  assert.equal(await dialog.getByRole('checkbox',{checked:true}).count(),3);
  await dialog.locator('#intake-source-followup-0').focus();await page.keyboard.press('Control+a');
  assert.equal(await dialog.getByRole('checkbox',{checked:true}).count(),1000);
  await dialog.getByLabel('Response set',{exact:true}).selectOption('__new');
  await dialog.getByLabel('Response set name',{exact:true}).fill('One thousand responses');
  await dialog.getByRole('button',{name:'Review responses',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').responseSetIntake.intakeReviewBusy);
  assert.equal(await dialog.locator('.intake-review ol > li').count(),1000,'The old 64-response cap still blocked review');
  await page.keyboard.press('Escape');
  await sets.getByRole('button',{name:'← Response sets',exact:true}).click();
  await sets.getByRole('button',{name:'Saved checkout workspace',exact:true}).focus();await page.keyboard.press('Enter');
  await page.waitForFunction(()=>document.querySelector('app-shell').automation.collectionOpen);
  await page.setViewportSize({width:1440,height:900});
}

async function checkAutoResponseInspectorLayout(page,traffic,evidence) {
  const url=traffic.getByTitle('Show full URL',{exact:true});
  const actions=traffic.locator('.selection-actions');
  for(const width of [1440,960,800]) {
    await page.setViewportSize({width,height:720});
    for(const name of ['Checkout response','Very long saved checkout demo response '.repeat(4).slice(0,128)]) {
      await page.evaluate(async name=>{
        globalThis.__workspaceFixture.sessions.find(row=>row.id==='first').autoResponse.ruleName=name;
        await document.querySelector('app-shell').traffic.refreshInspection();
      },name);
      await page.waitForFunction(name=>document.querySelector('app-shell').traffic.selectedDetail?.autoResponse?.ruleName===name,name);
      const geometry=await url.evaluate(el=>{
        const rect=el.getBoundingClientRect(),header=el.closest('.selection-bar'),bounds=header.getBoundingClientRect();
        return {width:rect.width,left:rect.left,right:rect.right,headerLeft:bounds.left,headerRight:bounds.right,overflow:header.scrollWidth-header.clientWidth};
      });
      assert.ok(geometry.width>=60,`Auto-response banner hid the URL at ${width}px: ${JSON.stringify(geometry)}`);
      assert.ok(geometry.left>=geometry.headerLeft&&geometry.right<=geometry.headerRight&&geometry.overflow<=1,'Inspector header overflowed');
      await url.click();await traffic.locator('#selected-url-full:popover-open').waitFor();await page.keyboard.press('Escape');
      await url.focus();await page.keyboard.press('Enter');await traffic.locator('#selected-url-full:popover-open').waitFor();await page.keyboard.press('Escape');
      assert.ok(await url.evaluate(el=>el.matches(':focus')),'Closing the URL popover lost its return focus');
      await actions.getByRole('button',{name:'Details…',exact:true}).click();
      await page.waitForFunction(name=>document.querySelector('app-shell').traffic.timingAutoResponse?.ruleName===name,name);
      assert.match(await traffic.locator('.auto-response-evidence').innerText(),/Recorded rule revision 7/);
      await page.keyboard.press('Escape');
      await page.waitForFunction(()=>{const workspace=document.querySelector('app-shell').traffic;return !workspace.timingDialog.open&&workspace.detailsReturnFocus===null;});
      const actionButton=actions.getByRole('button',{name:'Actions',exact:true});
      await actionButton.focus();await page.keyboard.press('Enter');await traffic.locator('#selected-request-actions:popover-open').waitFor();await page.keyboard.press('Escape');
      await page.screenshot({path:resolve(evidence,`auto-response-inspector-${width}-${name.length>100?'long':'short'}.png`)});
    }
  }
}

async function checkGlobalResponseCompletion(page) {
  await page.evaluate(()=>{
    const shell=document.querySelector('app-shell'),workspace=shell.automation,fixture=globalThis.__workspaceFixture;
    const action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
    const rule={id:'late-toggle-response',displayName:'Paused set response',enabled:true,revision:1,priority:0,matcher:{method:'GET',url:{kind:'exact',value:'https://example.test/'},requestHeaders:[],responseHeaders:[]},request:{...action,responseAsset:'late-toggle-body@1',allowNonIdempotentBodyReplacement:false},response:action};
    fixture.automation={generation:(shell.autoresponseState?.generation??0)+1,autoresponsesEnabled:false,rules:[rule],responseSets:[{id:'late-toggle-set',incarnation:'late-toggle-owner',name:'Response set',notes:'',revision:1,modifiedAt:1,rules:[rule],versions:[],applied:{revision:1,rules:[rule]},undoApply:null,paused:false,previewPage:null,previewPageRevision:0}],diagnostics:[],usage:[],candidateCount:0,historyCount:0};
    workspace.renderAutomation(structuredClone(fixture.automation));
    const original=window.__TAURI_INTERNALS__.invoke;globalThis.__responseToggleOriginal=original;
    window.__TAURI_INTERNALS__.invoke=async(command,args)=>{
      const result=await original(command,args);
      if(command==='set_autoresponses_enabled')await new Promise(resolve=>globalThis.__releaseResponseToggle=resolve);
      return result;
    };
    globalThis.__responseTogglePending=shell.toggleAutoresponses();
  });
  try {
    await page.waitForFunction(()=>Boolean(globalThis.__releaseResponseToggle));
    await page.evaluate(async()=>{
      const shell=document.querySelector('app-shell'),workspace=shell.automation,fixture=globalThis.__workspaceFixture;
      await workspace.refreshAutomation();
      workspace.renderAutomation(await window.__TAURI_INTERNALS__.invoke('change_response_set',{generation:fixture.automation.generation,change:{action:'pause',id:'late-toggle-set',paused:true}}));
      shell.diagnosticText='Response set paused for new traffic.';
    });
    await page.waitForFunction(()=>document.querySelector('app-shell').automation.querySelector('autoresponse-switch').autoresponsesLabel==='Saved responses · On (0 active)');
    await page.evaluate(async()=>{globalThis.__releaseResponseToggle();await globalThis.__responseTogglePending;});
    assert.deepEqual(await page.evaluate(()=>{
      const shell=document.querySelector('app-shell');
      return {currentGeneration:shell.autoresponseState.generation,backendGeneration:globalThis.__workspaceFixture.automation.generation,paused:shell.autoresponseState.responseSets[0].paused,label:shell.automation.querySelector('autoresponse-switch').autoresponsesLabel,pending:shell.autoresponsePending,diagnostic:shell.diagnosticText};
    }),await page.evaluate(()=>({currentGeneration:globalThis.__workspaceFixture.automation.generation,backendGeneration:globalThis.__workspaceFixture.automation.generation,paused:true,label:'Saved responses · On (0 active)',pending:false,diagnostic:'Response set paused for new traffic.'})), 'Late global response switch completion restored obsolete participation');
  } finally {
    await page.evaluate(async()=>{
      globalThis.__releaseResponseToggle?.();await globalThis.__responseTogglePending;
      window.__TAURI_INTERNALS__.invoke=globalThis.__responseToggleOriginal;
      delete globalThis.__responseToggleOriginal;delete globalThis.__releaseResponseToggle;delete globalThis.__responseTogglePending;
    });
  }
}
