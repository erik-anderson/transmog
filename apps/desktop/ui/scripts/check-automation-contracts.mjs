import assert from 'node:assert/strict';
import {resolve} from 'node:path';
import {checkUnifiedRules} from './check-unified-rules.mjs';

export async function checkAutomationContracts(page,root) {
  await checkUnifiedRules(page,root);
  await page.setViewportSize({width:1440,height:900});
  const workspace=page.locator('automation-workspace'),grid=workspace.getByRole('grid',{name:'Traffic rules'});
  const headers=workspace.locator('header-rules[editor-kind="headers"]');
  const saved=workspace.locator('.auto-response-editor'),browser=workspace.locator('user-agent-overrides');
  const idle=()=>page.waitForFunction(()=>!document.querySelector('app-shell').automation.changingAutoResponse);
  const current=name=>page.evaluate(name=>structuredClone(globalThis.__workspaceFixture.automation.rules.find(rule=>rule.displayName===name)),name);
  const row=name=>grid.locator('tr[data-rule-id]').filter({has:page.getByText(name,{exact:true})});
  const choose=async name=>{await row(name).click();};
  await choose('Outbound headers');
  await headers.getByRole('button',{name:'Discard changes',exact:true}).click();
  const name=headers.getByLabel('Rule name',{exact:true}),save=headers.getByRole('button',{name:'Save rule',exact:true});
  assert.equal(await save.isDisabled(),true);
  await name.fill('Changed name');assert.equal(await save.isEnabled(),true);
  await name.fill('Outbound headers');assert.equal(await save.isDisabled(),true,'Returning to saved content should clear dirty state');
  await name.fill('Unsaved header');
  await row('Outbound headers').getByRole('button',{name:'Disable Outbound headers',exact:true}).click();await idle();
  assert.equal(await name.inputValue(),'Unsaved header');assert.equal((await current('Outbound headers')).enabled,false);
  assert.equal(await save.isEnabled(),true,'Activation discarded the draft');
  await workspace.locator('.auto-response-undo').getByRole('button',{name:'Undo',exact:true}).click();await idle();
  assert.equal((await current('Outbound headers')).enabled,true);assert.equal(await name.inputValue(),'Unsaved header');
  await page.evaluate(()=>globalThis.__workspaceFixture.uaRejectValidation=true);
  await row('Outbound headers').getByRole('button',{name:'Disable Outbound headers',exact:true}).click();await idle();
  assert.equal((await current('Outbound headers')).enabled,true);
  assert.equal(await workspace.locator('.automation-feedback').isVisible(),true);
  assert.match(await workspace.locator('.automation-feedback').innerText(),/Rule change failed/);
  await page.evaluate(()=>globalThis.__workspaceFixture.uaRejectValidation=false);
  await workspace.locator('.automation-feedback').getByRole('button',{name:'Dismiss'}).click();
  await headers.getByRole('button',{name:'Discard changes',exact:true}).click();

  await choose('Offline response');
  const body=saved.locator('[name="body"]'),saveResponse=saved.getByRole('button',{name:'Save rule',exact:true});
  assert.equal(await saveResponse.isDisabled(),true);
  await body.fill('Unsaved response');assert.equal(await saveResponse.isEnabled(),true);
  await row('Offline response').getByRole('button',{name:'Disable Offline response',exact:true}).click();await idle();
  assert.equal(await body.inputValue(),'Unsaved response');
  await saved.getByRole('button',{name:'Discard changes',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').automation.loadingSavedResponse);
  assert.equal(await body.inputValue(),'');assert.equal(await saveResponse.isDisabled(),true);
  await body.fill('Saved response');await saveResponse.click();
  await page.waitForFunction(()=>{const w=document.querySelector('app-shell').automation;return !w.savingAutoResponse&&!w.loadingSavedResponse&&!w.draftDirty;});
  assert.equal(await saveResponse.isDisabled(),true);
  const revision=(await current('Offline response')).revision;
  await body.focus();await page.keyboard.press('Control+s');
  assert.equal((await current('Offline response')).revision,revision,'Saving unchanged content created another revision');

  await choose('Browser identity');
  const uaName=browser.locator('[name="uaName"]'),uaSave=browser.getByRole('button',{name:'Save rule',exact:true});
  assert.equal(await uaSave.isDisabled(),true);
  await uaName.fill('Unsaved browser');
  await row('Browser identity').getByRole('button',{name:'Enable Browser identity',exact:true}).click();await idle();
  assert.equal(await uaName.inputValue(),'Unsaved browser');assert.equal(await uaSave.isEnabled(),true);
  await browser.getByRole('button',{name:'Discard changes',exact:true}).click();
  assert.equal(await uaName.inputValue(),'Browser identity');assert.equal(await uaSave.isDisabled(),true);
  const inspections=await page.evaluate(()=>globalThis.__workspaceFixture.calls.inspect_response_asset);
  await choose('Offline response');assert.equal(await body.inputValue(),'Saved response');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.inspect_response_asset),inspections,'Resuming a mounted saved-response editor reloaded its draft');

  await choose('Outbound headers');
  const navigation=page.getByRole('button',{name:'Toggle navigation labels',exact:true});
  const originalExpanded=await navigation.getAttribute('aria-expanded');
  for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
    await page.setViewportSize({width,height});
    for(const expanded of ['true','false']) {
      if(await navigation.getAttribute('aria-expanded')!==expanded)await navigation.click();
      const bounds=await save.boundingBox();
      assert.ok(bounds&&bounds.y>=0&&bounds.y+bounds.height<=height,'Save must remain visible without scrolling');
      assert.equal(await save.evaluate(button=>{
        const box=button.getBoundingClientRect(),root=button.getRootNode();
        return [box.top+2,box.bottom-2].every(y=>button.contains(root.elementFromPoint(box.left+box.width/2,y)));
      }),true,`Save is clipped at ${width}×${height} with navigation expanded=${expanded}`);
      await page.screenshot({path:resolve(root,`../../../target/ui-check/automation-contracts-${width}-${expanded==='true'?'expanded':'collapsed'}.png`)});
    }
  }
  if(await navigation.getAttribute('aria-expanded')!==originalExpanded)await navigation.click();
  await page.setViewportSize({width:1440,height:900});
  await checkUnsavedDialogs(page,workspace,choose,root);
}

async function checkUnsavedDialogs(page,workspace,choose,root) {
  for(const item of [
    {name:'Outbound headers',editor:'headerRulesEditor',selector:'header-rules[editor-kind="headers"]',field:'[name="headerRuleName"]',dialog:'header-rules[editor-kind="headers"] .header-unsaved',command:'change_header_rule',rejection:'headerReject'},
    {name:'Browser identity',editor:'userAgentOverrides',selector:'user-agent-overrides',field:'[name="uaName"]',dialog:'.ua-unsaved',command:'validate_automation',rejection:'uaRejectValidation'},
    {name:'Offline response',editor:null,selector:'.auto-response-editor',field:'[name="name"]',dialog:'.unsaved-rule-dialog',command:'validate_automation',rejection:'uaRejectValidation'},
  ]) {
    await choose(item.name);
    const field=workspace.locator(item.selector).locator(item.field),dialog=workspace.locator(item.dialog);
    await field.fill(item.name+' pending');
    await page.evaluate(({editor,command,rejection})=>{
      const w=document.querySelector('app-shell').automation,state=globalThis.__workspaceFixture;
      state.holdCommand=command;state[rejection]=true;globalThis.__ruleLeaveResult=null;
      const pending=editor?w[editor].canLeaveDraft():w.canLeaveEditor();
      void pending.then(result=>globalThis.__ruleLeaveResult=result);
    },item);
    await dialog.getByRole('button',{name:'Save and continue',exact:true}).click();
    await page.waitForFunction(()=>typeof globalThis.__workspaceFixture.finishHeldCommand==='function');
    for(const label of ['Save and continue','Discard changes','Keep editing'])assert.equal(await dialog.getByRole('button',{name:label,exact:true}).isDisabled(),true,`${item.name}: pending save permits another leave decision`);
    await page.keyboard.press('Escape');assert.equal(await dialog.isVisible(),true,`${item.name}: Escape abandons a pending save`);
    await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.finishHeldCommand();delete state.finishHeldCommand;});
    await dialog.getByRole('alert').filter({hasText:/[Ff]ailed|not (?:be )?saved/}).waitFor();
    assert.equal(await field.inputValue(),item.name+' pending');assert.equal(await dialog.isVisible(),true);
    await page.setViewportSize({width:800,height:600});
    if(item.editor==='userAgentOverrides')await page.emulateMedia({colorScheme:'dark'});
    if(item.editor===null)await page.emulateMedia({forcedColors:'active'});
    for(const button of await dialog.getByRole('button').all())assert.equal(await button.evaluate(node=>{const r=node.getBoundingClientRect();return r.x>=0&&r.y>=0&&r.right<=innerWidth&&r.bottom<=innerHeight&&node.contains(node.getRootNode().elementFromPoint(r.x+r.width/2,r.y+r.height/2));}),true,`${item.name}: failed save hides a recovery action`);
    await page.screenshot({path:resolve(root,`../../../target/ui-check/automation-review-${item.editor??'saved-response'}-error.png`)});
    await page.emulateMedia({colorScheme:null,forcedColors:'none'});
    await page.setViewportSize({width:1440,height:900});
    await page.evaluate(rejection=>globalThis.__workspaceFixture[rejection]=false,item.rejection);
    await dialog.getByRole('button',{name:'Save and continue',exact:true}).click();
    await dialog.waitFor({state:'hidden'});
    await page.waitForFunction(()=>globalThis.__ruleLeaveResult===true);
    await field.fill(item.name);
    await workspace.locator(item.selector).getByRole('button',{name:'Save rule',exact:true}).click();
    await page.waitForFunction(()=>{const w=document.querySelector('app-shell').automation;return !w.savingAutoResponse&&!w.loadingSavedResponse&&!w.headerRulesEditor.hrBusy&&!w.userAgentOverrides.uaBusy;});
    if(item.editor!=='userAgentOverrides') {
      await field.fill('');
      await page.evaluate(editor=>{const w=document.querySelector('app-shell').automation;globalThis.__ruleLeaveResult=null;void (editor?w[editor].canLeaveDraft():w.canLeaveEditor()).then(result=>globalThis.__ruleLeaveResult=result);},item.editor);
      await dialog.getByRole('button',{name:'Save and continue',exact:true}).click();
      await dialog.waitFor({state:'hidden'});await page.waitForFunction(()=>globalThis.__ruleLeaveResult===false);
      assert.equal(await field.evaluate(node=>node.getRootNode().activeElement===node),true,`${item.name}: invalid input stays behind the modal`);
      await field.fill(item.name);
    }
  }
}
