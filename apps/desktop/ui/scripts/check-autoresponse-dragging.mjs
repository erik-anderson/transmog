import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkAutoResponseDragging(page,root) {
  const workspace=page.locator('automation-workspace'),list=workspace.locator('.auto-response-list');
  const row=id=>list.locator(`[data-rule-id="${id}"]`);
  const order=()=>page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').rules.map(rule=>rule.id));
  const idle=()=>page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').changingAutoResponse);
  const start=async id=>{const grip=row(id).locator('.rule-drag-grip');await grip.scrollIntoViewIfNeeded();const box=await grip.boundingBox();await page.mouse.move(box.x+box.width/2,box.y+box.height/2);await page.mouse.down();await page.mouse.move(box.x+box.width/2+12,box.y+box.height/2,{steps:4});await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleDragging);};
  const hover=async(x,y)=>{await page.mouse.move(x,y,{steps:5});await page.mouse.move(x,y);};
  const drop=async(id,after)=>{const box=await row(id).boundingBox();await hover(box.x+box.width/2,box.y+box.height*(after?.85:.15));await page.mouse.up();await idle();};
  await page.evaluate(async()=>{
    const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),state=globalThis.__workspaceFixture;
    globalThis.__ruleDragBackup={automation:structuredClone(state.automation),history:w.ruleHistory};
    w.draftDirty=false;await w.cancelAutoResponseEdit();w.ruleHistory=[];w.ruleSelection.clear();
    const source=state.automation.rules.find(rule=>rule.request.responseAsset);
    state.automation={...state.automation,generation:state.automation.generation+1,rules:Array.from({length:55},(_,index)=>({...structuredClone(source),id:'drag-'+index,displayName:'Rule '+index,enabled:index%2===0,priority:-2000000+index,matcher:{...structuredClone(source.matcher),url:{kind:'exact',value:'https://example.test/drag/'+index}}}))};
    await w.refreshAutomation();w.$flushUpdates();
  });
  try {
    await row('drag-0').click();await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').loadingSavedResponse);
    await start('drag-0');await drop('drag-2',true);
    assert.deepEqual((await order()).slice(0,4),['drag-1','drag-2','drag-0','drag-3']);
    await workspace.getByRole('button',{name:'Undo',exact:true}).click();await idle();assert.equal((await order())[0],'drag-0');

    await start('drag-0');const box=await list.boundingBox();await hover(box.x+box.width/2,box.y+box.height-4);
    await page.waitForFunction(()=>{const list=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').autoResponseList;return list.scrollTop>=list.scrollHeight-list.clientHeight-2;},{},{timeout:10000});
    await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-drag-bottom.png')});
    await page.mouse.up();await idle();assert.equal((await order()).at(-1),'drag-0','Holding at the bottom did not allow last-position placement');
    await start('drag-0');await hover(box.x+box.width/2,box.y+4);
    await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').autoResponseList.scrollTop===0,{},{timeout:10000});
    await page.mouse.up();await idle();assert.equal((await order())[0],'drag-0','Holding at the top did not allow first-position placement');
    const unchangedGeneration=await workspace.evaluate(w=>w.automationStatus.generation);
    await start('drag-0');await drop('drag-1',false);assert.equal(await workspace.evaluate(w=>w.automationStatus.generation),unchangedGeneration,'An unchanged position saved another generation');
    await start('drag-0');await hover(box.x+box.width+20,box.y+box.height/2);await page.mouse.up();await idle();assert.equal(await workspace.evaluate(w=>w.automationStatus.generation),unchangedGeneration,'An outside drop reordered rules');

    // A cancellation restores the prior properties and selection, and stops scrolling.
    await start('drag-2');await hover(box.x+box.width/2,box.y+box.height-4);await page.keyboard.press('Escape');await page.mouse.up();
    await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleDragging);
    assert.equal(await row('drag-0').getAttribute('aria-selected'),'true');
    assert.equal(await row('drag-2').getAttribute('aria-selected'),'false');
    const scroll=await list.evaluate(el=>el.scrollTop);await page.waitForTimeout(150);assert.equal(await list.evaluate(el=>el.scrollTop),scroll);

    // Move a noncontiguous selected group in its existing priority order.
    await list.evaluate(el=>el.scrollTop=0);await row('drag-2').click({modifiers:['Control']});
    assert.equal(await workspace.evaluate(w=>w.ruleSelectionCount),2);
    await start('drag-0');await drop('drag-4',true);assert.deepEqual((await order()).slice(0,5),['drag-1','drag-3','drag-4','drag-0','drag-2']);
    await workspace.getByRole('button',{name:'Undo',exact:true}).click();await idle();
    await workspace.getByLabel('Filter rules',{exact:true}).selectOption('disabled');
    const hiddenBefore=(await order()).filter(id=>Number(id.split('-')[1])%2===0);
    await start('drag-1');await drop('drag-5',true);
    assert.deepEqual((await order()).filter(id=>Number(id.split('-')[1])%2===0),hiddenBefore,'Filtered drag reordered hidden rules');
    assert.deepEqual((await order()).filter(id=>Number(id.split('-')[1])%2===1).slice(0,3),['drag-3','drag-5','drag-1']);
    await workspace.getByLabel('Filter rules',{exact:true}).selectOption('all');

    // Reordering the edited rule preserves its unsaved body and editor selection.
    await row('drag-0').click();await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').loadingSavedResponse);
    await workspace.locator('[name="body"]').fill('pending body');await start('drag-0');await drop('drag-4',true);
    assert.equal(await workspace.locator('[name="body"]').inputValue(),'pending body');assert.equal(await workspace.evaluate(w=>w.draftDirty),true);
    // A changed generation cannot be overwritten by a drag started earlier.
    const beforeStale=await order();await start('drag-0');
    const target=await row('drag-6').boundingBox();await hover(target.x+target.width/2,target.y+target.height*.85);
    await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-drag-contrast.png')});await page.emulateMedia({forcedColors:'none'});
    await page.evaluate(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');globalThis.__workspaceFixture.automation.generation++;w.renderAutomation(structuredClone(globalThis.__workspaceFixture.automation));});
    await drop('drag-6',true);assert.deepEqual(await order(),beforeStale);assert.match(await workspace.evaluate(w=>w.automationText),/changed while dragging/);
    await page.evaluate(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');w.draftDirty=false;globalThis.__workspaceFixture.uaRejectValidation=true;});
    await start('drag-0');await drop('drag-6',true);assert.deepEqual(await order(),beforeStale);assert.match(await workspace.evaluate(w=>w.automationText),/Fixture validation failed/);
  }finally {
    await page.mouse.up();await page.evaluate(async()=>{
      const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace'),state=globalThis.__workspaceFixture,backup=globalThis.__ruleDragBackup;
      w.finishRuleDrag();w.draftDirty=false;await w.cancelAutoResponseEdit();w.ruleSelection.clear();w.ruleHistory=backup.history;w.ruleSearch='';w.ruleFilter='all';state.uaRejectValidation=false;
      state.automation={...backup.automation,generation:state.automation.generation+1};await w.refreshAutomation();delete globalThis.__ruleDragBackup;
    });
  }
  await page.setViewportSize({width:800,height:600});await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-rules-compact.png')});await page.setViewportSize({width:1280,height:800});
}
