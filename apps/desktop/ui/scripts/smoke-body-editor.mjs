import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

/** Real WebView2 top-layer positioning and keyboard/pointer behavior. */
export async function smokeBodyEditor(call,evaluate,screenshotPath) {
  const widget="document.querySelector('app-shell').shadowRoot.querySelector('response-body-editor')";
  const wait=async expression=>evaluate(`(async()=>{const deadline=Date.now()+15000;while(!(${expression})&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));if(!(${expression}))throw new Error(${JSON.stringify('Timed out: '+expression)});})()`);
  const escape=async()=>{await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});};
  const screenshot=async suffix=>{if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,`-svg-${suffix}.png`),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));};
  const clickColor=async()=>{
    await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
    await evaluate(`(async()=>{const w=${widget},deadline=Date.now()+15000;while(!w.editorHost.querySelector('.colorpicker-color-decoration')&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));if(!w.editorHost.querySelector('.colorpicker-color-decoration'))throw new Error('Native SVG color decoration unavailable: '+JSON.stringify({ready:w.bodyEditorReady,status:w.bodyEditorStatus,error:w.bodyError,language:w.model?.getLanguageId(),bodyLanguage:w.bodyLanguage,model:w.model?.getValue(),host:w.editorHost.getBoundingClientRect().toJSON(),dialog:w.surface.getBoundingClientRect().toJSON(),section:w.closest('automation-workspace')?.automationSection,workers:performance.getEntriesByType('resource').filter(item=>item.name.includes('worker')).map(item=>item.name)}));})()`);
    const point=await evaluate(`(()=>{const el=${widget}.editorHost.querySelector('.colorpicker-color-decoration'),r=el.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',...point});
    await call('Input.dispatchMouseEvent',{type:'mousePressed',...point,button:'left',buttons:1,clickCount:1});
    await call('Input.dispatchMouseEvent',{type:'mouseReleased',...point,button:'left',buttons:0,clickCount:1});
    // Async color providers can replace the hover between two CDP calls. Read
    // its visible geometry and hit targets atomically once it is rendered.
    const position=await evaluate(`(async()=>{const w=${widget},root=w.getRootNode(),deadline=Date.now()+15000;while(Date.now()<deadline){const el=w.overflowHost.querySelector('.colorpicker-widget'),r=el?.getBoundingClientRect();if(r?.width&&r.height&&getComputedStyle(el).visibility!=='hidden'&&Number(getComputedStyle(el.closest('.monaco-hover')).opacity)>=1){const targets=[r.top+5,r.bottom-5].map(y=>root.elementFromPoint(r.left+r.width/2,y));return {rect:r.toJSON(),viewport:{width:innerWidth,height:innerHeight},insideDialog:el.closest('dialog')===w.surface,hit:targets.every(target=>el.contains(target)),targets:targets.map(target=>({tag:target?.tagName,class:target?.className,rect:target?.getBoundingClientRect().toJSON()}))};}await new Promise(resolve=>setTimeout(resolve,25));}throw new Error('Native pointer click did not open a visible color picker');})()`);
    if(!position.insideDialog||!position.hit)await screenshot('picker-failure');
    assert.ok(position.insideDialog&&position.hit,`Native picker is clipped or outside the dialog top layer: ${JSON.stringify(position)}`);
    assert.ok(position.rect.top>=0&&position.rect.left>=0&&position.rect.bottom<=position.viewport.height&&position.rect.right<=position.viewport.width,`Native picker leaves viewport: ${JSON.stringify(position)}`);
    return position;
  };
  try {
    await evaluate(`(async()=>{
      const shell=document.querySelector('app-shell'),root=shell.shadowRoot;
      globalThis.__bodyEditorProbe={view:shell.activeView,focus:root.activeElement};
      root.querySelector('a[data-view="automation"]').click();
      const deadline=Date.now()+15000;while(!root.querySelector('response-body-editor')?.open&&Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,25));
      const w=root.querySelector('automation-workspace'),editor=w.bodyEditor;
      if(editor.surface.open)throw new Error('Body editor probe requires a closed dialog');
      globalThis.__bodyEditorProbe.theme=editor.theme;
      globalThis.__bodyEditorProbe.section=w.automationSection;
      globalThis.__bodyEditorProbe.editorHidden=w.editorHidden;
      // The compact layout hides the properties pane when no rule is open.
      // Expose it for this direct dialog probe without resetting any draft.
      w.showAutomationSection('responses');w.editorHidden=false;w.$flushUpdates();
      editor.open('<svg><style>.shape { fill: #336699; }</style></svg>','svg',false,root.querySelector('a[data-view="automation"]'),{saveUnavailableReason:'Apply to draft, then save the new rule.'});
    })()`);
    await wait(`${widget}.bodyEditorReady`);
    const positions=[];
    for(const [width,height,label] of [[1440,900,'wide'],[960,720,'medium'],[800,600,'compact']]) {
      await call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
      await evaluate(`${widget}.editor.layout()`);
      positions.push(await clickColor());await screenshot(label);
      await escape();await wait(`!${widget}.overflowHost.querySelector('.colorpicker-widget')?.getClientRects().length`);
      assert.equal(await evaluate(`${widget}.surface.open`),true,'Native Escape dismissed the dialog instead of its color picker');
    }
    await evaluate(`(()=>{const w=${widget};w.editor.setValue('<svg>\\n'+Array(40).fill('<!-- shape -->').join('\\n')+'\\n<style>.shape { fill: #336699; }</style>\\n</svg>');w.editor.setPosition({lineNumber:42,column:25});w.editor.revealLine(42);})()`);
    positions.push(await clickColor());await screenshot('last-line');await escape();
    await wait(`!${widget}.overflowHost.querySelector('.colorpicker-widget')?.getClientRects().length`);
    await evaluate(`${widget}.theme='dark'`);await wait(`${widget}.editorHost.querySelector('.vs-dark')`);positions.push(await clickColor());await screenshot('dark');await escape();
    await call('Emulation.setEmulatedMedia',{media:'screen',features:[{name:'forced-colors',value:'active'}]});
    await wait(`${widget}.editorHost.querySelector('.hc-black')`);
    positions.push(await clickColor());await screenshot('forced-colors');await escape();
    await call('Emulation.setEmulatedMedia',{media:'screen',features:[]});
    // Editor zoom exercises enlarged editor text independently of OS DPI.
    await evaluate(`(()=>{const w=${widget};w.editor.trigger('probe','editor.action.fontZoomIn',{});w.editor.trigger('probe','editor.action.fontZoomIn',{});w.editor.revealLine(42);})()`);
    positions.push(await clickColor());await screenshot('editor-zoom');await escape();
    await evaluate(`${widget}.editor.trigger('probe','editor.action.fontZoomReset',{})`);
    await call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:2,mobile:false});
    positions.push(await clickColor());await screenshot('200dpi');
    // A high-DPI CDP screenshot can relayout WebView2 and dismiss Monaco's
    // hover. Reopen it with the pointer before testing Escape from the picker.
    await clickColor();
    await escape();
    assert.equal(await evaluate(`${widget}.surface.open`),true,'Native Escape at 200% DPI dismissed the dialog instead of its color picker');
    await call('Emulation.setDeviceMetricsOverride',{width:400,height:300,deviceScaleFactor:2,mobile:false});
    for(const canSave of [false,true]) {
      await evaluate(`(()=>{const w=${widget};w.bodyCanSave=${canSave};w.$flushUpdates();})()`);
      // Let the button color transition finish before checking hierarchy and
      // capturing the saved-rule footer: Save is primary, Apply is secondary.
      if(canSave)await wait(`getComputedStyle(${widget}.surface.querySelector('.body-apply')).backgroundColor===getComputedStyle(${widget}.surface.querySelector('footer button.secondary')).backgroundColor`);
      await screenshot(canSave?'200percent-saved':'200percent');
      assert.ok(await evaluate(`${widget}.editorHost.getBoundingClientRect().height>=60`),'Native body editor has no usable editing area at 200% zoom');
      assert.equal(await evaluate(`Array.from(${widget}.surface.querySelectorAll('footer button')).every(button=>{const r=button.getBoundingClientRect();return r.top>=0&&r.left>=0&&r.bottom<=innerHeight&&r.right<=innerWidth;})`),true,'Native dialog actions are unreachable at 200% zoom in an 800×600 window');
    }
    await evaluate(`${widget}.editor.focus()`);await escape();await wait(`!${widget}.surface.open&&${widget}.model===null&&${widget}.editor===null`);
    assert.equal(await evaluate(`document.querySelector('app-shell').shadowRoot.activeElement===document.querySelector('app-shell').shadowRoot.querySelector('a[data-view="automation"]')`),true,'Native editor did not restore focus');
    return {topLayer:true,pointer:true,firstLastLines:true,escapeKeepsDialog:true,disposedOnClose:true,responsive:[1440,960,800],editorZoom:true,dpi:200,dark:true,forcedColors:true,positions};
  } finally {
    await call('Emulation.clearDeviceMetricsOverride');
    await call('Emulation.setEmulatedMedia',{media:'screen',features:[]});
    await evaluate(`(()=>{const probe=globalThis.__bodyEditorProbe;if(!probe)return;const shell=document.querySelector('app-shell'),root=shell.shadowRoot,editor=root.querySelector('response-body-editor');if(editor){editor.editor?.trigger('probe','editor.action.fontZoomReset',{});editor.theme=probe.theme??'system';editor.close();}const workspace=root.querySelector('automation-workspace');if(probe.editorHidden!==undefined)workspace.editorHidden=probe.editorHidden;if(probe.section)workspace.showAutomationSection(probe.section);workspace.$flushUpdates();root.querySelector('a[data-view="'+probe.view+'"]').click();if(probe.focus?.isConnected&&probe.focus.getClientRects().length)probe.focus.focus();delete globalThis.__bodyEditorProbe;})()`);
  }
}
