import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkResponseBodyEditor(page,root) {
  const form=page.locator('.auto-response-editor'),field=name=>form.locator(`[name="${name}"]`);
  const dialog=page.locator('.response-body-dialog');
  const closed=async()=>{await dialog.waitFor({state:'hidden'});await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').editor===null);};
  const open=async()=>{await form.getByRole('button',{name:'Edit in dialog…',exact:true}).click();await dialog.locator('.monaco-editor').waitFor();};
  const focus=()=>page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').editor.focus());
  const type=async value=>{await focus();await page.keyboard.press('Control+a');await page.keyboard.type(value);};
  const value=()=>page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').model.getValue());
  const original=await field('body').inputValue();
  let releaseStyles,requestedStyles;
  const stylesRequested=new Promise(resolve=>requestedStyles=resolve),stylesReleased=new Promise(resolve=>releaseStyles=resolve);
  const delayStyles=async route=>{requestedStyles();await stylesReleased;await route.fallback();};
  await page.route('**/monaco.css',delayStyles);
  await form.getByRole('button',{name:'Edit in dialog…',exact:true}).click();await stylesRequested;
  await dialog.locator('.body-dialog-content > label textarea').fill('plain text fallback');
  await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').bodyEditorStyles.dispatchEvent(new Event('error')));
  await dialog.getByText('Syntax editor could not load. You can still edit plain text.',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await closed();
  releaseStyles();await page.unroute('**/monaco.css',delayStyles);
  assert.equal(await field('body').inputValue(),original,'Failed syntax loading changed the inline draft');

  // Start saving the plain-text fallback before Monaco's stylesheet finishes loading.
  // Hydration must inherit the pending save lock, including real keyboard input.
  let releaseLateStyles,requestedLateStyles;
  const lateRequested=new Promise(resolve=>requestedLateStyles=resolve),lateReleased=new Promise(resolve=>releaseLateStyles=resolve);
  const delayLateStyles=async route=>{requestedLateStyles();await lateReleased;await route.fallback();};
  await page.route('**/monaco.css',delayLateStyles);
  await form.getByRole('button',{name:'Edit in dialog…',exact:true}).click();await lateRequested;
  await dialog.locator('.body-dialog-content > label textarea').fill('saved before syntax hydration');
  await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.deferAssetEdit=true;s.assetEditPending=false;});
  await dialog.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.assetEditPending);
  releaseLateStyles();await dialog.locator('.monaco-editor').waitFor();await page.unroute('**/monaco.css',delayLateStyles);
  await focus();await page.keyboard.press('Control+End');await page.keyboard.type(' input during save');
  assert.equal(await value(),'saved before syntax hydration','Late syntax hydration allowed edits that the pending save would lose');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferAssetEdit=false;globalThis.__workspaceFixture.finishAssetEdit();});
  await closed();await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').draftDirty);
  assert.equal(await field('body').inputValue(),'saved before syntax hydration');
  await field('body').fill(original);await form.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').draftDirty);

  const html='<html><body><script>globalThis.autoBodyExecuted=true;</script><h1>Offline</h1></body></html>';
  await field('body').fill(html);await open();
  assert.equal(await dialog.getByLabel('Body language',{exact:true}).inputValue(),'html');
  assert.equal(await dialog.getByRole('button',{name:'Save rule',exact:true}).isVisible(),true);
  await type('cancelled body');await dialog.getByRole('button',{name:'Cancel',exact:true}).click();
  await closed();assert.equal(await field('body').inputValue(),html);
  assert.equal(await form.getByRole('button',{name:'Edit in dialog…',exact:true}).evaluate(el=>el.getRootNode().activeElement===el),true);
  assert.equal(await page.evaluate(()=>globalThis.autoBodyExecuted),undefined,'Body markup executed in the workbench');

  for(const [language,text] of [['html',html],['css','body{color:red;background:white;}'],['javascript','function add(a,b){return a+b;}'],['json','{"value":[1,2],"active":true}']]) {
    await open();
    await page.evaluate(text=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor');w.editor.executeEdits('fixture-paste',[{range:w.model.getFullModelRange(),text}]);},text);
    await dialog.getByLabel('Body language',{exact:true}).selectOption(language);
    await dialog.getByRole('button',{name:'Format',exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').bodyFormatting);
    assert.notEqual(await value(),text,`${language} formatting did not change unformatted content`);
    await focus();await page.keyboard.press('Control+z');
    await page.waitForFunction(text=>document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor').model.getValue()===text,text,{timeout:3000});
    assert.equal(await value(),text,`${language} formatting cannot be undone`);
    await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await closed();
  }
  await open();await page.setViewportSize({width:800,height:600});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-body-dialog.png')});
  const apply=await dialog.getByRole('button',{name:'Apply',exact:true}).boundingBox();assert.ok(apply.y+apply.height<600);
  await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-body-dialog-contrast.png')});await page.emulateMedia({forcedColors:'none'});
  await page.setViewportSize({width:1280,height:800});
  await type('applied body');await dialog.getByRole('button',{name:'Apply',exact:true}).click();await closed();
  assert.equal(await field('body').inputValue(),'applied body');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').draftDirty),true);

  const name=await field('name').inputValue();await field('name').fill('Another pending property');await open();
  assert.equal(await dialog.getByRole('button',{name:'Save rule',exact:true}).count(),0,'Modal can save unrelated unsaved properties');
  await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await closed();await field('name').fill(name);
  await open();assert.equal(await dialog.getByRole('button',{name:'Save rule',exact:true}).count(),1,'Restored properties still hide body-only saving');
  await type('save failure draft');
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetError=true;globalThis.__workspaceFixture.deferAssetEdit=true;});
  await dialog.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.assetEditPending);
  assert.equal(await page.evaluate(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor');return w.editor.getOption(w.monaco.editor.EditorOption.readOnly);}),true,'Pending save permits later body edits');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferAssetEdit=false;globalThis.__workspaceFixture.finishAssetEdit();});
  await dialog.getByText(/Fixture asset write failed/).waitFor();assert.equal(await value(),'save failure draft');
  await type('revised after failed save');assert.equal(await value(),'revised after failed save','Failed Save left the body editor locked');
  await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await closed();
  assert.equal(await field('body').inputValue(),'applied body','Cancel after a failed save lost the earlier draft');
  await page.evaluate(()=>globalThis.__workspaceFixture.assetError=false);
  await open();await type('saved from dialog');await dialog.getByRole('button',{name:'Save rule',exact:true}).click();await closed();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').draftDirty);
  assert.equal(await field('body').inputValue(),'saved from dialog');
  assert.equal(await page.evaluate(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor');return w.editor===null&&w.model===null;}),true,'Closing retained editor/model resources');
  // Preserve the fixture's prior body for the remaining workspace checks.
  await field('body').fill(original);await form.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').draftDirty);
}
