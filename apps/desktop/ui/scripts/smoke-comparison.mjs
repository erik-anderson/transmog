import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

export async function smokeComparison(viewer,main,traffic,screenshot){
  const result=await viewer.evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),id=${traffic}.sessions[0].id;
    const input={left:{kind:'exchange',id},right:{kind:'exchange',id}};
    await shell.comparison.open(input);
    const result=shell.comparison.comparisonResult;
    if(!result)throw new Error(shell.comparison.comparisonStatus);
    const page=await window.__TAURI_INTERNALS__.invoke('comparison_bytes',{id:result.id,section:'Response',offset:0});
    const denial=await window.__TAURI_INTERNALS__.invoke('compare_evidence',{input:{operationId:'viewer-authoring-denial',left:{kind:'asset',reference:'none@1'},right:{kind:'asset',reference:'none@1'}}}).then(()=>'',e=>e?.message??String(e));
    return {equal:result.rows.some(r=>r.field==='Decoded bytes'&&r.state==='equal'),complete:result.complete,bytesEqual:page.left===page.right,denial};
  })()`);
  assert.equal(result.equal,true);assert.equal(result.complete,false);assert.equal(result.bytesEqual,true);assert.match(result.denial,/main window/);
  if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-comparison-viewer.png'),Buffer.from(image.data,'base64'));}
  await viewer.evaluate(`document.querySelector('app-shell').comparison.closeComparison()`);
  const draft=await main.evaluate(`(async()=>{
    const invoke=window.__TAURI_INTERNALS__.invoke;
    const before=await invoke('automation_status');
    const bytes=text=>Array.from(new TextEncoder().encode(text));
    const asset=await invoke('create_response_asset',{input:{id:'native-comparison',revision:1,status:200,headers:[{name:bytes('content-type'),value:bytes('text/plain; charset=utf-8')}],body:bytes('before'),mediaType:'text/plain'}});
    const base={kind:'asset',reference:asset.id+'@'+asset.revision};
    await document.querySelector('app-shell').comparison.open({left:base,right:{kind:'draft',label:'Native unsaved draft',status:'404',headers:asset.headers,decodedBody:bytes('after'),base,bodyPath:null}});
    const result=document.querySelector('app-shell').comparison.comparisonResult;
    if(!result)throw new Error(document.querySelector('app-shell').comparison.comparisonStatus);
    const after=await invoke('automation_status');
    return {different:result.different,status:result.rows.some(r=>r.field==='Status'&&r.left==='200'&&r.right==='404'),body:result.rows.some(r=>r.field==='Decoded bytes'&&r.state==='different'),rulesUnchanged:JSON.stringify(before.rules)===JSON.stringify(after.rules),gateUnchanged:before.autoresponsesEnabled===after.autoresponsesEnabled};
  })()`);
  assert.deepEqual(draft,{different:true,status:true,body:true,rulesUnchanged:true,gateUnchanged:true});
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-comparison-draft.png'),Buffer.from(image.data,'base64'));}
  await main.evaluate(`document.querySelector('app-shell').comparison.closeComparison()`);
  process.stdout.write('Native comparison: viewer evidence/permissions, byte equality and unsaved response differences verified.\n');
}

export async function smokePreviewComparison(owner,other,label){
  const result=await owner.evaluate(`(async()=>{
    const invoke=window.__TAURI_INTERNALS__.invoke,label=${JSON.stringify(label)};
    const before=await invoke('captured_page_report',{label}),observation=before.requests.find(r=>r.outcome==='captured'||r.outcome==='auto-response');
    const subject={kind:'preview',run:label,observation:observation.id};
    const result=await invoke('compare_evidence',{input:{operationId:'native-preview-comparison',left:subject,right:subject}});
    const after=await invoke('captured_page_report',{label});
    await invoke('close_comparison',{id:result.id});
    return {equal:result.rows.some(r=>r.field==='Decoded bytes'&&r.state==='equal'),complete:result.complete,unchanged:before.hits===after.hits&&before.misses===after.misses&&before.requests.length===after.requests.length,observation:observation.id};
  })()`);
  assert.equal(result.equal,true);assert.equal(result.complete,false);assert.equal(result.unchanged,true);
  assert.equal(await other.evaluate(`window.__TAURI_INTERNALS__.invoke('compare_evidence',{input:{operationId:'cross-window-preview-denial',left:{kind:'preview',run:${JSON.stringify(label)},observation:${result.observation}},right:{kind:'exchange',id:'00000000000000000000000000000000'}}}).then(()=>false,()=>true)`),true);
  process.stdout.write('Native preview response comparison is immutable, read-only and owner scoped.\n');
}
