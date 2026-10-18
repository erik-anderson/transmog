import assert from 'node:assert/strict';

// A displayed byte must fit its viewport and every enclosing scroller.
export function hexReachability(viewer, offset) {
  const clipped=[];
  const visible=element=>{
    if(!element)return false;const rect=element.getBoundingClientRect();
    let left=0,top=0,right=innerWidth,bottom=innerHeight;
    for(let parent=element.parentElement;parent;parent=parent.parentElement??parent.getRootNode().host??null){
      const style=getComputedStyle(parent),bounds=parent.getBoundingClientRect(),scaleX=bounds.width/(parent.offsetWidth||1),scaleY=bounds.height/(parent.offsetHeight||1);
      if(/auto|scroll|hidden|clip/.test(style.overflowX)){left=Math.max(left,bounds.left+parent.clientLeft*scaleX);right=Math.min(right,bounds.left+(parent.clientLeft+parent.clientWidth)*scaleX);}
      if(/auto|scroll|hidden|clip/.test(style.overflowY)){top=Math.max(top,bounds.top+parent.clientTop*scaleY);bottom=Math.min(bottom,bounds.top+(parent.clientTop+parent.clientHeight)*scaleY);}
    }
    const result=rect.width>0&&rect.height>0&&rect.left>=left&&rect.right<=right&&rect.top>=top&&rect.bottom<=bottom;if(!result)clipped.push({element:element.className,rect:rect.toJSON(),clip:{left,top,right,bottom}});return result;
  };
  const viewport=viewer.viewport.getBoundingClientRect(),tail=viewer.querySelector(`[data-byte="${offset}"][data-column="hex"]`),bounds=tail?.getBoundingClientRect(),rowHeight=viewer.columnHeader.getBoundingClientRect().height;
  return {visible:visible(viewer.querySelector('.hex-status span'))&&visible(viewer.querySelector('.hex-status button')),rows:viewer.hexRows.length,tailVisible:!!bounds&&bounds.top>=viewport.top+rowHeight&&bounds.bottom<=viewport.bottom&&visible(tail),viewport:viewport.toJSON(),tail:bounds?.toJSON(),rowHeight,clientHeight:viewer.viewport.clientHeight,clipped};
}

export async function checkHexViewer(page, inspector, screenshotPath) {
  const viewer = inspector.locator('hex-viewer');
  const grid = viewer.getByRole('grid');
  const cell = (index, column = 'hex') => viewer.locator(`.hex-cell[data-byte="${index}"][data-column="${column}"]`);
  const selected = column => viewer.locator(`.hex-cell[data-selected][data-column="${column}"]`).evaluateAll(cells => cells.map(cell => Number(cell.dataset.byte)));
  const state = () => viewer.evaluate(element => [Number(element.caret), Number(element.rangeStart), Number(element.rangeEnd), element.activeColumn]);
  const fixture = async (bytes, truncated = false) => {
    await inspector.evaluate(async (element, { bytes, truncated }) => {
      const fixture = globalThis.__workspaceFixture;
      fixture.hexBytes = bytes; fixture.hexTruncated = truncated;
      await element.inspectBody();
    }, { bytes, truncated });
  };
  const drag = async (from, to, column = 'hex') => {
    const start = await cell(from, column).boundingBox();
    const end = await cell(to, column).boundingBox();
    await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
    await page.mouse.down();
    await page.mouse.move(end.x + end.width / 2, end.y + end.height / 2, { steps: 6 });
    await page.mouse.up();
  };
  const copied = () => page.evaluate(() => globalThis.__hexCopied.at(-1));
  const openSubmenu = async () => {
    await grid.press('Shift+F10');
    await viewer.getByRole('menuitem', { name: 'Copy As', exact: true }).hover();
    await viewer.getByRole('menu', { name: 'Copy bytes as', exact: true }).waitFor({ state: 'visible' });
  };
  const original = await page.evaluate(() => {
    const override = globalThis.__workspaceFixture.bodyOverride;
    globalThis.__workspaceFixture.bodyOverride = { ...override, availability: 'complete' };
    globalThis.__hexCopied = [];
    globalThis.__hexClipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => globalThis.__hexCopied.push(text) } });
    return override;
  });
  try {
    for (const availability of ['complete', 'capturing', 'lost']) {
      await page.evaluate(availability => {
        Object.assign(globalThis.__workspaceFixture.bodyOverride, { availability, retainedBytes: 8, observedBytes: 8 });
      }, availability);
      await fixture([1, 0x47, 1, 0x57, 1, 0x5b, 1, 0x65]);
      await grid.press('Shift+F10');
      const selectAll = viewer.getByRole('menuitem', { name: /^Select all/ });
      assert.equal((await selectAll.textContent()).includes('(truncated)'), false, 'A fully displayed body was labeled truncated: ' + availability);
      await selectAll.click();
      await grid.press('Control+c');
      assert.equal(await copied(), 'AUcBVwFbAWU=', 'Select all did not copy the entire displayed body');
    }
    await page.evaluate(() => globalThis.__workspaceFixture.bodyOverride.availability = 'complete');
    await fixture([1, 0x47, 1, 0x57, 1, 0x5b, 1, 0x65]);
    await drag(0, 7);
    assert.deepEqual(await selected('hex'), [0,1,2,3,4,5,6,7]);
    assert.deepEqual(await selected('text'), await selected('hex'));
    assert.equal(await cell(7).getAttribute('aria-selected'), 'true');
    assert.equal(await cell(7).getAttribute('data-caret'), '');
    await grid.press('Control+c');
    assert.equal(await copied(), 'AUcBVwFbAWU=');
    const nativeCopy = await viewer.evaluate(element => {
      const data = new DataTransfer();
      const event = new ClipboardEvent('copy', { bubbles: true, cancelable: true, clipboardData: data });
      element.viewport.dispatchEvent(event);
      return [event.defaultPrevented, data.getData('text/plain')];
    });
    assert.deepEqual(nativeCopy, [true, 'AUcBVwFbAWU=']);
    await drag(7, 2, 'text');
    assert.deepEqual(await selected('text'), [2,3,4,5,6,7]);
    assert.deepEqual(await selected('hex'), await selected('text'));
    assert.deepEqual(await state(), [2,2,7,'text']);
    await grid.press('Shift+ArrowRight');
    assert.deepEqual(await state(), [3,3,7,'text']);
    await grid.press('Control+ArrowRight');
    assert.deepEqual(await state(), [4,-1,-1,'text']);
    await grid.press('Control+Shift+ArrowRight');
    assert.deepEqual(await state(), [5,4,5,'text']);
    await grid.press('ArrowLeft');
    assert.deepEqual(await state(), [4,-1,-1,'text']);
    await cell(1).click();
    await cell(6).click({ modifiers: ['Shift'] });
    assert.deepEqual(await state(), [6,1,6,'hex']);
    await grid.press('Tab');
    assert.deepEqual(await state(), [6,1,6,'text']);
    await grid.press('Shift+Tab');
    assert.deepEqual(await state(), [6,1,6,'hex']);
    await grid.press('Control+a');
    await cell(3).click({ button: 'right' });
    assert.deepEqual(await state(), [7,0,7,'hex'], 'Context menu destroyed an existing selection');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('ArrowRight');
    assert.equal(await viewer.getByRole('menuitem', { name: 'Hex', exact: true }).evaluate(button => button === button.getRootNode().activeElement), true);
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Escape');
    assert.equal(await viewer.getByRole('menuitem', { name: 'Copy As', exact: true }).evaluate(button => button === button.getRootNode().activeElement), true);
    await page.keyboard.press('Escape');
    assert.equal(await grid.evaluate(element => element === element.getRootNode().activeElement), true);

    const sample = [0,0xa7,0,0xa9,0,0xab,0,0xaf,0,0xb3,0,0xb7,0,0xbb,0,0xc5,0,0xcf,0];
    await fixture(sample, true);
    await grid.press('Control+a');
    const expected = {
      Hex: '00a700a900ab00af00b300b700bb00c500cf00',
      Literal: '\\x00\\xa7\\x00\\xa9\\x00\\xab\\x00\\xaf\\x00\\xb3\\x00\\xb7\\x00\\xbb\\x00\\xc5\\x00\\xcf\\x00',
      C: 'unsigned char from_transmog[19] =\n{\n\t0x00, 0xa7, 0x00, 0xa9, 0x00, 0xab, 0x00, 0xaf, \n\t0x00, 0xb3, 0x00, 0xb7, 0x00, 0xbb, 0x00, 0xc5, \n\t0x00, 0xcf, 0x00, \n};',
      Go: 'var from_transmog = []byte{\n\t0x00, 0xa7, 0x00, 0xa9, 0x00, 0xab, 0x00, 0xaf, \n\t0x00, 0xb3, 0x00, 0xb7, 0x00, 0xbb, 0x00, 0xc5, \n\t0x00, 0xcf, 0x00, \n}',
      Java: 'byte from_transmog[] =\n{\n\t0x00, 0xa7, 0x00, 0xa9, 0x00, 0xab, 0x00, 0xaf, \n\t0x00, 0xb3, 0x00, 0xb7, 0x00, 0xbb, 0x00, 0xc5, \n\t0x00, 0xcf, 0x00, \n};',
      JSON: '{"0":0,"1":167,"2":0,"3":169,"4":0,"5":171,"6":0,"7":175,"8":0,"9":179,"10":0,"11":183,"12":0,"13":187,"14":0,"15":197,"16":0,"17":207,"18":0}',
      Base64: 'AKcAqQCrAK8AswC3ALsAxQDPAA=='
    };
    for (const [label, text] of Object.entries(expected)) {
      await openSubmenu();
      if (label === 'Hex') {
        const bounds=await viewer.getByRole('menu',{name:'Copy bytes as',exact:true}).boundingBox();
        const viewport=page.viewportSize();
        assert.ok(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=viewport.width&&bounds.y+bounds.height<=viewport.height,'Copy As submenu escaped the viewport');
        await page.screenshot({path:screenshotPath.replace('.png','-menu.png')});
      }
      assert.equal(await viewer.getByRole('menuitem', { name: 'UTF-8', exact: true }).isEnabled(), false);
      assert.equal(await viewer.getByRole('menuitem', { name: 'Select all (truncated)', exact: false }).count(), 0,'Paged Hex inherited the text preview limit');
      await viewer.getByRole('menuitem', { name: label, exact: true }).click();
      assert.equal(await copied(), text, label + ' copy format differs');
    }
    await drag(17, 1);
    assert.deepEqual(await selected('hex'), Array.from({length:17},(_,i)=>i+1));
    assert.deepEqual(await selected('text'), await selected('hex'));
    await page.screenshot({ path: screenshotPath });
    await fixture([0x68,0xc3,0xa9,0,0x78]);
    await grid.press('Control+a');
    await openSubmenu();
    assert.equal(await viewer.getByRole('menuitem', { name: 'UTF-8', exact: true }).isEnabled(), true);
    await viewer.getByRole('menuitem', { name: 'UTF-8', exact: true }).click();
    assert.equal(await copied(), 'hé');
    await fixture([0,0x61]);
    await grid.press('Control+a'); await openSubmenu();
    await viewer.getByRole('menuitem', { name: 'UTF-8', exact: true }).click();
    assert.equal(await copied(), '');
    await fixture([0x61,0,0xff]);
    await grid.press('Control+a'); await openSubmenu();
    assert.equal(await viewer.getByRole('menuitem', { name: 'UTF-8', exact: true }).isEnabled(), false);
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');

    // Address labels retain native text selection independently of byte highlighting.
    await viewer.locator('.hex-data-row .hex-address').evaluate(address => {
      const range = document.createRange(); range.selectNodeContents(address);
      window.getSelection().removeAllRanges(); window.getSelection().addRange(range);
    });
    assert.equal(await page.evaluate(() => window.getSelection().toString()), '00000000');
    assert.deepEqual(await state(), [2,0,2,'hex']);
    await cell(0).click();
    assert.equal(await page.evaluate(() => window.getSelection().toString()), '');

    await fixture(Array.from({length:262144},(_,i)=>i%256), true);
    assert.ok(await viewer.locator('.hex-data-row').count() < 100, 'Hex preview rendered every row');
    await grid.press('Control+End');
    assert.deepEqual((await state()).slice(0,3), [262143,-1,-1]);
    await cell(262143).waitFor({state:'visible'});
    await grid.press('Control+Home');
    await grid.press('Shift+ArrowDown');
    assert.deepEqual((await state()).slice(0,3), [16,0,16]);
    await grid.press('PageDown');
    assert.ok((await state())[0] > 16);
    await grid.press('Control+Home');
    const first = await cell(0).boundingBox();
    const bounds = await grid.boundingBox();
    await page.mouse.move(first.x+first.width/2,first.y+first.height/2); await page.mouse.down();
    await viewer.evaluate(element=>element.viewport.releasePointerCapture(element.pointer));
    await page.mouse.move(first.x+first.width/2,bounds.y+bounds.height+10);
    await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('hex-viewer').viewport.scrollTop > 0);
    await page.mouse.up();
    assert.ok((await state())[2] > 16, 'Dragging beyond the viewport did not extend the range');
    await grid.press('Control+a');
    assert.deepEqual((await state()).slice(0,3), [262143,0,262143]);
    // A synthetic multi-petabyte source has no browser scroll-height or integer ceiling.
    const total='9007199254741009',end='9007199254741008';
    await page.evaluate(total=>{const f=globalThis.__workspaceFixture;f.hexLargeTotal=total;f.rangeRequests=[];},total);
    await inspector.evaluate(element=>element.inspectBody());
    await grid.press('Control+End');
    assert.equal(await viewer.evaluate(element=>element.caret),end);
    await cell(end).waitFor({state:'visible'});
    await page.waitForFunction(end=>document.querySelector('app-shell').shadowRoot.querySelector('hex-viewer').querySelector(`[data-byte="${end}"][data-column="hex"]`)?.textContent==='10',end);
    assert.ok(await viewer.locator('.hex-data-row').count()<100);
    assert.ok(await grid.evaluate(element=>element.scrollHeight)<=4_000_100,'Native scroll canvas exceeded its bounded height');
    await grid.press('Shift+F10');
    await viewer.getByRole('menuitem',{name:'Copy Offset as Decimal',exact:true}).click();
    assert.equal(await copied(),end);
    await grid.press('Control+g');
    const offsetField=viewer.getByRole('textbox',{name:'Byte offset'});
    await offsetField.fill('0x20000000000002');await offsetField.press('Enter');
    assert.equal(await viewer.evaluate(element=>element.caret),'9007199254740994');
    assert.equal(await grid.evaluate(element=>element===element.getRootNode().activeElement),true,'Go to offset did not restore grid focus');
    await grid.press('Shift+ArrowRight');
    await grid.press('Control+c');
    await page.waitForFunction(()=>globalThis.__hexCopied.at(-1)==='AgM=');
    await grid.press('Control+a');
    assert.equal(await viewer.evaluate(element=>element.rangeEnd),end);
    await grid.press('Control+g');assert.equal(await offsetField.inputValue(),end,'Go to offset retained an earlier edited input');
    await offsetField.press('Escape');
    assert.equal(await grid.evaluate(element=>element===element.getRootNode().activeElement),true,'Escape did not return focus to the byte grid');
    const before=await page.evaluate(()=>globalThis.__hexCopied.length);
    await grid.press('Control+c');
    assert.equal(await page.evaluate(()=>globalThis.__hexCopied.length),before,'An oversized selection was copied');
    assert.ok(await page.evaluate(()=>globalThis.__workspaceFixture.rangeRequests.every(request=>request.maxBytes<=65536)),'Scrolling or selecting loaded the whole body');
    await page.screenshot({path:screenshotPath.replace('.png','-huge.png')});
    // Compact message panes scroll their chrome instead of clipping the byte row.
    const savedViewport=page.viewportSize(),savedPreferences=await page.evaluate(()=>document.querySelector('app-shell').traffic.preferences),savedSide=await page.evaluate(()=>document.querySelector('app-shell').traffic.inspectorSide);
    try {
      await page.setViewportSize({width:640,height:720});
      await page.evaluate(()=>{const t=document.querySelector('app-shell').traffic;t.preferences={...t.preferences,layout:'stacked',listSplit:45};});
      await page.locator('traffic-workspace').getByLabel('Message to inspect',{exact:true}).selectOption('response');
      for(const zoom of [1,1.5]){
        await page.setViewportSize({width:640,height:720});
        await page.evaluate(zoom=>document.documentElement.style.zoom=String(zoom),zoom);
        // Exercise a genuinely constrained pane, preserving a usable area at each text scale.
        await inspector.evaluate((element,zoom)=>{
          const t=document.querySelector('app-shell').traffic,content=element.querySelector('.message-content'),workspace=t.querySelector('.traffic-workspace');
          t.preferences={...t.preferences,listSplit:Math.max(15,Math.min(85,Math.round(t.preferences.listSplit+(content.getBoundingClientRect().height-110*zoom)/workspace.getBoundingClientRect().height*100)))};
        },zoom);
        const save=inspector.getByRole('button',{name:'Save as…',exact:true});await save.focus();
        assert.equal(await save.evaluate(element=>{const r=element.getBoundingClientRect(),c=element.closest('.message-content').getBoundingClientRect();return r.top>=c.top&&r.bottom<=c.bottom&&r.bottom<=innerHeight;}),true,'Save control became unreachable in the compact pane');
        await viewer.evaluate(element=>element.scrollIntoView({block:'nearest'}));await grid.focus();await grid.press('Control+End');
        await viewer.getByRole('button',{name:'Go to offset',exact:true}).focus();
        let reachable;for(let attempt=0;attempt<40;attempt++){reachable=await viewer.evaluate(hexReachability,end);if(reachable.visible&&reachable.tailVisible)break;await new Promise(resolve=>setTimeout(resolve,25));}assert.ok(reachable.visible&&reachable.rows<100&&reachable.tailVisible,'Compact Hex clipping: '+JSON.stringify(reachable));
        await viewer.getByRole('button',{name:'Go to offset',exact:true}).click();assert.equal(await offsetField.inputValue(),end);await offsetField.press('Escape');assert.equal(await grid.evaluate(element=>element===element.getRootNode().activeElement),true);
        await page.screenshot({path:screenshotPath.replace('.png',zoom===1?'-compact.png':'-compact-zoom.png')});
      }
    } finally {await page.evaluate(({preferences,side})=>{document.documentElement.style.zoom='';const t=document.querySelector('app-shell').traffic;t.preferences=preferences;t.inspectorSide=side;},{preferences:savedPreferences,side:savedSide});await page.setViewportSize(savedViewport);}
    // Late pages from a previous body snapshot must not restore its old size or bytes.
    await grid.press('Control+Home');
    await page.evaluate(()=>{const f=globalThis.__workspaceFixture;f.deferRangeOffset='50003968';});
    await grid.press('Control+g');await offsetField.fill('50004000');await offsetField.press('Enter');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.rangePending);
    await page.evaluate(()=>delete globalThis.__workspaceFixture.hexLargeTotal);
    await fixture([0x41,0x42]);
    await page.evaluate(()=>{const f=globalThis.__workspaceFixture;delete f.deferRangeOffset;f.releaseRange();delete f.releaseRange;delete f.rangePending;});
    await page.waitForTimeout(100);
    assert.ok((await state())[0]<=1,'A stale page restored the previous source length');
    await grid.press('Control+End');assert.equal((await state())[0],1);
    assert.equal(await cell(1).textContent(),'42');
    await fixture([0x41,0x42]);
    assert.ok((await state())[2] <= 1, 'Shrinking a preview left the selection outside its bytes');
    await viewer.evaluate(element => element.preview = { ...element.preview, key: 'different-body' });
    assert.deepEqual((await state()).slice(0,3), [0,-1,-1], 'A new body inherited the old selection');
    await viewer.evaluate(element => element.preview = { ...element.preview, offset: 256 });
    await cell(1).click({button:'right'});
    assert.deepEqual((await state()).slice(0,3), [1,1,1]);
    await viewer.getByRole('menuitem',{name:'Copy Offset as Decimal',exact:true}).click();
    assert.equal(await copied(),'257');
    await grid.press('Shift+F10');
    await viewer.getByRole('menuitem',{name:'Copy Offset as Hex',exact:true}).click();
    assert.equal(await copied(),'0x101');
    await grid.press('Shift+F10');
    await page.keyboard.press('Escape');
    await page.emulateMedia({reducedMotion:'reduce'});
    assert.equal(await cell(1).evaluate(element=>getComputedStyle(element,'::before').animationName),'none');
    await page.emulateMedia({reducedMotion:'no-preference'});
    const retainedDetail = await inspector.evaluate(element=>structuredClone(element.detail));
    await inspector.evaluate(element=>{
      const detail=structuredClone(element.detail);
      Object.assign(detail.storedBodies[0],{availability:'evicted',retainedBytes:0});
      element.detail=detail;
    });
    assert.equal(await viewer.count(),0,'An evicted body still exposed its previous byte selection');
    await inspector.evaluate((element,detail)=>element.detail=detail,retainedDetail);
  } finally {
    await page.evaluate(override => {
      const fixture = globalThis.__workspaceFixture;
      fixture.bodyOverride = override;
      delete fixture.hexBytes; delete fixture.hexTruncated;
      delete fixture.hexLargeTotal;delete fixture.rangeRequests;delete fixture.deferRangeOffset;
      if (globalThis.__hexClipboardDescriptor) Object.defineProperty(navigator, 'clipboard', globalThis.__hexClipboardDescriptor);
      else delete navigator.clipboard;
      delete globalThis.__hexClipboardDescriptor; delete globalThis.__hexCopied;
    }, original);
    await inspector.evaluate(element => element.inspectBody());
  }
}
