import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

/** Production controls, real WebView2 layout, and native preference persistence. */
export async function smokeResponseSetGuide({main,waitFor,screenshot,reload=false}) {
  const shell="document.querySelector('app-shell')",a=shell+'.automation',v=a+'.experimentVariants';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  // Dispatch real input: DOM .click() does not focus the button and concealed
  // the native disabled-button blur that used to cancel the user's own Start.
  const click=async expression=>{
    const point=await main.evaluate(`(()=>{const b=${expression};b.scrollIntoView({block:'nearest'});const r=b.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await main.call('Input.dispatchMouseEvent',{type:'mouseMoved',...point});
    await main.call('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',buttons:1,clickCount:1,...point});
    await main.call('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',buttons:0,clickCount:1,...point});
  };
  const key=async(key,code,windowsVirtualKeyCode)=>{
    await main.call('Input.dispatchKeyEvent',{type:'keyDown',key,code,windowsVirtualKeyCode,...(key==='Enter'?{text:'\r',unmodifiedText:'\r'}:{})});
    await main.call('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode});
  };
  const startControl=async keyboard=>{
    if(keyboard){await key('Tab','Tab',9);assert.equal(await main.evaluate(`${v}.getRootNode().activeElement===${v}.guideStartButton`),true,'Native Tab did not reach Start');await key('Enter','Enter',13);}
    else await click(`${v}.guideStartButton`);
  };
  const before=await invoke('automation_status');
  const selection=await main.evaluate(`(()=>{const v=${v};return {experimentId:v.experimentId,variantId:v.variantId,revision:v.variantRevision};})()`);
  await waitFor(()=>main.evaluate(`${v}.preferencesReady&&!${v}.guidePending`),'Native guide preferences did not load');
  if(!reload){
    assert.equal(await main.evaluate(`${v}.invitationVisible`),true,'First successful native save did not offer the tour');
    assert.equal(await main.evaluate(`${v}.tourStep`),0,'Invitation started the tour');
    await click(`${v}.querySelector('.response-set-invitation button')`);
    await waitFor(()=>main.evaluate(`${v}.tourStep===1&&!${v}.guidePending`),'Native invitation pointer Start failed');
    await key('Escape','Escape',27);
    assert.equal((await invoke('product_state')).workspace.responseSetGuide,'started');
  }
  const start=async(keyboard=false)=>{
    await click(`${v}.guideHelpButton`);
    await startControl(keyboard);
    await waitFor(()=>main.evaluate(`${v}.tourStep===1&&!${v}.guidePending`),'Native guide did not start at step 1');
  };
  for(const [width,height] of reload?[[800,600]]:[[1440,900],[960,720],[800,600]]){
    await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    await start(width===960);
    for(let step=1;step<=4;step++){
      assert.equal(await main.evaluate(`${v}.tourStep`),step);
      const reach=await main.evaluate(`(()=>{const v=${v},p=v.tourPanel,b=p.nextButton;b.scrollIntoView({block:'nearest'});const r=b.getBoundingClientRect();return {visible:r.top>=0&&r.bottom<=innerHeight,highlight:v.querySelectorAll('[data-tour-target]').length,dialog:p.querySelector('dialog').open};})()`);
      assert.equal(reach.visible,true,`Native guide ${step} controls clipped at ${width}`);assert.equal(reach.highlight,1);assert.equal(reach.dialog,true);
      if(screenshot&&!reload){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-tour-${width}-step-${step}.png`),Buffer.from(image.data,'base64'));}
      await click(`${v}.tourPanel.nextButton`);
    }
    assert.equal(await main.evaluate(`${v}.guideHelpButton.getRootNode().activeElement===${v}.guideHelpButton`),true,'Native Done did not return visible focus');
  }
  await start();
  await key('Escape','Escape',27);
  await waitFor(()=>main.evaluate(`${v}.tourStep===0`),'Native Escape did not end the guide');
  // Hold real persistence replies across rendered pending state, with natural
  // pointer/Enter focus retention and intentional Tab/Close focus changes.
  for(const mode of ['pointer','keyboard','tab','close']){
    await click(`${v}.guideHelpButton`);
    await main.evaluate(`(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks;globalThis.guideNativeSet=callbacks.set;globalThis.nativeGuideReplies=0;const delayed=function(id,callback){return globalThis.guideNativeSet.call(this,id,result=>{if(result?.history==='started'&&typeof result.offered==='boolean'){globalThis.nativeGuideReplies++;globalThis.finishNativeGuide=()=>callback(result);}else callback(result);});};callbacks.set=delayed;if(callbacks.set!==delayed)throw new Error('Native guide callback delay did not install');})()`);
    try {
      await startControl(mode==='keyboard');
      await waitFor(()=>main.evaluate('!!globalThis.finishNativeGuide'),'Native guide reply was not delayed');
      await main.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
      assert.equal(await main.evaluate(`${v}.getRootNode().activeElement===${v}.guideStartButton`),true,'Pending native Start lost focus');
      if(mode==='tab')await key('Tab','Tab',9);
      else if(mode==='close')await click(`${v}.guideHelpPanel.querySelector('button:last-child')`);
      else {await key('Enter','Enter',13);assert.equal(await main.evaluate('globalThis.nativeGuideReplies'),1,'Pending native Start duplicated preference writes');}
      await main.evaluate('globalThis.finishNativeGuide()');
      await waitFor(()=>main.evaluate(`!${v}.guidePending`),'Native delayed start remained pending');
      if(mode==='tab'){
        assert.equal(await main.evaluate(`${v}.helpOpen&&${v}.tourStep===0&&${v}.getRootNode().activeElement===${v}.guideHelpPanel.querySelector('button:last-child')`),true,'Late native start removed newly focused help');
        await click(`${v}.guideHelpPanel.querySelector('button:last-child')`);
      } else if(mode==='close')assert.equal(await main.evaluate(`!${v}.helpOpen&&${v}.tourStep===0&&${v}.getRootNode().activeElement===${v}.guideHelpButton`),true,'Late native Start reopened closed help');
      else {assert.equal(await main.evaluate(`${v}.tourStep`),1,'Delayed native Start failed');await key('Escape','Escape',27);}
    }finally{await main.evaluate('globalThis.finishNativeGuide?.();delete window.__TAURI_INTERNALS__.callbacks.set;delete globalThis.guideNativeSet;delete globalThis.finishNativeGuide;delete globalThis.nativeGuideReplies;');}
  }
  if(!reload){
    await main.call('Emulation.setEmulatedMedia',{features:[{name:'forced-colors',value:'active'}]});
    await main.evaluate("document.documentElement.style.zoom='1.5'");
    try {
      await start();
      for(let step=1;step<=4;step++){
        assert.equal(await main.evaluate(`(()=>{const p=${v}.tourPanel;return [...p.querySelectorAll('button:not(:disabled),h3,#response-set-step-copy')].every(node=>{node.scrollIntoView({block:'nearest'});const r=node.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;});})()`),true,'Native zoomed guide controls or copy clipped');
        await main.evaluate(`${v}.tourPanel.nextButton.scrollIntoView({block:'nearest'})`);
        if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png',`-tour-zoom-contrast-step-${step}.png`),Buffer.from(image.data,'base64'));}
        await click(`${v}.tourPanel.nextButton`);
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
    await click(`${v}.guideHelpButton`);
    assert.equal(await main.evaluate(`${v}.guideStartButton.textContent`),'Replay tour');
    await click(`${v}.guideHelpPanel.querySelector('button:last-child')`);
    assert.deepEqual(await invoke('automation_status'),before);
  }
  await main.call('Emulation.clearDeviceMetricsOverride');
  process.stdout.write('Native response set guide: pointer invitation, pointer/keyboard replay, delayed replies, duplicate Start, moved focus, Close, four steps, Escape, compact controls, navigation, passive selection and unchanged generation/Undo verified'+(reload?' after reload':'')+'.\n');
}
