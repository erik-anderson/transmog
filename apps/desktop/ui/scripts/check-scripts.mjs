import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkScripts(page, root) {
  const scripts=page.locator('script-editor');
  const sourceSurface=scripts.locator('editor-surface').first();
  const state=()=>scripts.locator('.script-state');
  const waitIdle=()=>page.waitForFunction(()=>{const editor=document.querySelector('app-shell').shadowRoot.querySelector('script-editor');return editor.scriptReady&&!editor.scriptBusy;});
  const append=async text=>scripts.evaluate((editor,text)=>editor.sourceEditor.setValue(editor.sourceEditor.getValue()+text),text);
  const fixture=async values=>page.evaluate(values=>Object.assign(globalThis.__workspaceFixture,values),values);
  await waitIdle();
  assert.deepEqual(await scripts.evaluate(async editor=>{
    const {getTypeScriptWorker}=await import('/monaco.js');
    const worker=await (await getTypeScriptWorker())(editor.draftModel.uri);
    return (await worker.getSemanticDiagnostics(editor.draftModel.uri.toString())).map(item=>item.messageText);
  }),[],'Default script API import has TypeScript errors');
  assert.ok((await scripts.locator('.monaco-editor-host').first().boundingBox()).height>=192,'Source editor has insufficient space');
  await sourceSurface.getByRole('button',{name:'Expand editor',exact:true}).click();
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),true);
  assert.equal(await sourceSurface.getByRole('button',{name:'Save & enable',exact:true}).isVisible(),true);
  await page.keyboard.press('Escape');
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),false);

  // One action validates, saves, and enables; reopening selects the same source and settings.
  await scripts.getByRole('button',{name:'Save & enable',exact:true}).click();await waitIdle();
  assert.equal(await scripts.getByRole('button',{name:'Pause script',exact:true}).count(),1);
  assert.match(await state().textContent(),/Draft saved/);
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty),false);
  const activated=await page.evaluate(()=>structuredClone(globalThis.__workspaceFixture.savedScript));
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].source),activated.source);
  await append('\n// edit applied with one click');
  assert.match(await state().textContent(),/Unapplied changes/);
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();await waitIdle();
  assert.match(await scripts.locator('output').textContent(),/Saved and enabled/);
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty),false);
  await scripts.locator('.script-settings > summary').click();
  await scripts.getByLabel('Host filter (optional)',{exact:true}).fill('sandbox.example.test');
  await scripts.getByLabel('Writable headers',{exact:true}).fill('User-Agent, user-agent');
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();await waitIdle();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.prefilter.host),'sandbox.example.test');
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty||editor.scriptUnapplied),false,'Applied capability settings differ from the saved baseline');
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.capabilities.writeHeaders),['user-agent'],'Writable headers were not normalized as a set');
  await scripts.locator('.script-settings > summary').click();

  // Optional sandbox automatically validates and describes its actual input and action.
  await scripts.locator('.script-test summary').first().click();
  await scripts.getByLabel('Test URL',{exact:true}).fill('https://sandbox.example.test:8443/users/42?ready=yes');
  await scripts.getByLabel('Method',{exact:true}).fill('PATCH');
  await scripts.getByLabel('Request headers',{exact:true}).fill('X-Test: yes');
  await scripts.getByRole('button',{name:'Test in sandbox',exact:true}).click();await waitIdle();
  assert.match(await scripts.locator('.script-test-result').textContent(),/Completed for this input.*onRequestHead.*Action: headers/s);
  assert.deepEqual(await page.evaluate(()=>{const input=globalThis.__workspaceFixture.scriptTestInput.request;return {method:input.method,host:input.host,port:input.port,path:input.path,query:input.query};}),{method:'PATCH',host:'sandbox.example.test',port:8443,path:'/users/42',query:'ready=yes'});
  await scripts.getByText('Test input and returned changes',{exact:true}).click();
  assert.match(await scripts.locator('.script-test-result pre').textContent(),/User-Agent/);
  await scripts.getByLabel('Method',{exact:true}).fill('GET');
  assert.match(await scripts.locator('.script-test-result').textContent(),/Outdated/);
  await fixture({deferScriptTest:true});
  await scripts.getByRole('button',{name:'Test in sandbox',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.scriptTestPending);
  await scripts.getByLabel('Method',{exact:true}).fill('POST');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferScriptTest=false;globalThis.__workspaceFixture.finishScriptTest();});await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.testOutdated),true,'Delayed test input was treated as current');

  // Source/capability edits during validation must not proceed to activation.
  await fixture({deferScriptValidation:true});
  const beforeRevision=await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.revision);
  await append('\n// before delayed validation');
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.scriptValidationPending);
  await append('\n// changed while validating');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferScriptValidation=false;globalThis.__workspaceFixture.finishScriptValidation();});await waitIdle();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.revision),beforeRevision);
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty),true);

  // The generation read while preparing a draft also guards validation start.
  await fixture({changeScriptBeforeValidation:true});
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();await waitIdle();
  assert.match(await scripts.locator('output').textContent(),/workspace changed/);
  assert.match(await page.evaluate(()=>globalThis.__workspaceFixture.savedScript.source),/saved concurrently in another window/);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.revision),beforeRevision);

  // A failed activation keeps both the previous live version and the edited draft.
  await fixture({scriptActivationError:true});
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();await waitIdle();
  assert.match(await scripts.locator('output').textContent(),/Activation failed/);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].manifest.revision),beforeRevision);
  assert.match(await scripts.evaluate(editor=>editor.sourceEditor.getValue()),/changed while validating/);
  await fixture({scriptActivationError:false,deferScriptActivation:true});
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.scriptActivationPending);
  await append('\n// newer than activation snapshot');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferScriptActivation=false;globalThis.__workspaceFixture.finishScriptActivation();});await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty&&editor.scriptUnapplied),true,'Delayed activation marked newer edits saved');
  assert.doesNotMatch(await page.evaluate(()=>globalThis.__workspaceFixture.savedScript.source),/newer than activation snapshot/);

  // Save draft remains separate, including Ctrl+S and edits made while the save is pending.
  await fixture({deferScriptSave:true});
  await scripts.getByRole('button',{name:'Save draft',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.scriptSavePending);
  await append('\n// newer than saved snapshot');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferScriptSave=false;globalThis.__workspaceFixture.finishScriptSave();});await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty),true);
  await scripts.evaluate(editor=>editor.sourceEditor.focus());await page.keyboard.press('Control+s');await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty),false);
  assert.equal(await scripts.evaluate(editor=>editor.scriptUnapplied),true);
  assert.doesNotMatch(await page.evaluate(()=>globalThis.__workspaceFixture.activeScripts[0].source),/newer than saved snapshot/);

  // Comparison includes settings and stale comparisons are labelled.
  await scripts.getByRole('button',{name:'More',exact:true}).click();
  await scripts.getByRole('button',{name:'Compare with active',exact:true}).click();await waitIdle();
  assert.match(await scripts.locator('.script-settings-diff').textContent(),/writeHeaders/);
  await append('\n// comparison is now old');
  assert.equal(await scripts.evaluate(editor=>editor.diffStale),true);
  await scripts.getByRole('button',{name:'More',exact:true}).click();
  await scripts.getByRole('button',{name:'Compare with active',exact:true}).click();await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.diffStale),false);
  await scripts.getByRole('button',{name:'Pause script',exact:true}).click();await waitIdle();
  assert.equal(await scripts.evaluate(editor=>editor.diffStale),true,'Pausing left a comparison labelled current');
  await scripts.getByRole('button',{name:'Close comparison',exact:true}).click();
  assert.equal(await scripts.evaluate(editor=>editor.diffModels.length),0);
  assert.match(await state().textContent(),/Paused/);
  assert.match(await scripts.evaluate(editor=>editor.sourceEditor.getValue()),/comparison is now old/);
  await scripts.getByRole('button',{name:'Save & enable',exact:true}).click();await waitIdle();

  // Simulate reopening a legacy workspace: active beats older/equal saved revisions;
  // a truly newer saved draft stays available without replacing the running source.
  const reopen=async()=>scripts.evaluate(async editor=>{
    editor.sourceEditor.dispose();editor.sourceEditor=null;editor.draftModel.dispose();editor.draftModel=null;
    editor.declarationsDisposable.dispose();editor.declarationsDisposable=null;editor.scriptReady=false;
    await editor.initializeScriptEditor();
  });
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.savedScript={...state.savedScript,source:'// stale separately saved source'};});
  await reopen();
  assert.match(await scripts.evaluate(editor=>editor.sourceEditor.getValue()),/comparison is now old/);
  assert.equal(await scripts.evaluate(editor=>editor.scriptDirty||editor.scriptUnapplied),false);
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;const active=state.activeScripts[0];const {source,...unused}=active;state.savedScript={...active.manifest,source:active.source+'\n// newest saved draft',revision:active.manifest.revision+1};delete state.savedScript.sourceHash;delete state.savedScript.apiRevision;});
  await reopen();
  assert.match(await scripts.evaluate(editor=>editor.sourceEditor.getValue()),/newest saved draft/);
  assert.equal(await scripts.evaluate(editor=>!editor.scriptDirty&&editor.scriptUnapplied),true);
  await scripts.getByRole('button',{name:'Save & apply',exact:true}).click();await waitIdle();

  // More remains anchored inside an expanded editor and Escape restores focus.
  await sourceSurface.getByRole('button',{name:'Expand editor',exact:true}).click();
  await scripts.getByRole('button',{name:'More',exact:true}).click();
  const button=await scripts.getByRole('button',{name:'More',exact:true}).boundingBox();
  const menu=await scripts.locator('#script-actions-menu').boundingBox();
  assert.ok(menu.x<button.x+button.width&&menu.x+menu.width>button.x&&Math.abs(menu.y-button.y-button.height)<12,'Script menu is detached from More');
  await scripts.getByRole('button',{name:'Validate only',exact:true}).focus();
  await page.keyboard.press('Escape');
  await page.waitForFunction(()=>{const editor=document.querySelector('app-shell').shadowRoot.querySelector('script-editor');return editor.scriptMore===editor.getRootNode().activeElement;});
  assert.equal(await scripts.getByRole('button',{name:'More',exact:true}).evaluate(button=>button===document.activeElement||button===button.getRootNode().activeElement),true);
  await sourceSurface.getByRole('button',{name:'Return to panel',exact:true}).click();
  await scripts.locator('.script-test summary').first().click();
  for(const [name,width,height] of [['wide',1440,900],['compact',960,720],['small',800,600]]){
    await page.setViewportSize({width,height});
    await scripts.locator('.script-state').scrollIntoViewIfNeeded();
    const bounds=await scripts.getByRole('button',{name:'Pause script',exact:true}).boundingBox();
    assert.ok(bounds.x>=0&&bounds.x+bounds.width<=width&&bounds.y+bounds.height<height,'Primary script action is clipped');
    await page.screenshot({path:resolve(root,`../../../target/ui-check/scripts-${name}.png`)});
  }
  await page.emulateMedia({colorScheme:'dark'});
  await scripts.locator('.monaco-editor.vs-dark').first().waitFor({state:'visible'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/scripts-dark.png')});
  await page.emulateMedia({forcedColors:'active'});
  await scripts.locator('.monaco-editor.hc-black').first().waitFor({state:'visible'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/scripts-forced-colors.png')});
  await page.emulateMedia({colorScheme:'light',forcedColors:'none'});
  await page.setViewportSize({width:1280,height:800});
}
