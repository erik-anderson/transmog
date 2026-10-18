import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function newTrafficRule(page,label) {
  const workspace=page.locator('automation-workspace');
  await workspace.getByRole('button',{name:'New rule…',exact:true}).click();
  await workspace.locator('#rule-kind-menu').getByRole('button',{name:label,exact:true}).click();
}

export async function checkUnifiedRules(page,root) {
  page.setDefaultTimeout(15000);await page.goto('https://workspace.test/');
  await page.getByRole('link',{name:/Automation/}).click();
  await page.waitForFunction(()=>!!document.querySelector('app-shell').automation?.autoresponseState);
  await page.evaluate(()=>{globalThis.__workspaceFixture.automation=structuredClone(document.querySelector('app-shell').automation.autoresponseState);});
  await page.setViewportSize({width:1440,height:900});
  const workspace=page.locator('automation-workspace'),headers=workspace.locator('header-rules[editor-kind="headers"]'),text=workspace.locator('header-rules[editor-kind="response-text"]'),browser=workspace.locator('user-agent-overrides');
  assert.deepEqual(await workspace.locator('.workspace-tabs button').allTextContents(),['Rules','Experiments','Scripts']);
  for(const [label,name] of [['Change request headers…','Outbound headers'],['Change response headers…','Inbound headers']]) {
    await newTrafficRule(page,label);await headers.getByLabel('Rule name',{exact:true}).fill(name);
    await headers.getByLabel('Operation header name',{exact:true}).fill('X-Example');
    await headers.getByRole('button',{name:'Save rule',exact:true}).click();
    await page.waitForFunction(()=>!document.querySelector('app-shell').automation.headerRulesEditor.hrBusy);
  }
  await newTrafficRule(page,'Find and replace response text…');await text.getByLabel('Rule name',{exact:true}).fill('Replace response wording');
  await text.getByRole('button',{name:'Save rule',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').automation.responseTextEditor.hrBusy);
  await newTrafficRule(page,'Change browser headers…');await browser.locator('[name="uaName"]').fill('Browser identity');
  await browser.getByRole('button',{name:'Save rule',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').automation.userAgentOverrides.uaBusy);
  assert.equal(await browser.locator('rule-activation button').getAttribute('aria-pressed'),'false','A new browser header rule became enabled while saving');
  await newTrafficRule(page,'Create saved response from scratch…');const saved=workspace.locator('.auto-response-editor');
  await saved.locator('[name="name"]').fill('Offline response');await saved.getByLabel('Matching address',{exact:true}).fill('https://example.test/offline');await saved.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.waitForFunction(()=>{const w=document.querySelector('app-shell').automation;return !w.savingAutoResponse&&w.existingResponse&&!w.draftDirty;});
  process.stdout.write('Unified rule fixtures saved.\n');
  const grid=workspace.getByRole('grid',{name:'Traffic rules',exact:true});
  assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),5);
  const savedRule=await page.evaluate(()=>structuredClone(globalThis.__workspaceFixture.automation.rules.find(rule=>rule.request.responseAsset)));
  await grid.getByRole('button',{name:/^Outbound headers/}).click();await grid.getByRole('checkbox',{name:'Select Replace response wording',exact:true}).check();
  const bulk=async label=>{await workspace.locator('.rule-selection-toolbar').getByRole('button',{name:'More',exact:true}).click();await workspace.locator('#rule-actions-menu').getByRole('button',{name:new RegExp('^'+label)}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').automation.changingAutoResponse);};
  await bulk('Enable selected');assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.filter(rule=>rule.enabled).length),3);
  await bulk('Remove selected');assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),3);
  await workspace.locator('.auto-response-undo').getByRole('button',{name:'Undo',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').automation.changingAutoResponse);
  assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),5);
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.find(rule=>rule.request.responseAsset)),savedRule,'A mixed rule action or Undo rewrote the saved response');
  await workspace.getByLabel('Message direction',{exact:true}).selectOption('requests');assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),2);
  await workspace.getByLabel('Message direction',{exact:true}).selectOption('responses');assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),3);
  await workspace.getByLabel('Message direction',{exact:true}).selectOption('all');await workspace.getByLabel('Search rules',{exact:true}).fill('wording');assert.equal(await grid.locator('tbody tr[data-rule-id]').count(),1);await workspace.getByLabel('Search rules',{exact:true}).fill('');
  await workspace.locator('autoresponse-switch button').click();await page.waitForFunction(()=>document.querySelector('app-shell').autoresponseState?.autoresponsesEnabled===false);
  assert.match(await grid.locator('.auto-response-rule .rule-state').textContent(),/Enabled · Paused/);
  await grid.getByRole('button',{name:/^Outbound headers/}).click();await headers.getByLabel('Rule name',{exact:true}).fill('Keep this draft');
  await grid.getByRole('button',{name:/^Replace response wording/}).click();await grid.getByRole('button',{name:/^Outbound headers/}).click();
  assert.equal(await headers.getByLabel('Rule name',{exact:true}).inputValue(),'Keep this draft','Switching rule types lost a draft');
  // Leave the draft mounted while inspecting another rule type and changing layouts.
  await grid.getByRole('button',{name:/^Replace response wording/}).click();
  for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
    await page.setViewportSize({width,height});
    assert.equal(await text.getByLabel('Rule name',{exact:true}).isVisible(),true);
    assert.equal(await workspace.evaluate(node=>node.getBoundingClientRect().width<=innerWidth),true);
    await page.screenshot({path:resolve(root,'../../../target/ui-check/unified-rules-'+width+'.png')});
    if(width===800){await workspace.getByRole('button',{name:'← Back to rules',exact:true}).click();assert.equal(await grid.isVisible(),true);await grid.getByRole('button',{name:/^Replace response wording/}).click();}
  }
  await page.emulateMedia({forcedColors:'active'});await page.screenshot({path:resolve(root,'../../../target/ui-check/unified-rules-contrast.png')});await page.emulateMedia({forcedColors:'none'});
  await page.setViewportSize({width:1280,height:800});page.setDefaultTimeout(30000);
}
