import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

/** Production controls, real WebView2 layout, and native preference persistence. */
export async function smokeResponseSetGuide({main,waitFor,screenshot,reload=false}) {
  const shell="document.querySelector('app-shell')",a=shell+'.automation',v=a+'.experimentVariants';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const before=await invoke('automation_status');
  const selection=await main.evaluate(`(()=>{const v=${v};return {experimentId:v.experimentId,variantId:v.variantId,revision:v.variantRevision};})()`);
  await waitFor(()=>main.evaluate(`${v}.preferencesReady&&!${v}.guidePending`),'Native guide preferences did not load');
  if(!reload){
    assert.equal(await main.evaluate(`${v}.invitationVisible`),true,'First successful native save did not offer the tour');
    assert.equal(await main.evaluate(`${v}.tourStep`),0,'Invitation started the tour');
    await main.evaluate(`${v}.dismissGuideInvitation()`);
    assert.equal((await invoke('product_state')).workspace.responseSetGuide,'offered');
  }
  const start=async()=>{
    await main.evaluate(`${v}.guideHelpButton.click()`);
    await main.evaluate(`${v}.guideStartButton.click()`);
    await waitFor(()=>main.evaluate(`${v}.tourStep===1&&!${v}.guidePending`),'Native guide did not start at step 1');
  };
  for(const [width,height] of reload?[[800,600]]:[[1440,900],[960,720],[800,600]]){
    await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    await start();
    for(let step=1;step<=4;step++){
      assert.equal(await main.evaluate(`${v}.tourStep`),step);
      const reach=await main.evaluate(`(()=>{const v=${v},p=v.tourPanel,b=p.nextButton;b.scrollIntoView({block:'nearest'});const r=b.getBoundingClientRect();return {visible:r.top>=0&&r.bottom<=innerHeight,highlight:v.querySelectorAll('[data-tour-target]').length,dialog:p.querySelector('dialog').open};})()`);
      assert.equal(reach.visible,true,`Native guide ${step} controls clipped at ${width}`);assert.equal(reach.highlight,1);assert.equal(reach.dialog,true);
      if(screenshot&&!reload){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-tour-${width}-step-${step}.png`),Buffer.from(image.data,'base64'));}
      await main.evaluate(`${v}.tourPanel.nextButton.click()`);
    }
    assert.equal(await main.evaluate(`${v}.guideHelpButton.getRootNode().activeElement===${v}.guideHelpButton`),true,'Native Done did not return visible focus');
  }
  await start();
  await main.call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
  await main.call('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
  await waitFor(()=>main.evaluate(`${v}.tourStep===0`),'Native Escape did not end the guide');
  // Delay the real native reply, then move focus within help before it completes.
  await main.evaluate(`(()=>{const v=${v},callbacks=window.__TAURI_INTERNALS__.callbacks;v.showGuideHelp();globalThis.guideNativeSet=callbacks.set;const delayed=function(id,callback){return globalThis.guideNativeSet.call(this,id,result=>{if(result?.history==='started'&&typeof result.offered==='boolean')globalThis.finishNativeGuide=()=>callback(result);else callback(result);});};callbacks.set=delayed;if(callbacks.set!==delayed)throw new Error('Native guide callback delay did not install');v.guideStartButton.click();})()`);
  try {
    await waitFor(()=>main.evaluate('!!globalThis.finishNativeGuide'),'Native guide reply was not delayed');
    await main.evaluate(`(()=>{const v=${v};v.guideHelpPanel.querySelector('button:last-child').focus();globalThis.finishNativeGuide();})()`);
    await waitFor(()=>main.evaluate(`!${v}.guidePending`),'Native delayed start remained pending');
    assert.equal(await main.evaluate(`${v}.helpOpen&&${v}.tourStep===0&&${v}.guideHelpPanel.contains(${v}.getRootNode().activeElement)`),true,'Late native start removed newly focused help');
    await main.evaluate(`${v}.closeGuideHelp()`);
  }finally{await main.evaluate('globalThis.finishNativeGuide?.();delete window.__TAURI_INTERNALS__.callbacks.set;delete globalThis.guideNativeSet;delete globalThis.finishNativeGuide;');}
  if(!reload){
    await main.call('Emulation.setEmulatedMedia',{features:[{name:'forced-colors',value:'active'}]});
    await main.evaluate("document.documentElement.style.zoom='1.5'");
    try {
      await start();
      for(let step=1;step<=4;step++){
        assert.equal(await main.evaluate(`(()=>{const p=${v}.tourPanel;return [...p.querySelectorAll('button:not(:disabled),h3,#response-set-step-copy')].every(node=>{node.scrollIntoView({block:'nearest'});const r=node.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;});})()`),true,'Native zoomed guide controls or copy clipped');
        await main.evaluate(`${v}.tourPanel.nextButton.scrollIntoView({block:'nearest'})`);
        if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-tour-zoom-contrast-step-${step}.png`),Buffer.from(image.data,'base64'));}
        await main.evaluate(`${v}.tourPanel.nextButton.click()`);
      }
    }finally{await main.evaluate("document.documentElement.style.zoom=''");await main.call('Emulation.setEmulatedMedia',{features:[]});}
  }
  await start();
  await main.evaluate(`${a}.showAutomationSection('responses')`);
  assert.equal(await main.evaluate(`${v}.tourStep===0&&!${v}.helpOpen`),true,'Hidden native overview kept guide open');
  await main.evaluate(`${a}.showAutomationSection('experiments')`);
  assert.deepEqual(await invoke('automation_status'),before,'Native guide changed automation generation, Undo or live state');
  assert.deepEqual(await main.evaluate(`(()=>{const v=${v};return {experimentId:v.experimentId,variantId:v.variantId,revision:v.variantRevision};})()`),selection);
  assert.equal((await invoke('product_state')).workspace.responseSetGuide,'started');
  if(reload){
    await main.call('Page.reload');
    await waitFor(()=>main.evaluate(`${shell}?.workspaceLoaded`),'Native guide state did not hydrate after reload');
    await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${a}.showAutomationSection('experiments');await ${v}.reveal(${JSON.stringify(selection)},false);})()`);
    await waitFor(()=>main.evaluate(`${v}.preferencesReady&&!${v}.guidePending`),'Reloaded native overview did not become ready');
    assert.equal(await main.evaluate(`${v}.invitationVisible`),false,'Native guide invitation repeated after reload');
    await main.evaluate(`${v}.guideHelpButton.click()`);
    assert.equal(await main.evaluate(`${v}.guideStartButton.textContent`),'Replay tour');
    await main.evaluate(`${v}.closeGuideHelp()`);
    assert.deepEqual(await invoke('automation_status'),before);
  }
  await main.call('Emulation.clearDeviceMetricsOverride');
  process.stdout.write('Native response set guide: invitation, dismissal, replay, four steps, Escape, compact controls, navigation, passive selection and unchanged generation/Undo verified'+(reload?' after reload':'')+'.\n');
}
