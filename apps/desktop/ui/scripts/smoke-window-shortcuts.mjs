import assert from 'node:assert/strict';

async function key(client,value,modifiers=2) {
  const code='Key'+value.toUpperCase(),windowsVirtualKeyCode=value.toUpperCase().charCodeAt(0);
  await client.call('Input.dispatchKeyEvent',{type:'rawKeyDown',key:value,code,windowsVirtualKeyCode,modifiers});
  await client.call('Input.dispatchKeyEvent',{type:'keyUp',key:value,code,windowsVirtualKeyCode,modifiers});
}

export async function smokeWindowShortcuts(main,viewer,targets,connect,waitFor,closeNative) {
  const shell='document.querySelector("app-shell")',traffic=shell+'.traffic';
  const original=await targets();
  for(const client of [main,viewer]) {
    await client.evaluate(`${traffic}.filesMenuButton.focus()`);
    await key(client,'f');
    assert.equal(await client.evaluate(`${traffic}.searchInput.getRootNode().activeElement===${traffic}.searchInput`),true,'Native Ctrl+F did not focus Traffic');
    await key(client,'s');await waitFor(()=>client.evaluate(`${traffic}.saveTraceDialog.open`),'Native Ctrl+S did not open Save copy');
    await client.call('Input.dispatchKeyEvent',{type:'rawKeyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await client.call('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
    await waitFor(()=>client.evaluate(`!${traffic}.saveTraceDialog.open`),'Save copy did not dismiss');
    await key(client,'o');
    await closeNative('Open saved traffic');
    await waitFor(()=>client.evaluate(`!${traffic}.pickingTrace`),'Native import picker did not cancel');
    await key(client,'n');
    const created=await waitFor(async()=>{const all=await targets();return all.find(target=>!original.some(before=>before.id===target.id));},'Native Ctrl+N did not create a viewer');
    const blank=await connect(created);
    await waitFor(()=>blank.evaluate(`${shell}?.viewerMode&&${traffic}?.queryLoaded`),'Empty viewer did not initialize');
    assert.equal(await blank.evaluate(`${traffic}.sessions.length`),0,'New viewer was not empty');
    await key(blank,'w');
    await waitFor(async()=>!(await targets()).some(target=>target.id===created.id),'Native Ctrl+W did not close the viewer');blank.close();
  }
  await key(main,'w');
  assert.equal((await targets()).length,original.length,'Ctrl+W closed the main proxy window');
  process.stdout.write('Native Ctrl+N, Ctrl+W, Ctrl+F, Ctrl+O and Ctrl+S verified in main and viewer windows.\n');
}
