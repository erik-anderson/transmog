import assert from 'node:assert/strict';

export async function smokeHexViewer(evaluate, call) {
  try {
    const points = await evaluate(`(async () => {
      const inspector = document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="response"]');
      globalThis.__hexSmoke = { inspector, mode:inspector.mode, preview:inspector.hexPreview, text:inspector.bodyText,
        clipboard:Object.getOwnPropertyDescriptor(navigator,'clipboard') };
      Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>globalThis.__hexSmoke.copied=text}});
      inspector.mode='body'; inspector.bodyText='';
      inspector.hexPreview={key:'webview-hex-check',bytesBase64:'AUcBVwFbAWU=',offset:16,truncated:true};
      inspector.$flushUpdates();
      await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
      const viewer=inspector.querySelector('hex-viewer');
      globalThis.__hexSmoke.viewer=viewer;
      const deadline=performance.now()+5000;
      while(getComputedStyle(viewer.columnHeader).display!=='grid'||viewer.columnHeader.getBoundingClientRect().height===0){
        if(performance.now()>deadline)throw new Error('Hex viewer styles did not become ready');
        await new Promise(resolve=>setTimeout(resolve,25));
      }
      viewer.renderRows(true);viewer.$flushUpdates();
      globalThis.__hexSmoke.events=[];
      globalThis.__hexSmoke.listener=event=>globalThis.__hexSmoke.events.push({type:event.type,id:event.pointerId,x:event.clientX,y:event.clientY,buttons:event.buttons});
      for(const type of ['pointerdown','pointermove','pointerup','lostpointercapture'])viewer.viewport.addEventListener(type,globalThis.__hexSmoke.listener);
      const point=index=>{const bounds=viewer.querySelector('[data-byte="'+index+'"][data-column="hex"]').getBoundingClientRect();return {x:bounds.left+bounds.width/2,y:bounds.top+bounds.height/2};};
      return [point(0),point(7)];
    })()`);
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',...points[0]});
    await call('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...points[0]});
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',buttons:1,...points[1]});
    await evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
    await call('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...points[1]});
    const selection = await evaluate(`(() => {
      const viewer=globalThis.__hexSmoke.viewer;
      const selected=column=>[...viewer.querySelectorAll('[data-selected][data-column="'+column+'"]')].map(cell=>Number(cell.dataset.byte));
      return {hex:selected('hex'),text:selected('text'),caret:viewer.caret,
        caretAnimation:getComputedStyle(viewer.querySelector('[data-caret]'),'::before').animationName,
        allLabel:viewer.selectAllLabel,reducedMotion:matchMedia('(prefers-reduced-motion:reduce)').matches,
        focused:viewer.viewport.matches(':focus'),events:globalThis.__hexSmoke.events};
    })()`);
    assert.deepEqual(selection.hex,[0,1,2,3,4,5,6,7],JSON.stringify({points,selection}));
    assert.deepEqual(selection.text,selection.hex);
    assert.equal(selection.caret,7);
    assert.equal(selection.caretAnimation,selection.reducedMotion?'none':'hex-caret-blink',JSON.stringify(selection));
    assert.equal(selection.allLabel,'Select all (truncated)');
    const key = async (key, code, modifiers=0) => {
      await call('Input.dispatchKeyEvent',{type:'keyDown',key,windowsVirtualKeyCode:code,modifiers});
      await call('Input.dispatchKeyEvent',{type:'keyUp',key,windowsVirtualKeyCode:code,modifiers});
    };
    await key('c',67,2);
    assert.equal(await evaluate('globalThis.__hexSmoke.copied'),'AUcBVwFbAWU=');
    await key('ArrowLeft',37,2);
    assert.deepEqual(await evaluate('(()=>{const v=globalThis.__hexSmoke.viewer;return [v.caret,v.rangeStart,v.rangeEnd]})()'),[6,-1,-1]);
    await key('ArrowRight',39,8);
    assert.deepEqual(await evaluate('(()=>{const v=globalThis.__hexSmoke.viewer;return [v.caret,v.rangeStart,v.rangeEnd]})()'),[7,6,7]);
    await key('F10',121,8);
    await key('ArrowDown',40);
    await key('ArrowRight',39);
    assert.equal(await evaluate('globalThis.__hexSmoke.viewer.copyMenuOpen'),true);
    const active = await evaluate('globalThis.__hexSmoke.viewer.getRootNode().activeElement.textContent');
    assert.equal(active.trim(),'Hex');
    await key('Escape',27); await key('Escape',27);
    assert.equal(await evaluate('globalThis.__hexSmoke.viewer.viewport===globalThis.__hexSmoke.viewer.getRootNode().activeElement'),true);
    return {mouseSelection:true,mirroredHighlight:true,keyboardSelection:true,copy:true,submenu:true};
  } finally {
    await evaluate(`(() => {
      const saved=globalThis.__hexSmoke;
      if(!saved)return;
      if(saved.viewer&&saved.listener)for(const type of ['pointerdown','pointermove','pointerup','lostpointercapture'])saved.viewer.viewport.removeEventListener(type,saved.listener);
      saved.viewer?.contextMenu.hidePopover();
      saved.inspector.hexPreview=saved.preview;saved.inspector.bodyText=saved.text;saved.inspector.mode=saved.mode;
      saved.inspector.$flushUpdates();
      if(saved.clipboard)Object.defineProperty(navigator,'clipboard',saved.clipboard);else delete navigator.clipboard;
      delete globalThis.__hexSmoke;
    })()`);
  }
}
