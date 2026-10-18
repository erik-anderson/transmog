import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

// Runs only in the caller's isolated native profile with a saved Baseline and no traffic.
export async function smokePreviewPrerequisites({main,waitFor,screenshot,reproduce=false,recoveryPath}) {
  const shell="document.querySelector('app-shell')",v=shell+'.automation.experimentVariants';
  const pointer=async expression=>{
    const point=await main.evaluate(`(()=>{const b=${expression};b.scrollIntoView({block:'nearest'});const r=b.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await main.call('Input.dispatchMouseEvent',{type:'mouseMoved',...point});
    await main.call('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',buttons:1,clickCount:1,...point});
    await main.call('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',buttons:0,clickCount:1,...point});
  };
  const key=async name=>{await main.call('Input.dispatchKeyEvent',{type:'keyDown',key:name,code:name,windowsVirtualKeyCode:name==='Escape'?27:13,...(name==='Enter'?{text:'\r',unmodifiedText:'\r'}:{})});await main.call('Input.dispatchKeyEvent',{type:'keyUp',key:name,code:name,windowsVirtualKeyCode:name==='Escape'?27:13});};
  const preview=v+".querySelector('.variant-preview-actions button')";
  const before=await main.evaluate("window.__TAURI_INTERNALS__.invoke('automation_status')");
  const statusBefore=await main.evaluate("window.__TAURI_INTERNALS__.invoke('app_status')");
  await pointer(preview);
  if(reproduce){await waitFor(()=>main.evaluate(`${shell}.activeView==='traffic'`),'Old preview did not reproduce navigation');}
  else {
    await waitFor(()=>main.evaluate(`${shell}.previewPrerequisiteDialog.open`),'Missing-page dialog did not open');
    assert.equal(await main.evaluate(`${shell}.activeView`),'automation');
    assert.equal(await main.evaluate(`${v}.variantId`),'baseline');
    await key('Escape');await waitFor(()=>main.evaluate(`!${shell}.previewPrerequisiteDialog.open`),'Escape did not close prerequisites');
    assert.equal(await main.evaluate(`${preview}===${v}.getRootNode().activeElement`),true);
    await key('Enter');await waitFor(()=>main.evaluate(`${shell}.previewPrerequisiteDialog.open`),'Keyboard preview did not reopen prerequisites');
    await pointer(shell+'.previewPrerequisiteDialog.querySelector(\'button[data-close]\')');
    assert.equal(await main.evaluate(`${preview}===${v}.getRootNode().activeElement`),true);
    await pointer(preview);await waitFor(()=>main.evaluate(`${shell}.previewPrerequisiteDialog.open`),'Retry did not open prerequisites');
  }
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',reproduce?'-before.png':'-prerequisite.png'),Buffer.from(image.data,'base64'));}
  assert.deepEqual(await main.evaluate("window.__TAURI_INTERNALS__.invoke('automation_status')"),before,'Preview prerequisites mutated automation');
  assert.equal((await main.evaluate("window.__TAURI_INTERNALS__.invoke('app_status')")).lifecycle,statusBefore.lifecycle,'Preview prerequisites started capture');
  assert.equal(await main.evaluate(`${shell}.traffic.previewReportAvailable`),false,'Missing-page action opened a preview');
  if(!reproduce){
    for(const [width,height] of [[1440,900],[960,720],[800,600]]){
      await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
      assert.equal(await main.evaluate(`(()=>{const d=${shell}.previewPrerequisiteDialog,r=d.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight&&r.left>=0&&r.right<=innerWidth;})()`),true,'Native prerequisite dialog clipped');
      if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-${width}.png`),Buffer.from(image.data,'base64'));}
    }
    await main.call('Emulation.setEmulatedMedia',{features:[{name:'forced-colors',value:'active'}]});
    await main.evaluate("document.documentElement.style.zoom='1.5'");
    try {
      assert.equal(await main.evaluate(`(()=>{const d=${shell}.previewPrerequisiteDialog;return [...d.querySelectorAll('button,p:not([hidden])')].every(n=>{const r=n.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;});})()`),true,'Native zoomed prerequisite controls clipped');
      if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-zoom-contrast.png'),Buffer.from(image.data,'base64'));}
    }finally{await main.evaluate("document.documentElement.style.zoom=''");await main.call('Emulation.setEmulatedMedia',{features:[]});}
    await pointer(shell+'.previewPrerequisiteDialog.querySelector(\'button[data-traffic]\')');
    await waitFor(()=>main.evaluate(`${shell}.activeView==='traffic'`),'Explicit Traffic action did not navigate');
    if(recoveryPath){
      await main.evaluate(`${shell}.traffic.importPaths([${JSON.stringify(recoveryPath)}])`);
      await waitFor(()=>main.evaluate(`${shell}.traffic.sessions.length===1&&!${shell}.traffic.importingTrace`),'Native recovery capture did not import');
      await pointer(shell+".traffic.querySelector('tr[data-session-id]')");
      await waitFor(()=>main.evaluate(`${shell}.traffic.previewPageAvailable`),'Native recovery HTML did not select');
    }
    await main.evaluate(`${shell}.activateView('automation')`);
    assert.equal(await main.evaluate(`${v}.variantId`),'baseline');
    if(recoveryPath){
      await pointer(preview);await waitFor(()=>main.evaluate(`${shell}.traffic.previewPageDialog.open&&!${shell}.traffic.previewRulesBusy`),'Native captured recovery did not offer preview');
      assert.equal(await main.evaluate(`${shell}.traffic.previewPageScripts.checked`),false);
      assert.equal(await main.evaluate(`${shell}.traffic.previewVariantSelection.variantId`),'baseline');
      await pointer(shell+".traffic.previewPageDialog.querySelector('footer button')");
      await waitFor(()=>main.evaluate(`!${shell}.traffic.previewPageBusy&&${shell}.traffic.previewSceneAvailable`),'Native captured recovery preview did not open');
      const report=await main.evaluate(`window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${shell}.traffic.previewReportLabel})`);
      assert.equal(report.variant.variantId,'baseline');assert.equal(report.scriptsEnabled,false);
      assert.equal(report.url,'https://preview.invalid/captured');
      assert.deepEqual(await main.evaluate("window.__TAURI_INTERNALS__.invoke('automation_status')"),before);
    }
    await main.call('Emulation.clearDeviceMetricsOverride');
  }
  process.stdout.write(reproduce?'Native empty Baseline preview navigation reproduced.\n':'Native preview prerequisites: pointer/Enter, Close/Escape focus, explicit Traffic, Baseline selection, unchanged automation, compact/zoom/forced-colors verified.\n');
}
