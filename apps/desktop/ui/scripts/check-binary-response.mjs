import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkBinaryResponse(page,root) {
  const form=page.locator('.auto-response-editor'),field=name=>form.locator(`[name="${name}"]`);
  const view=form.getByLabel('Response body view',{exact:true});
  const saved=()=>page.waitForFunction(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');return !w.savingAutoResponse&&!w.loadingSavedResponse&&!w.draftDirty;});
  const initial=await page.evaluate(()=>{const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');return {assets:structuredClone(s.assets),automation:structuredClone(s.automation),id:w.editingAutoResponseId};});
  try {
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');
      const rule=w.rules.find(rule=>rule.id===w.editingAutoResponseId),asset=s.assets.find(asset=>asset.id+'@'+asset.revision===rule.request.responseAsset);
      asset.rawBody=[0,255,128,13,10];asset.display='';asset.mediaType='application/octet-stream';asset.bodyBytes=5;s.assetTextEncoding=null;
      await w.editAutoResponse(rule,true);
    });
    assert.equal(await view.inputValue(),'hex');
    assert.equal(await field('body').inputValue(),'00 ff 80 0d 0a');
    assert.equal(await field('body').evaluate(input=>input.readOnly),false,'Complete saved binary body is read-only');
    await view.selectOption('text');assert.equal(await view.inputValue(),'hex');
    await form.getByText(/unsupported text encoding/).waitFor();
    const count=await page.evaluate(()=>globalThis.__workspaceFixture.assets.length);
    assert.equal(await form.getByRole('button',{name:'Save rule',exact:true}).isDisabled(),true,'Changing the body view dirtied unchanged bytes');
    await field('body').focus();await page.keyboard.press('Control+s');await saved();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets.length),count,'Saving unchanged binary bytes created a replacement');
    await field('body').fill('00 ff 81 0d 0a');await form.getByRole('button',{name:'Save rule',exact:true}).click();await saved();
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.lastAssetEdit.decodedBody),[0,255,129,13,10]);
    await page.setViewportSize({width:800,height:600});
    await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-binary.png')});
    await page.setViewportSize({width:1280,height:800});

    // Inline textarea normalization cannot replace unchanged original CRLF bytes.
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');
      const rule=w.rules.find(rule=>rule.id===w.editingAutoResponseId),asset=s.assets.find(asset=>asset.id+'@'+asset.revision===rule.request.responseAsset);
      asset.rawBody=[65,13,10,66,10,67];asset.display='A\r\nB\nC';asset.mediaType='text/plain; charset=utf-8';delete s.assetTextEncoding;
      await w.editAutoResponse(rule,true);
    });
    await view.selectOption('hex');assert.equal(await field('body').inputValue(),'41 0d 0a 42 0a 43');
    await view.selectOption('text');assert.equal(await field('body').inputValue(),'A\nB\nC');
    await field('status').fill('202');await form.getByRole('button',{name:'Save rule',exact:true}).click();await saved();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastAssetEdit.decodedBody),null,'A view switch normalized original bytes when saving metadata');

    // Newly captured non-Unicode responses enter the same byte-preserving editor.
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');
      s.editableBodyBytes=[0,255,128,13,10];s.assetTextEncoding=null;w.draftDirty=false;await w.beginAutoResponseFromSelected();
    });
    assert.equal(await view.inputValue(),'hex');assert.equal(await field('body').inputValue(),'00 ff 80 0d 0a');
    await field('editBody').check();await field('body').fill('00 ff 82 0d 0a');
    await form.getByRole('button',{name:'Save rule',exact:true}).click();await saved();
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.lastAsset.decodedBody),[0,255,130,13,10]);

    // A bounded preview remains read-only and cannot become a replacement on save.
    await page.evaluate(async()=>{
      const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');
      s.editableBodyTruncated=true;w.draftDirty=false;await w.beginAutoResponseFromSelected();
    });
    assert.equal(await field('editBody').isDisabled(),true);
    assert.equal(await field('body').evaluate(input=>input.readOnly),true);
    await form.getByText(/bounded preview is read-only/).waitFor();
    await form.getByRole('button',{name:'Save rule',exact:true}).click();await saved();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastAsset.decodedBody),null,'Saving a rule promoted a partial body preview to a replacement');
  } finally {
    await page.evaluate(async initial=>{
      const s=globalThis.__workspaceFixture,w=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');
      s.assets=initial.assets;s.automation={...initial.automation,generation:s.automation.generation+1};delete s.assetTextEncoding;delete s.editableBodyBytes;delete s.editableBodyTruncated;
      w.draftDirty=false;await w.refreshAutomation();await w.editAutoResponse(w.rules.find(rule=>rule.id===initial.id),true);
    },initial);
    await page.setViewportSize({width:1280,height:800});
  }
}
