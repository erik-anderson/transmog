import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function openActionGroup(menu,label) {
  await menu.locator(`action-submenu[label="${label}"]`).getByRole('button',{name:label,exact:true}).click();
}

export async function checkActionMenus(page,root) {
  await page.goto('https://workspace.test/');
  const traffic=page.locator('traffic-workspace'),actions=traffic.locator('.selection-actions').getByRole('button',{name:'Actions',exact:true}),menu=traffic.locator('#selected-request-actions');
  await traffic.locator('.session-link').first().click();
  await page.waitForFunction(()=>!!document.querySelector('app-shell').traffic.selectedDetail);
  for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
    await page.setViewportSize({width,height});await actions.click();await openActionGroup(menu,'Copy');
    const panel=menu.locator('action-submenu[label="Copy"]').locator('#submenu');
    const box=await panel.boundingBox();assert.ok(box.x>=0&&box.y>=0&&box.x+box.width<=width&&box.y+box.height<=height,'Submenu escaped the viewport');
    await page.screenshot({path:resolve(root,'../../../target/ui-check/actions-copy-'+width+'.png')});
    await page.mouse.move(0,0);
    await page.waitForFunction(node=>getComputedStyle(node).backgroundColor==='rgba(0, 0, 0, 0)',await menu.getByRole('button',{name:'Copy request summary',exact:true}).elementHandle());
    assert.equal(await menu.getByRole('button',{name:'Copy request summary',exact:true}).evaluate(node=>getComputedStyle(node).backgroundColor),'rgba(0, 0, 0, 0)');
    await menu.getByRole('button',{name:'Copy request summary',exact:true}).click();
    const text=await page.evaluate(()=>navigator.clipboard.readText());assert.match(text,/GET http:\/\/example.test\//);assert.match(text,/Request #\d+ in this workspace/);assert.doesNotMatch(text,/Request ID:|Path:/);
    assert.equal(await actions.evaluate(node=>node===node.getRootNode().activeElement),true,'Copy left focus in a closed submenu');
  }
  await actions.click();const compare=menu.locator('action-submenu[label="Compare"]').getByRole('button',{name:'Compare',exact:true});
  await compare.focus();await compare.press('ArrowRight');
  const baseline=menu.getByRole('button',{name:'Set as comparison baseline',exact:true});await baseline.press('Escape');
  assert.equal(await compare.evaluate(node=>node===node.getRootNode().activeElement),true,'Escape did not restore the submenu trigger');
  await compare.press('Escape');assert.equal(await menu.isVisible(),false);assert.equal(await actions.evaluate(node=>node===node.getRootNode().activeElement),true);
  await actions.click();await openActionGroup(menu,'Compare');await baseline.click();
  await actions.click();await openActionGroup(menu,'Compare');assert.match(await menu.locator('action-submenu[label="Compare"]').textContent(),/Baseline: #/);await page.keyboard.press('Escape');await page.keyboard.press('Escape');
  await actions.click();await openActionGroup(menu,'Flag');await menu.getByRole('button',{name:'Flag response…',exact:true}).click();
  assert.equal(await traffic.getByRole('dialog',{name:'Add marker'}).count(),1);await page.keyboard.press('Escape');
  await page.setViewportSize({width:1280,height:800});
}
