import assert from "node:assert/strict";

export async function smokeHexViewer(evaluate, call) {
  try {
    const points = await evaluate(`(async () => {
      const inspector = document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="response"]');
      const traffic=document.querySelector('app-shell').traffic;
      globalThis.__hexSmoke = { traffic,selectedId:traffic.selectedSessionId,side:traffic.inspectorSide,inspector, mode:inspector.mode, preview:inspector.hexPreview, text:inspector.bodyText,notice:inspector.bodyNotice,
        clipboard:Object.getOwnPropertyDescriptor(navigator,'clipboard') };
      Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>globalThis.__hexSmoke.copied=text}});
      traffic.selectedSessionId='hex-smoke';traffic.inspectorSide='response';traffic.$flushUpdates();
      inspector.mode='body'; inspector.bodyText='';inspector.bodyNotice='';
      inspector.hexPreview={key:'webview-hex-check',bytes:new Uint8Array([1,71,1,87,1,91,1,101]),offset:16,truncated:true};
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
    await call("Input.dispatchMouseEvent", { type: "mouseMoved", ...points[0] });
    await call("Input.dispatchMouseEvent", {
      type: "mousePressed",
      button: "left",
      clickCount: 1,
      ...points[0],
    });
    await call("Input.dispatchMouseEvent", { type: "mouseMoved", buttons: 1, ...points[1] });
    await evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    await call("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      button: "left",
      clickCount: 1,
      ...points[1],
    });
    const selection = await evaluate(`(() => {
      const viewer=globalThis.__hexSmoke.viewer;
      const selected=column=>[...viewer.querySelectorAll('[data-selected][data-column="'+column+'"]')].map(cell=>Number(cell.dataset.byte));
      return {hex:selected('hex'),text:selected('text'),caret:Number(viewer.caret),
        caretAnimation:getComputedStyle(viewer.querySelector('[data-caret]'),'::before').animationName,
        allLabel:viewer.selectAllLabel,reducedMotion:matchMedia('(prefers-reduced-motion:reduce)').matches,
        focused:viewer.viewport.matches(':focus'),events:globalThis.__hexSmoke.events};
    })()`);
    assert.deepEqual(
      selection.hex,
      [0, 1, 2, 3, 4, 5, 6, 7],
      JSON.stringify({ points, selection }),
    );
    assert.deepEqual(selection.text, selection.hex);
    assert.equal(selection.caret, 7);
    assert.equal(
      selection.caretAnimation,
      selection.reducedMotion ? "none" : "hex-caret-blink",
      JSON.stringify(selection),
    );
    assert.equal(selection.allLabel, "Select all (truncated)");
    const key = async (key, code, modifiers = 0) => {
      await call("Input.dispatchKeyEvent", {
        type: "keyDown",
        key,
        windowsVirtualKeyCode: code,
        modifiers,
      });
      await call("Input.dispatchKeyEvent", {
        type: "keyUp",
        key,
        windowsVirtualKeyCode: code,
        modifiers,
      });
      await evaluate("new Promise(resolve=>requestAnimationFrame(resolve))");
    };
    await key("c", 67, 2);
    assert.equal(await evaluate("globalThis.__hexSmoke.copied"), "AUcBVwFbAWU=");
    await key("ArrowLeft", 37, 2);
    assert.deepEqual(
      await evaluate(
        "(()=>{const v=globalThis.__hexSmoke.viewer;return [v.caret,v.rangeStart,v.rangeEnd].map(Number)})()",
      ),
      [6, -1, -1],
    );
    await key("ArrowRight", 39, 8);
    assert.deepEqual(
      await evaluate(
        "(()=>{const v=globalThis.__hexSmoke.viewer;return [v.caret,v.rangeStart,v.rangeEnd].map(Number)})()",
      ),
      [7, 6, 7],
    );
    await key("F10", 121, 8);
    await key("ArrowDown", 40);
    await key("ArrowRight", 39);
    assert.equal(await evaluate("globalThis.__hexSmoke.viewer.copyMenuOpen"), true);
    const active = await evaluate(
      "globalThis.__hexSmoke.viewer.getRootNode().activeElement.textContent",
    );
    assert.equal(active.trim(), "Hex");
    await key("Escape", 27);
    await key("Escape", 27);
    assert.equal(
      await evaluate(`(async () => {
      const viewer=globalThis.__hexSmoke.viewer,deadline=performance.now()+1000;
      while(viewer.viewport!==viewer.getRootNode().activeElement && performance.now()<deadline)
        await new Promise(resolve=>requestAnimationFrame(resolve));
      return viewer.viewport===viewer.getRootNode().activeElement;
    })()`),
      true,
    );
    const dragPoints = await evaluate(`(async () => {
      const viewer=globalThis.__hexSmoke.viewer;
      viewer.preview={key:'webview-hex-drag',bytes:Uint8Array.from({length:262144},(_,i)=>i%256),offset:0,truncated:false};
      viewer.$flushUpdates();
      await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
      const first=viewer.querySelector('[data-byte="0"][data-column="hex"]').getBoundingClientRect(),bounds=viewer.viewport.getBoundingClientRect();
      return {start:{x:first.left+first.width/2,y:first.top+first.height/2},bottom:{x:first.left+first.width/2,y:bounds.bottom-8}};
    })()`);
    await call("Input.dispatchMouseEvent", { type: "mouseMoved", ...dragPoints.start });
    await call("Input.dispatchMouseEvent", {
      type: "mousePressed",
      button: "left",
      clickCount: 1,
      ...dragPoints.start,
    });
    try {
      await call("Input.dispatchMouseEvent", {
        type: "mouseMoved",
        buttons: 1,
        ...dragPoints.bottom,
      });
      for (let step = 0; step < 2; step++) {
        assert.equal(
          await evaluate(`(async () => {
            const viewer=globalThis.__hexSmoke.viewer,start=viewer.viewport.scrollTop,end=BigInt(viewer.rangeEnd),deadline=performance.now()+5000;
            while(viewer.viewport.scrollTop-start<250&&performance.now()<deadline)await new Promise(resolve=>setTimeout(resolve,30));
            return viewer.viewport.scrollTop-start>=250&&BigInt(viewer.rangeEnd)>end+128n&&viewer.rangeStart==='0';
          })()`),
          true,
          "Holding a drag at the bottom stalled native scrolling or selection",
        );
      }
    } finally {
      await call("Input.dispatchMouseEvent", {
        type: "mouseReleased",
        button: "left",
        clickCount: 1,
        ...dragPoints.bottom,
      });
    }
    assert.equal(
      await evaluate(`(async () => {
        const viewer=globalThis.__hexSmoke.viewer,top=viewer.viewport.scrollTop,end=viewer.rangeEnd;
        await new Promise(resolve=>setTimeout(resolve,120));
        return viewer.viewport.scrollTop===top&&viewer.rangeEnd===end;
      })()`),
      true,
      "Native byte selection kept scrolling after pointer release",
    );
    return {
      mouseSelection: true,
      mirroredHighlight: true,
      keyboardSelection: true,
      dragScrolling: true,
      copy: true,
      submenu: true,
    };
  } finally {
    await evaluate(`(() => {
      const saved=globalThis.__hexSmoke;
      if(!saved)return;
      saved.viewer?.cancelSelection();
      if(saved.viewer&&saved.listener)for(const type of ['pointerdown','pointermove','pointerup','lostpointercapture'])saved.viewer.viewport.removeEventListener(type,saved.listener);
      saved.viewer?.contextMenu.hidePopover();
      saved.inspector.hexPreview=saved.preview;saved.inspector.bodyText=saved.text;saved.inspector.bodyNotice=saved.notice;saved.inspector.mode=saved.mode;
      saved.inspector.$flushUpdates();
      saved.traffic.selectedSessionId=saved.selectedId;saved.traffic.inspectorSide=saved.side;saved.traffic.$flushUpdates();
      if(saved.clipboard)Object.defineProperty(navigator,'clipboard',saved.clipboard);else delete navigator.clipboard;
      delete globalThis.__hexSmoke;
    })()`);
  }
}
