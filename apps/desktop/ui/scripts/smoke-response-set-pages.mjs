import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

// The caller owns a uniquely named native executable, profile, and CDP endpoint.
export async function smokeResponseSetPages({main,waitFor,directory,restart=false}) {
  const shell="document.querySelector('app-shell')",a=shell+'.automation',v=a+'.experimentVariants',picker=shell+'.responseSetPagePicker',dialog=picker+'.pageDialog';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const pointer=async expression=>{const point=await main.evaluate(`(()=>{const b=${expression};b.scrollIntoView({block:'nearest'});const r=b.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);for(const type of ['mouseMoved','mousePressed','mouseReleased'])await main.call('Input.dispatchMouseEvent',{type,button:'left',buttons:type==='mousePressed'?1:0,clickCount:1,...point});};
  const key=async key=>{const windowsVirtualKeyCode=key==='Escape'?27:13;await main.call('Input.dispatchKeyEvent',{type:'keyDown',key,code:key,windowsVirtualKeyCode,...(key==='Enter'?{text:'\r',unmodifiedText:'\r'}:{})});await main.call('Input.dispatchKeyEvent',{type:'keyUp',key,code:key,windowsVirtualKeyCode});};
  const preview=v+".querySelector('.variant-preview-actions button')",change=v+".querySelector('.variant-page button')",use=dialog+".querySelector('footer button')";
  await waitFor(`${shell}?.workspaceLoaded`);
  let status=await invoke('automation_status');
  if(!status.experiments.length){
    const asset=await invoke('create_response_asset',{input:{id:'page-native-response',revision:1,status:200,headers:[],body:[...Buffer.from('Saved Facebook POST')],mediaType:'application/json'}});
    const action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
    const rule={id:'page-native-rule',displayName:'Saved Facebook POST',revision:1,priority:1,enabled:true,matcher:{method:'POST',url:{kind:'exact',value:'https://facebook.invalid/api'},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null},request:{...action,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:action};
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'create',id:'native-pages',name:'Facebook POST response set',variantId:'edited',variantName:'Edited response',rules:[rule]}});
    status=await invoke('change_experiment',{generation:status.generation,change:{action:'activate',selection:{experimentId:'native-pages',variantId:'edited',revision:1},acknowledgeOverlaps:false}});
  }
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${a}.renderAutomation(${JSON.stringify(status)});${a}.showAutomationSection('experiments');await ${v}.reveal({experimentId:'native-pages',variantId:'baseline',revision:1},false);})()`);
  await waitFor(`${v}.preferencesReady&&!${v}.guidePending`);
  await main.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))');
  if(restart){
    await waitFor(`${v}.pageAvailability==='Capture unavailable'`);assert.equal(await main.evaluate(`${v}.pageUrl`),'https://transmog.invalid/');
    const remembered=status.experiments[0].previewPage;
    await pointer(preview);await waitFor(`${dialog}.open`);assert.equal(await main.evaluate(`${picker}.selectedId`),'');assert.equal(await main.evaluate(`${picker}.rememberedUrl`),remembered.url);
    await key('Escape');assert.deepEqual(await invoke('automation_status'),status);
    await writeFile(new URL('native-restart.json',directory),JSON.stringify({remembered,availability:await main.evaluate(`${v}.pageAvailability`),status:await invoke('automation_status')},null,2));return;
  }
  const before=structuredClone(status);
  await pointer(preview);await waitFor(`${dialog}.open`);assert.equal(await main.evaluate(`${picker}.pages.length`),0);await key('Escape');assert.equal(await main.evaluate(`${preview}===${v}.getRootNode().activeElement`),true);
  const fixture=async(name,urls)=>{const text='<html><head><title>Page association verification</title></head><body><h1>Captured page</h1></body></html>';const entry=url=>({startedDateTime:'2026-10-18T18:00:00.000Z',time:1,request:{method:'GET',url,httpVersion:'HTTP/1.1',headers:[],queryString:[],cookies:[],headersSize:-1,bodySize:0},response:{status:200,statusText:'OK',httpVersion:'HTTP/1.1',headers:[{name:'Content-Type',value:'text/html'}],cookies:[],content:{size:text.length,mimeType:'text/html',text},redirectURL:'',headersSize:-1,bodySize:text.length},cache:{},timings:{send:0,wait:0,receive:1}});const path=new URL(name,directory);await writeFile(path,JSON.stringify({log:{version:'1.2',creator:{name:'Owned page association test',version:'1'},entries:urls.map(entry)}}));return decodeURIComponent(path.pathname).replace(/^\/(\w:)/,'$1');};
  const first=await fixture('pages.har',['https://transmog.invalid/','https://other.invalid/'+('long/'.repeat(30)),'https://transmog.invalid/']);
  await main.evaluate(`${shell}.traffic.importPaths([${JSON.stringify(first)}])`);await waitFor(`${shell}.traffic.totalMatched===3`);
  await pointer(preview);await waitFor(`${dialog}.open`);assert.equal(await main.evaluate(`${picker}.selectedId`),'');assert.equal(await main.evaluate(`${dialog}.querySelectorAll('input:checked').length`),0);
  for(const [width,height] of [[1440,900],[960,720],[800,600]]){await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(new URL(`native-${width}.png`,directory),Buffer.from(image.data,'base64'));assert.equal(await main.evaluate(`(()=>{const r=${dialog}.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;})()`),true);}
  await main.call('Emulation.setEmulatedMedia',{features:[{name:'forced-colors',value:'active'}]});await main.evaluate("document.documentElement.style.zoom='1.5'");const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(new URL('native-zoom-contrast.png',directory),Buffer.from(image.data,'base64'));await main.evaluate("document.documentElement.style.zoom=''");await main.call('Emulation.setEmulatedMedia',{features:[]});
  const index=await main.evaluate(`${picker}.pages.findIndex(page=>page.url==='https://transmog.invalid/')`);await pointer(dialog+`.querySelectorAll('input')[${index}]`);
  // Delay the real durable reply through Tauri's callback map. invoke itself is
  // intentionally immutable in native WebView2, so assignment is not a delay.
  await main.evaluate(`(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks;globalThis.pageNativeSet=callbacks.set;globalThis.nativePageReplies=0;const delayed=function(id,callback){return globalThis.pageNativeSet.call(this,id,result=>{if(result?.experiments?.some(set=>set.id==='native-pages'&&set.previewPageRevision===1)){globalThis.nativePageReplies++;globalThis.releasePageNative=()=>callback(result);}else callback(result);});};callbacks.set=delayed;if(callbacks.set!==delayed)throw new Error('Native page reply delay did not install');})()`);
  await pointer(use);await waitFor(`${picker}.saving&&globalThis.releasePageNative`);assert.equal(await main.evaluate(`${use}===${picker}.getRootNode().activeElement`),true);await key('Enter');
  assert.equal(await main.evaluate('globalThis.nativePageReplies'),1);await main.evaluate('delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releasePageNative()');await waitFor(`${shell}.traffic.previewPageDialog.open&&!${shell}.traffic.previewRulesBusy`);
  assert.equal(await main.evaluate(`${shell}.traffic.previewPageId`),await main.evaluate(`${a}.automationStatus.experiments[0].previewPage.entryId`));
  await pointer(shell+".traffic.previewPageDialog.querySelector('footer button')");await waitFor(`!${shell}.traffic.previewPageBusy&&${shell}.traffic.previewSceneAvailable`);
  const report=await invoke('captured_page_report',{label:await main.evaluate(`${shell}.traffic.previewReportLabel`)});assert.equal(report.url,'https://transmog.invalid/');assert.equal(report.variant.variantId,'baseline');assert.equal(report.scriptsEnabled,false);
  await main.evaluate(`${shell}.activateView('automation')`);const associated=await invoke('automation_status');assert.equal(associated.generation,before.generation);assert.deepEqual(associated.rules,before.rules);assert.deepEqual(associated.experiments[0].undo,before.experiments[0].undo);assert.deepEqual(associated.experiments[0].variants,before.experiments[0].variants);
  const additional=await fixture('new-page.har',['https://new.invalid/']);await main.evaluate(`${shell}.traffic.importPaths([${JSON.stringify(additional)}])`);await waitFor(`${shell}.traffic.totalMatched===4`);assert.equal(await main.evaluate(`${v}.pageUrl`),'https://transmog.invalid/');
  await pointer(change);await waitFor(`${dialog}.open`);assert.equal(await main.evaluate(`${picker}.selectedId`),'');await key('Escape');assert.deepEqual(await invoke('automation_status'),associated);
  await invoke('remove_traffic_entries',{ids:[associated.experiments[0].previewPage.entryId],restore:false});
  await main.evaluate(`${v}.refreshPage()`);await waitFor(`${v}.pageAvailability==='Retained preview capture'`);
  await pointer(preview);await waitFor(`${shell}.traffic.previewPageDialog.open`);assert.equal(await main.evaluate(`${shell}.traffic.previewReusesScene`),true);assert.equal(await main.evaluate(`${shell}.traffic.previewPageScope`),'original-trace');
  await pointer(shell+".traffic.previewPageDialog.querySelector('footer button.secondary')");await main.evaluate(`${shell}.activateView('automation')`);
  await pointer(change);await waitFor(`${dialog}.open`);const replacement=await main.evaluate(`${picker}.pages.findIndex(page=>page.url==='https://transmog.invalid/')`);await pointer(dialog+`.querySelectorAll('input')[${replacement}]`);
  await main.evaluate(`(()=>{const callbacks=window.__TAURI_INTERNALS__.callbacks;globalThis.pageNativeSet=callbacks.set;globalThis.releasePageNative=null;callbacks.set=function(id,callback){return globalThis.pageNativeSet.call(this,id,result=>{if(result?.experiments?.some(set=>set.id==='native-pages'&&set.previewPageRevision===2))globalThis.releasePageNative=()=>callback(result);else callback(result);});};})()`);
  await pointer(use);await waitFor(`${picker}.saving&&globalThis.releasePageNative`);await key('Escape');await main.evaluate('delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releasePageNative()');await waitFor(`!${picker}.saving`);
  assert.equal(await main.evaluate(`${shell}.activeView`),'automation');assert.equal(await main.evaluate(`${shell}.traffic.previewPageDialog.open`),false);
  const final=await invoke('automation_status');assert.equal(final.generation,before.generation);assert.deepEqual(final.experiments[0].undo,before.experiments[0].undo);assert.notEqual(final.experiments[0].previewPage.entryId,associated.experiments[0].previewPage.entryId);
  // A related run keeps its own frozen page even after the set chooses a different response.
  await main.evaluate(`(async()=>{await ${shell}.activateView('traffic');${shell}.traffic.showRelatedPreview('captured');})()`);assert.equal(await main.evaluate(`${shell}.traffic.previewPageId`),associated.experiments[0].previewPage.entryId);await pointer(shell+".traffic.previewPageDialog.querySelector('footer button.secondary')");await main.evaluate(`${shell}.activateView('automation')`);
  await main.call('Emulation.clearDeviceMetricsOverride');await writeFile(new URL('native-result.json',directory),JSON.stringify({before,associated,final,report,lifecycle:(await invoke('app_status')).lifecycle},null,2));
}
