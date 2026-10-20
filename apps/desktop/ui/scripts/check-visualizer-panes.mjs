import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkVisualizerPanes(page, root, visualizer) {
  const divider = visualizer.getByRole('separator', {name:'Resize structure tree and data', exact:true});
  const tree = visualizer.getByRole('tree', {name:'Visualization structure', exact:true});
  const starts = await page.evaluate(() => globalThis.__workspaceFixture.calls.activate_visualizer);
  await divider.waitFor();
  assert.equal(await divider.getAttribute('aria-orientation'), 'vertical');
  const originalWidth = await tree.evaluate(element => element.clientWidth);
  await divider.focus();
  await page.keyboard.press('Shift+ArrowRight');
  assert.equal(await divider.getAttribute('aria-valuenow'), '58');
  assert.ok(await tree.evaluate(element => element.clientWidth) > originalWidth + 20);
  const layout = await visualizer.locator('.visualizer-layout').boundingBox();
  const grip = await divider.boundingBox();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + 15);
  await page.mouse.down();
  await page.mouse.move(layout.x + layout.width * .67, grip.y + 15);
  await page.mouse.up();
  assert.equal(await divider.getAttribute('aria-valuenow'), '67');
  const originalStyle = await visualizer.getAttribute('style');
  try {
    await visualizer.evaluate(element => {element.style.width='360px';element.style.height='400px';});
    await page.waitForFunction(element => element.visualizerOrientation === 'horizontal', await visualizer.elementHandle());
    assert.equal(await divider.getAttribute('aria-orientation'), 'horizontal');
    assert.equal(await divider.getAttribute('aria-valuenow'), '40');
    const originalHeight = await tree.evaluate(element => element.clientHeight);
    await divider.focus();
    await page.keyboard.press('Shift+ArrowDown');
    assert.equal(await divider.getAttribute('aria-valuenow'), '50');
    assert.ok(await tree.evaluate(element => element.clientHeight) > originalHeight + 15);
    await page.screenshot({path:resolve(root,'../../../target/ui-check/visualizer-resizable-compact.png')});
    await page.keyboard.press('Home');
  } finally {
    await visualizer.evaluate((element, style) => {
      if (style === null) element.removeAttribute('style');
      else element.setAttribute('style', style);
    }, originalStyle);
  }
  await page.waitForFunction(element => element.visualizerOrientation === 'vertical', await visualizer.elementHandle());
  assert.equal(await divider.getAttribute('aria-valuenow'), '67','Changing orientation lost the chosen width');
  await divider.focus();
  await page.keyboard.press('Home');
  assert.equal(await divider.getAttribute('aria-valuenow'), '48');
  assert.equal(await page.evaluate(() => globalThis.__workspaceFixture.calls.activate_visualizer), starts,'Resizing restarted the worker');
  const button = tree.locator('button:visible').first();
  const previousTheme = await page.locator('app-shell').getAttribute('data-theme');
  try {
    for (const theme of ['light','dark']) {
      await page.locator('app-shell').evaluate((element, value) => element.setAttribute('data-theme', value), theme);
      const contrast = await button.evaluate(element => {
        const luminance = value => {
          const channels=value.match(/[\d.]+/g).slice(0,3).map(Number).map(value => {
            value /= 255;
            return value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4;
          });
          return channels[0]*.2126 + channels[1]*.7152 + channels[2]*.0722;
        };
        const style=getComputedStyle(element),a=luminance(style.color),b=luminance(style.backgroundColor);
        return (Math.max(a,b)+.05)/(Math.min(a,b)+.05);
      });
      assert.ok(contrast >= 4.5,`${theme} expand/collapse contrast is ${contrast}`);
    }
    await page.emulateMedia({forcedColors:'active'});
    assert.equal(await button.evaluate(element => getComputedStyle(element).forcedColorAdjust), 'auto');
  } finally {
    await page.emulateMedia({forcedColors:'none'});
    await page.locator('app-shell').evaluate((element, value) => {
      if (value === null) element.removeAttribute('data-theme');
      else element.setAttribute('data-theme', value);
    }, previousTheme);
  }
}
