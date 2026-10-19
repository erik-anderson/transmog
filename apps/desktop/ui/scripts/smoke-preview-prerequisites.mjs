import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {nativeResponseSetContext} from './native-response-set-helpers.mjs';

// Empty sets use the same explicit captured-page chooser as populated sets.
export async function smokePreviewPrerequisites(options) {
  const {main,screenshot}=options,h=nativeResponseSetContext(options),{shell,sets,invoke}=h,before=await invoke('automation_status'),lifecycle=(await invoke('app_status')).lifecycle;
  try {
    const status=await invoke('change_response_set',{generation:before.generation,change:{action:'create',id:'native-empty-prerequisite',name:'Native empty response set',notes:'Preview requires a captured page',rules:[]}});await h.openSet(status,'native-empty-prerequisite');
    const picker=shell+'.responseSetPagePicker',dialog=picker+'.pageDialog',change=h.button(sets,'Choose page…');
    await h.pointer(change);await h.wait(dialog+'.open');assert.equal(await main.evaluate(picker+'.selectedId'),'');assert.equal(await main.evaluate(`${dialog}.querySelectorAll('input:checked').length`),0);assert.equal(await main.evaluate(`${h.button(dialog,'Use page')}.disabled`),true);
    await h.key('Escape');await h.wait('!'+dialog+'.open');assert.equal(await main.evaluate(`${sets}.getRootNode().activeElement===${change}`),true);await main.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');await h.key('Enter');await h.wait(dialog+'.open');await h.pointer(h.button(dialog,'Close'));assert.equal(await main.evaluate(`${sets}.getRootNode().activeElement===${change}`),true);
    await h.pointer(change);await h.wait(dialog+'.open');
    for(const [width,height] of [[1440,900],[960,720],[800,600]]){await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});assert.equal(await main.evaluate(`(()=>{const r=${dialog}.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight&&r.left>=0&&r.right<=innerWidth;})()`),true);if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-prerequisite-${width}.png`),Buffer.from(image.data,'base64'));}}
    await main.call('Emulation.setEmulatedMedia',{features:[{name:'forced-colors',value:'active'}]});await main.evaluate("document.documentElement.style.zoom='1.5'");const image=await main.call('Page.captureScreenshot',{format:'png'});if(screenshot)await writeFile(screenshot.replace('.png','-prerequisite-zoom-contrast.png'),Buffer.from(image.data,'base64'));await main.evaluate("document.documentElement.style.zoom=''");await main.call('Emulation.setEmulatedMedia',{features:[]});await main.call('Emulation.clearDeviceMetricsOverride');
    assert.deepEqual(await invoke('automation_status'),status);assert.equal((await invoke('app_status')).lifecycle,lifecycle);await h.pointer(h.button(dialog,'Go to Traffic'));await h.wait(`${shell}.activeView==='traffic'`);assert.equal(await main.evaluate(dialog+'.open'),false);
    process.stdout.write('Native empty current set page prerequisite: explicit selection, pointer/Enter, Escape/Close focus, Traffic recovery, compact/zoom/forced colors and unchanged live state verified.\n');
  }finally{await main.evaluate(`if(${shell}.responseSetPagePicker.pageDialog.open)${shell}.responseSetPagePicker.cancel()`);await h.cleanup(before);}
}
