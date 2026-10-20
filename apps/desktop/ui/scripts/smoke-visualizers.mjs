import {writeFile} from 'node:fs/promises';
import {smokeLiveVisualizers} from './smoke-live-visualizers.mjs';
import {smokeHttpVisualizers} from './smoke-http-visualizers.mjs';
import {smokeClassifications} from './smoke-classifications.mjs';

/** Real WebView2 -> native evidence broker -> AppContainer worker -> host renderer. */
export async function smokeVisualizers(call,evaluate,screenshotPath,tracePath) {
  await call('Emulation.setDeviceMetricsOverride',{width:1440,height:900,deviceScaleFactor:1,mobile:false});
  const result=await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),traffic=shell.traffic;
    globalThis.__visualizerProbe={preferences:traffic.preferences};
    shell.shadowRoot.querySelector('a[data-view="traffic"]').click();
    await traffic.importTrace(${JSON.stringify(tracePath)});
    await traffic.importTrace(${JSON.stringify(tracePath.replace(/\.saz$/i,'-other.saz'))});
    await (${smokeClassifications.toString()})(traffic);
    const row=traffic.sessions.find(row=>row.path==='/extension-fixture'&&row.responseBytes===23);if(!row)throw new Error('Native visualizer fixture did not import: '+traffic.importStatus);
    traffic.preferences={...traffic.preferences,layout:'stacked',listSplit:30};
    await traffic.selectTraffic(row,new MouseEvent('click'));
    const wait=async(condition)=>{const deadline=Date.now()+20000;while(!condition()){if(Date.now()>deadline)throw new Error('Native visualizer did not settle');await new Promise(resolve=>setTimeout(resolve,25));}};
    const root=traffic.shadowRoot??traffic,response=root.querySelector('message-inspector[side=response]'),request=root.querySelector('message-inspector[side=request]');
    await wait(()=>response.detail?.id===row.id&&response.bodyChoices.some(choice=>choice.visualizerId==='native-body'));
    const choose=(inspector,label,name)=>{const local=inspector.shadowRoot??inspector,select=local.querySelector('select[aria-label="'+label+'"]'),option=[...select.options].find(option=>option.textContent===name);if(!option)throw new Error('Native viewer choice missing: '+name);select.value=option.value;select.dispatchEvent(new Event('change',{bubbles:true}));};
    const child=inspector=>(inspector.shadowRoot??inspector).querySelector('extension-visualizer[slot$="body"]');
    response.showMode('body');choose(response,'Body viewer','Native protocol');
    await wait(()=>child(response)?.view||child(response)?.visualizerError);
    const viewer=child(response);if(viewer.visualizerError)throw new Error(viewer.visualizerError);
    if(viewer.view.document.roots[0]?.value?.value!=='Synthetic body evidence')throw new Error('The native byte continuation did not reach the parser');
    const handle=viewer.view.handle;viewer.clickNode('body-text');
    const contextView=await window.__TAURI_INTERNALS__.invoke('activate_visualizer',{request:{slot:'native-context-probe',generation:'1',pageId:handle.pageId,selection:{sessionId:row.id,boundary:'client-response',surface:'body'},extensionId:'test.development',visualizerId:'native-context'}});
    if(contextView.document.roots[0]?.value?.value!=='1')throw new Error('Native same-origin context did not reach the sandbox parser');
    await window.__TAURI_INTERNALS__.invoke('deactivate_visualizer',{slot:'native-context-probe',generation:'1',pageId:handle.pageId});
    await viewer.toggleNode('body-text');
    if(viewer.view.document.roots[0].children[0]?.value?.value!=='9007199254740993')throw new Error('Native deferred expansion lost integer precision');
    viewer.selectNode('body-text');viewer.showSpan(viewer.view.document.roots[0].spans[0]);
    await wait(()=>{const local=viewer.shadowRoot??viewer;return !!local.querySelector('hex-viewer')?.hasSelection;});
    const bytes=await window.__TAURI_INTERNALS__.invoke('visualizer_source',{handle,request:{sourceId:viewer.view.document.roots[0].spans[0].sourceId,offset:'0',maxBytes:8}});
    if(atob(bytes.bytesBase64)!=='Syntheti')throw new Error('Native exact source read returned different evidence');
    let denied=false;try {await window.__TAURI_INTERNALS__.invoke('visualizer_source',{handle,request:{sourceId:'ungranted-body',offset:'0',maxBytes:1}});}catch {denied=true;}
    if(!denied)throw new Error('Native UI source reads escaped the active grant');
    response.showMode('split');choose(response,'Headers viewer','Native fields');choose(request,'Headers viewer','Native fields');
    const children=()=>[response,request].flatMap(inspector=>[...(inspector.shadowRoot??inspector).querySelectorAll('extension-visualizer')]);
    await wait(()=>children().filter(view=>view.view).length===3);
    const viewers=children();
    for(const view of viewers.filter(view=>view.view?.instance.visualizerId==='native-headers'))if(view.view.sources.some(source=>source.name.endsWith(' · body')))throw new Error('Header-only native visualizer acquired a body source');
    globalThis.__visualizerProbe.handles=viewers.map(view=>view.view.handle);
    const classifications=globalThis.__nativeClassificationVerified;delete globalThis.__nativeClassificationVerified;
    return {workers:viewers.length,byteContinuation:true,sameOriginContext:true,sameCaptureContext:true,expansion:true,sourceDenied:denied,classifications};
  })()`);
  await call('Emulation.setDeviceMetricsOverride',{width:1440,height:900,deviceScaleFactor:1,mobile:false});
  if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-visualizer-wide.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
  await call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:1,mobile:false});
  if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-visualizer-compact.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
  await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="settings"]').click();shell.settings.showSettingsSection('extensions');
    const deadline=Date.now()+10000;
    for(;;){const active=await Promise.all(globalThis.__visualizerProbe.handles.map(handle=>window.__TAURI_INTERNALS__.invoke('visualizer_active',{handle})));if(active.every(value=>!value))break;if(Date.now()>deadline)throw new Error('Hidden native visualizers were not unloaded');await new Promise(resolve=>setTimeout(resolve,25));}
    const root=shell.traffic.shadowRoot??shell.traffic;
    const children=[...root.querySelectorAll('message-inspector')].flatMap(inspector=>[...(inspector.shadowRoot??inspector).querySelectorAll('extension-visualizer')]);
    if(children.some(view=>view.view||view.visualizerHex||view.visualizerRows.length))throw new Error('Hidden native visualizer retained captured data');
    shell.traffic.preferences=globalThis.__visualizerProbe.preferences;delete globalThis.__visualizerProbe;
  })()`);
  await call('Emulation.clearDeviceMetricsOverride');
  const live=await smokeLiveVisualizers(call,evaluate,screenshotPath);
  const http=await smokeHttpVisualizers(call,evaluate,screenshotPath);
  return {...result,unloaded:true,live,http};
}
