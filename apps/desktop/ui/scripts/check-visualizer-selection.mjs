import assert from 'node:assert/strict';

export async function checkVisualizerSelection(page, visualizer, tree) {
  const mode = visualizer.getByRole('combobox', {name: 'Structure view', exact: true});
  const selected = tree.getByRole('treeitem', {selected: true});

  // A table can reach rows which are outside the virtual tree window or hidden
  // beneath a collapsed ancestor. Selecting one must restore a navigable tree.
  await tree.focus();
  await page.keyboard.press('Home');
  await page.keyboard.press('ArrowLeft');
  await mode.selectOption('table');
  await visualizer.getByRole('button', {name: 'Next', exact: true}).click();
  await visualizer.getByRole('button', {name: 'Field 50', exact: true}).click();
  assert.equal(await selected.count(), 1, 'Selecting a table row left no visible tree selection');
  assert.ok((await selected.textContent()).includes('Field 50'));
  assert.equal(await tree.evaluate(element => element === element.getRootNode().activeElement), true,
    'Selecting a table row left focus on a disappearing control');
  const bounds = await selected.evaluate(element => {
    const row = element.getBoundingClientRect(), viewport = element.parentElement.parentElement.getBoundingClientRect();
    return {rowTop: row.top, rowBottom: row.bottom, viewportTop: viewport.top, viewportBottom: viewport.bottom};
  });
  assert.ok(bounds.rowTop >= bounds.viewportTop - 1 && bounds.rowBottom <= bounds.viewportBottom + 1,
    'The selected table row remained outside the tree viewport: ' + JSON.stringify(bounds));
  await page.keyboard.press('ArrowDown');
  await visualizer.locator('.visualizer-node-heading > strong').filter({hasText: 'Field 51'}).waitFor();

  await page.keyboard.press('Home');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await mode.selectOption('table');
  await visualizer.getByRole('button', {name: 'Nested text', exact: true}).click();
  assert.equal(await selected.count(), 1, 'A selected nested table row stayed under a collapsed branch');
  assert.ok((await selected.textContent()).includes('Nested text'));
  await tree.getByRole('button', {name: 'Collapse Field 1', exact: true}).waitFor();
  const leaf = await tree.getAttribute('aria-activedescendant');
  await page.keyboard.press('ArrowRight');
  assert.equal(await tree.getAttribute('aria-activedescendant'), leaf,
    'Right Arrow on a leaf moved to an unrelated sibling');
  await page.keyboard.press('ArrowLeft');
  await visualizer.locator('.visualizer-node-heading > strong').filter({hasText: 'Field 1'}).waitFor();
  assert.ok(await tree.getByRole('treeitem').count() < 80, 'Revealing a table selection created unbounded DOM');
}
