import assert from 'node:assert/strict';
import {readFile,writeFile,realpath} from 'node:fs/promises';
import {dirname,join,resolve} from 'node:path';

/** Real candidate processes and retained capabilities, independent of development UI controls. */
export async function smokeVisualizerReload(evaluate,tracePath) {
  const invoke=(command,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const status=await invoke('extension_status'),entry=status.entries.find(entry=>entry.catalog.manifest.id==='test.development');
  const directory=await realpath(join(dirname(tracePath),'extension-source'));
  assert.equal(resolve(entry.directory).toLowerCase(),directory.toLowerCase(),'Reload probe must use its owned source fixture');
  const path=join(directory,'parser.js'),original=await readFile(path,'utf8');
  const pageId=await invoke('visualizer_page'),slot='native-reload-probe';let generation=1;
  try {
    const selection=await evaluate(`(async()=>{const t=document.querySelector('app-shell').traffic;await t.refreshSessions(undefined,true);const view=await window.__TAURI_INTERNALS__.invoke('query_sessions',{query:{search:'/extension-fixture',limit:100,offset:0}});const row=view.sessions.find(row=>row.path==='/extension-fixture'&&row.responseBytes===23);if(!row)throw new Error('Reload retained fixture is missing');return {sessionId:row.id,boundary:'client-response',surface:'body'};})()`);
    const first=await invoke('activate_visualizer',{request:{slot,generation:String(generation),pageId,selection,extensionId:'test.development',visualizerId:'native-body'}});
    const changed=original.replace("title:'Synthetic protocol'","title:'Reloaded protocol'");assert.notEqual(changed,original);
    await writeFile(path,changed);
    const second=await invoke('reload_visualizer',{request:{handle:first.handle,generation:String(++generation)}});
    assert.notEqual(second.instance.id,first.instance.id);assert.equal(second.document.title,'Reloaded protocol');
    assert.equal(await invoke('visualizer_active',{handle:first.handle}),false,'Successful replacement retained old capabilities');
    const source=first.document.roots[0].spans[0].sourceId;
    await assert.rejects(()=>invoke('visualizer_source',{handle:first.handle,request:{sourceId:source,offset:'0',maxBytes:1}}));
    const published=(await invoke('extension_status')).entries[0].catalog.digest;
    for(const source of ['this is not JavaScript!',"export function visualize(){throw new Error('failed parser');}","export function visualize(){return {kind:'document',document:{title:'Invalid',status:'complete',presentation:'tree',roots:[{id:'broken'}]}};}"]) {
      await writeFile(path,source);
      await assert.rejects(()=>invoke('reload_visualizer',{request:{handle:second.handle,generation:String(++generation)}}));
      assert.equal(await invoke('visualizer_active',{handle:second.handle}),true,'A failed candidate discarded the current visualization');
      assert.equal((await invoke('extension_status')).entries[0].catalog.digest,published,'A failed candidate published its declaration');
      const sourceId=second.document.roots[0].spans[0].sourceId;
      assert.equal(Buffer.from((await invoke('visualizer_source',{handle:second.handle,request:{sourceId,offset:'0',maxBytes:8}})).bytesBase64,'base64').toString(),'Syntheti');
    }
    await writeFile(path,'while(true){}');
    const pending=assert.rejects(()=>invoke('reload_visualizer',{request:{handle:second.handle,generation:String(++generation)}}));
    await new Promise(resolve=>setTimeout(resolve,100));
    const began=Date.now();await invoke('deactivate_visualizer',{slot,generation:String(generation),pageId});await pending;
    assert.ok(Date.now()-began<3000,'Cancel waited for candidate initialization to time out');
    assert.equal(await invoke('visualizer_active',{handle:second.handle}),false);
    assert.equal((await invoke('extension_status')).entries[0].catalog.digest,published,'Canceled initialization published metadata');
    return {freshWorker:true,oldHandlesRevoked:true,failedSourceRetained:true,failedDocumentRetained:true,sourceCapabilitiesRetained:true,canceledStartup:true};
  } finally {
    await writeFile(path,original);
    await invoke('deactivate_visualizer',{slot,generation:String(generation),pageId});
  }
}
