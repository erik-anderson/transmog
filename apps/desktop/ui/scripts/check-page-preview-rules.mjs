import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkPagePreviewRules(page, root) {
  const saved=await page.evaluate(()=>({automation:structuredClone(globalThis.__workspaceFixture.automation),viewerMode:document.querySelector('app-shell').traffic.viewerMode}));
  const traffic=page.locator('traffic-workspace'), dialog=traffic.locator('.captured-page-warning[open]');
  try {
    await page.evaluate(()=>{
      const state=globalThis.__workspaceFixture;
      const rule=(id,enabled,priority)=>({id,displayName:id==='disabled'?'Disabled CSS experiment with a long descriptive name '.repeat(3):'Saved HTML response',enabled,priority,revision:7,matcher:{method:'GET'},request:{responseAsset:'saved@3'}});
      state.automation={generation:81,autoresponsesEnabled:false,rules:[rule('disabled',false,2),rule('enabled',true,1)],diagnostics:[],usage:[],candidateCount:0,historyCount:0};
      const traffic=document.querySelector('app-shell').traffic;traffic.viewerMode=false;traffic.showCapturedPage();
    });
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewRulesBusy);
    assert.equal(await dialog.getByLabel('Responses',{exact:true}).inputValue(),'captured');
    await dialog.getByLabel('Responses',{exact:true}).selectOption('modified');
    await dialog.getByText('Select saved rules in priority order',{exact:true}).click();
    const choices=dialog.locator('.preview-rule-picker input');
    assert.equal(await choices.nth(0).isChecked(),true);assert.equal(await choices.nth(1).isChecked(),false);
    await choices.nth(1).check();
    assert.match(await dialog.textContent(),/For this preview only/);
    await page.evaluate(()=>document.querySelector('app-shell').traffic.previewDraftDirty=true);
    const before=await page.evaluate(()=>globalThis.__workspaceFixture.calls.open_captured_page??0);
    await dialog.getByRole('button',{name:'Open preview',exact:true}).click();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.open_captured_page??0),before);
    await dialog.getByLabel('Exclude unsaved editor changes and use the selected saved versions',{exact:true}).check();
    assert.equal(await dialog.getByText('Select saved rules and resolve or explicitly exclude unsaved changes before opening.',{exact:true}).count(),0);
    for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
      await page.setViewportSize({width,height});
      await dialog.getByRole('button',{name:'Open preview',exact:true}).scrollIntoViewIfNeeded();
      assert.ok((await dialog.boundingBox()).height<=height);
      await page.screenshot({path:resolve(root,`../../../target/ui-check/preview-rules-${width}.png`)});
    }
    await page.evaluate(()=>document.documentElement.style.zoom='1.25');
    try {
      await dialog.getByRole('button',{name:'Open preview',exact:true}).scrollIntoViewIfNeeded();
      const button=await dialog.getByRole('button',{name:'Open preview',exact:true}).boundingBox();assert.ok(button.y>=0&&button.y+button.height<=600,'Zoom clipped the preview action');
      await page.screenshot({path:resolve(root,'../../../target/ui-check/preview-rules-zoom.png')});
    } finally {await page.evaluate(()=>document.documentElement.style.zoom='');}
    await page.emulateMedia({colorScheme:'dark',forcedColors:'active'});
    await page.screenshot({path:resolve(root,'../../../target/ui-check/preview-rules-forced-colors.png')});
    await page.emulateMedia({colorScheme:'light',forcedColors:'none'});
    await dialog.getByRole('button',{name:'Open preview',exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
    const args=await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs);
    assert.deepEqual(args.options,{scope:'original-trace',ruleIds:['enabled','disabled'],generation:81});
    assert.equal(args.enableScripts,false);
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.autoresponsesEnabled),false);
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules[0].enabled),false);
    await page.evaluate(()=>document.querySelector('app-shell').traffic.showRelatedPreview('captured'));
    assert.equal(await dialog.getByLabel('Responses',{exact:true}).inputValue(),'captured');
    await dialog.getByRole('button',{name:'Open preview',exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.sourceLabel),'captured-fixture');
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.options),{scope:'original-trace'});
    await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.deferPagePreview=true;state.ignorePreviewCancellation=true;state.pagePreviewCanceled=false;document.querySelector('app-shell').traffic.showCapturedPage();});
    await dialog.getByRole('button',{name:'Open preview',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('app-shell').traffic.previewPageBusy);
    await page.keyboard.press('Escape');
    const files=traffic.getByRole('button',{name:'Files',exact:true});await files.focus();
    await page.evaluate(()=>globalThis.__workspaceFixture.releasePagePreview());
    await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
    assert.equal(await files.evaluate(node=>node.getRootNode().activeElement===node),true,'Canceled preparation stole focus on late success');
    await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.deferPagePreview=false;state.ignorePreviewCancellation=false;state.pagePreviewCanceled=false;document.querySelector('app-shell').traffic.showCapturedPage();});
    await page.keyboard.press('Escape');
    assert.equal(await dialog.count(),0);
  } finally {
    await page.evaluate(saved=>{globalThis.__workspaceFixture.automation=saved.automation;const t=document.querySelector('app-shell').traffic;t.viewerMode=saved.viewerMode;t.previewDraftDirty=false;if(t.previewPageDialog.open)t.closeCapturedPageWarning();},saved);
    await page.emulateMedia({colorScheme:'light',forcedColors:'none'});
    await page.setViewportSize({width:1280,height:800});
  }
}
