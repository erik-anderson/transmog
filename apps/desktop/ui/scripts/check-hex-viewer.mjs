import assert from 'node:assert/strict';

export async function checkHexViewer(page, inspector, screenshotPath) {
  const viewer = inspector.locator('hex-viewer');
  const grid = viewer.getByRole('grid');
  const cell = (index, column = 'hex') => viewer.locator(`.hex-cell[data-byte="${index}"][data-column="${column}"]`);
  const selected = column => viewer.locator(`.hex-cell[data-selected][data-column="${column}"]`).evaluateAll(cells => cells.map(cell => Number(cell.dataset.byte)));
  const state = () => viewer.evaluate(element => [element.caret, element.rangeStart, element.rangeEnd, element.activeColumn]);
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
      assert.equal(await viewer.getByRole('menuitem', { name: 'Select all (truncated)', exact: false }).count(), 1);
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
      if (globalThis.__hexClipboardDescriptor) Object.defineProperty(navigator, 'clipboard', globalThis.__hexClipboardDescriptor);
      else delete navigator.clipboard;
      delete globalThis.__hexClipboardDescriptor; delete globalThis.__hexCopied;
    }, original);
    await inspector.evaluate(element => element.inspectBody());
  }
}
