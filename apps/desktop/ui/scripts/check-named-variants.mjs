import assert from 'node:assert/strict';
import {resolve} from 'node:path';

// Runs after connected authoring has created a real saved experiment in this fixture.
export async function checkNamedVariants(page,root){
 const manager=page.locator('experiment-variants');
 const editor=page.locator('automation-workspace .auto-response-editor');
 const settle=()=>page.waitForFunction(()=>!document.querySelector('app-shell').automation.experimentVariants.variantBusy);
 const state=()=>page.evaluate(()=>structuredClone(globalThis.__workspaceFixture.automation));
 await page.evaluate(async()=>{const shell=document.querySelector('app-shell');await shell.activateView('automation');const a=shell.automation;a.draftDirty=false;await a.refreshAutomation();a.showAutomationSection('experiments');});
 await manager.getByLabel('Experiment',{exact:true}).waitFor();
 const before=await state(),experiment=before.experiments[0];
 await manager.getByLabel('Variant',{exact:true}).selectOption('baseline');
 assert.deepEqual((await state()).rules,before.rules,'Selecting Baseline changed live rules');
 await manager.getByText('Names, duplicates and archive',{exact:true}).click();
 const name='Patched script with a long customer hypothesis and a second response';
 await manager.getByLabel('Duplicate name',{exact:true}).fill(name);
 await manager.getByRole('button',{name:'Duplicate saved revision',exact:true}).click();await settle();
 const variantId=await manager.getByLabel('Variant',{exact:true}).inputValue();
 assert.notEqual(variantId,'baseline');assert.deepEqual((await state()).rules,before.rules);
 // Status refreshes must preserve partially typed metadata.
 await manager.getByLabel('Variant name',{exact:true}).fill(name+' unsaved');
 await page.evaluate(()=>document.querySelector('app-shell').automation.refreshAutomation());
 assert.equal(await manager.getByLabel('Variant name',{exact:true}).inputValue(),name+' unsaved');
 await manager.getByRole('button',{name:'Save variant name',exact:true}).click();await settle();
 assert.equal((await state()).experiments[0].variants.find(v=>v.id===variantId).name,name+' unsaved');
 // Add two response rules using the shared editor. Neither save activates them.
 for(const [index,url] of ['https://example.test/a.js','https://example.test/b.js'].entries()){
  await manager.getByRole('button',{name:'Add selected response',exact:true}).click();
  await page.waitForFunction(()=>{const a=document.querySelector('app-shell').automation;return !a.editorHidden&&!a.loadingSavedResponse&&!a.editBodyDisabled;});
  await editor.getByLabel('Rule name',{exact:true}).fill(index?'Second script':'First script');
  await editor.getByLabel('Matching address',{exact:true}).fill(url);
  await editor.getByRole('button',{name:'Save experiment',exact:true}).click();
  await page.waitForFunction(()=>{const a=document.querySelector('app-shell').automation;return !a.savingAutoResponse&&!a.loadingSavedResponse&&!a.draftDirty;});
  assert.deepEqual((await state()).rules,before.rules,'Saving variant rules changed live traffic');
  await editor.getByRole('button',{name:'Named variants and revision history…',exact:true}).click();
 }
 assert.equal(await manager.locator('.variant-rule-list li').count(),2);
 await manager.getByRole('button',{name:'Move Second script later',exact:true}).click();await settle();
 let saved=(await state()).experiments[0].variants.find(v=>v.id===variantId);
 assert.equal(saved.revisions.at(-1).rules[0].displayName,'First script');
 // Review overlaps, then switch while keeping unrelated rules and global pause.
 await page.evaluate(()=>globalThis.__workspaceFixture.variantOverlaps=['First script may overlap another saved rule (earlier live rule)']);
 await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).click();await settle();
 assert.deepEqual((await state()).rules,before.rules);assert.equal(await manager.locator('.variant-overlaps li').count(),1);
 await manager.getByRole('button',{name:'Activate despite overlaps',exact:true}).click();await settle();
 let active=await state();assert.equal(active.experiments[0].active.variantId,variantId);assert.equal(active.autoresponsesEnabled,before.autoresponsesEnabled);
 assert.deepEqual(active.rules.find(r=>r.id==='unrelated'),before.rules.find(r=>r.id==='unrelated'));
 // Undo restores only this experiment's previous subset.
 await manager.getByRole('button',{name:'Undo activation',exact:true}).click();await settle();assert.deepEqual((await state()).rules,before.rules);
 await page.evaluate(()=>globalThis.__workspaceFixture.variantOverlaps=[]);
 await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).click();await settle();
 const authored=(await state()).experiments[0].variants;
 await manager.getByRole('button',{name:'Pause experiment',exact:true}).click();await settle();assert.deepEqual((await state()).experiments[0].variants,authored);
 assert.match(await manager.getByRole('status').first().textContent(),/Experiment paused/);
 await manager.getByRole('button',{name:'Resume experiment',exact:true}).click();await settle();
 // Old revisions are inspectable; selecting them is passive and preview uses exact selection.
 const live=(await state()).rules;
 await manager.getByLabel('Variant revision',{exact:true}).selectOption('2');
 assert.deepEqual((await state()).rules,live);assert.equal(await manager.locator('.variant-rule-list li').count(),1);
 await manager.getByRole('button',{name:'Preview saved revision…',exact:true}).click();
 const warning=page.locator('.captured-page-warning');await warning.getByRole('button',{name:'Open preview',exact:true}).waitFor();
 await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewRulesBusy);
 assert.deepEqual(await page.evaluate(()=>document.querySelector('app-shell').traffic.previewVariantSelection),{experimentId:experiment.id,variantId,revision:2});
 await warning.getByRole('button',{name:'Open preview',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
 assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.options.variant.revision),2);
 await page.evaluate(()=>document.querySelector('app-shell').traffic.showPreviewReport());
 assert.match(await page.locator('.captured-preview-report').textContent(),/revision 2/);
 await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.closePreviewReport();t.showRelatedPreview('modified');});
 await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewRulesBusy);
 assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.previewVariantSelection.revision),saved.revisions.at(-1).revision,'Updated preview lost the named variant or retained an obsolete revision');
 await warning.getByRole('button',{name:'Open preview',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
 await page.evaluate(()=>document.querySelector('app-shell').activateView('automation'));
 // A rejected transaction retains selection and input, with no live change.
 await manager.getByLabel('Variant revision',{exact:true}).selectOption(String(saved.revisions.at(-1).revision));
 await page.evaluate(()=>globalThis.__workspaceFixture.variantReject=true);
 await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).click();await settle();
 assert.match(await manager.locator('.field-error').textContent(),/validation failed/);assert.deepEqual((await state()).rules,live);
 await page.evaluate(()=>globalThis.__workspaceFixture.variantReject=false);
 // Baseline is an explicit activation and archive is reversible.
 await manager.getByLabel('Variant',{exact:true}).selectOption('baseline');
 await manager.getByRole('button',{name:'Preview saved revision…',exact:true}).click();
 await warning.getByRole('button',{name:'Open preview',exact:true}).waitFor();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewRulesBusy);
 await warning.getByRole('button',{name:'Open preview',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').traffic.previewPageBusy);
 assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.previewArgs.options.variant.variantId),'baseline');
 await page.evaluate(()=>document.querySelector('app-shell').activateView('automation'));
 await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).click();await settle();
 assert.deepEqual((await state()).rules,[before.rules.find(r=>r.id==='unrelated')]);
 await manager.getByLabel('Variant',{exact:true}).selectOption(variantId);
 await manager.getByRole('button',{name:'Archive variant',exact:true}).click();await settle();
 assert.equal(await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).isDisabled(),true);
 await manager.getByRole('button',{name:'Restore variant (Undo archive)',exact:true}).click();await settle();
 assert.equal(await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).isEnabled(),true);
 await manager.getByText('Activation order',{exact:true}).click();
 const beforePosition=(await state()).rules;
 await manager.getByRole('button',{name:'Save before other live rules',exact:true}).click();await settle();
 assert.deepEqual((await state()).rules,beforePosition,'Repairing priority activated the saved variant');
 const positioned=(await state()).experiments[0].variants.find(v=>v.id===variantId).revisions.at(-1);
 assert.ok(positioned.rules.every(rule=>rule.priority<Math.min(...beforePosition.map(r=>r.priority))));
 for(const [width,height] of [[1440,900],[960,720],[800,600]]){
  await page.setViewportSize({width,height});
  const button=await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).boundingBox();assert.ok(button.y>=0&&button.y+button.height<=height);
  await page.screenshot({path:resolve(root,`../../../target/ui-check/named-variants-${width}.png`)});
 }
 await page.evaluate(()=>document.documentElement.style.zoom='1.5');await page.emulateMedia({forcedColors:'active'});
 await manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true}).scrollIntoViewIfNeeded();
 await page.screenshot({path:resolve(root,'../../../target/ui-check/named-variants-zoom-contrast.png')});
 const activateButton=manager.getByRole('button',{name:'Use saved revision for live traffic',exact:true});await activateButton.focus();await page.keyboard.press('Tab');
 assert.equal(await manager.getByRole('button',{name:'Pause experiment',exact:true}).evaluate(b=>b===b.getRootNode().activeElement),true);
 await page.evaluate(()=>document.documentElement.style.zoom='');await page.emulateMedia({forcedColors:'none'});await page.setViewportSize({width:1440,height:900});
 // A delayed Add response must not attach an old variant to a different editor.
 await page.evaluate(()=>{const s=globalThis.__workspaceFixture,a=document.querySelector('app-shell').automation;s.deferSourceHeaders=true;s.variantAddPromise=a.editVariant(new CustomEvent('variant-edit',{detail:{selection:a.experimentVariants.selection(),ruleId:null}}));});
 await page.waitForFunction(()=>globalThis.__workspaceFixture.finishSourceHeaders);
 await page.evaluate(async()=>{const s=globalThis.__workspaceFixture,a=document.querySelector('app-shell').automation;a.draftDirty=false;await a.editAutoResponse(s.unrelated,true);s.finishSourceHeaders();await s.variantAddPromise;});
 assert.equal(await page.evaluate(()=>document.querySelector('app-shell').automation.editingVariant),null,'Late capture headers attached an obsolete variant to the current editor');
 assert.equal(await page.evaluate(()=>document.querySelector('app-shell').automation.responseExperiment),false);
}
