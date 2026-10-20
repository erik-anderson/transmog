import {readFile,writeFile} from 'node:fs/promises';
import {dirname,join} from 'node:path';

/** Synthetic binary evidence through the bundled ordinary tmext and real host UI. */
export async function smokeMapi(call,evaluate,screenshotPath,tracePath) {
  const path=join(dirname(tracePath),'mapi-fixture.saz');
  const specification=JSON.parse(await readFile(path.replace(/\.saz$/i,'.json'),'utf8'));
  await call('Emulation.setDeviceMetricsOverride',{width:1440,height:900,deviceScaleFactor:1,mobile:false});
  const result=await evaluate(`(async()=>{
    const specification=${JSON.stringify(specification)},shell=document.querySelector('app-shell'),traffic=shell.traffic,invoke=(command,args={})=>window.__TAURI_INTERNALS__.invoke(command,args);
    const wait=async(condition)=>{const deadline=Date.now()+30000;while(!condition()){if(Date.now()>deadline)throw new Error('Bundled MAPI did not settle');await new Promise(resolve=>setTimeout(resolve,25));}};
    globalThis.__mapiProbe={preferences:traffic.preferences,handles:[]};
    shell.shadowRoot.querySelector('a[data-view="traffic"]').click();
    await traffic.importTrace(${JSON.stringify(path.replace(/\.saz$/i,'-other.saz'))});
    await traffic.importTrace(${JSON.stringify(path)});
    traffic.preferences={...traffic.preferences,layout:'stacked',listSplit:30};
    const root=traffic.shadowRoot??traffic,response=root.querySelector('message-inspector[side=response]'),request=root.querySelector('message-inspector[side=request]');
    const choose=(inspector,label,value)=>{const local=inspector.shadowRoot??inspector,select=local.querySelector('select[aria-label="'+label+'"]');if(![...select.options].some(option=>option.value===value))throw new Error('Bundled MAPI viewer is not applicable: '+value);select.value=value;select.dispatchEvent(new Event('change',{bubbles:true}));};
    const child=(inspector,side='body')=>(inspector.shadowRoot??inspector).querySelector('extension-visualizer[slot$="'+side+'"]');
    function fields(document){const walk=node=>[node,...(node.children??[]).flatMap(walk)];return document.roots.flatMap(walk);}
    async function expand(viewer){for(let count=0;count<256;count++){const next=fields(viewer.view.document).find(node=>node.childrenToken);if(!next)return;await viewer.toggleNode(next.id);}throw new Error('MAPI expansion was not bounded');}
    async function show(name,direction='response'){
      const expected=specification.cases.find(item=>item.name===name),row=traffic.sessions.find(row=>row.requestBytes===expected.requestBytes&&row.responseBytes===expected.responseBytes&&row.host===expected.host);
      if(!row)throw new Error('Synthetic MAPI exchange missing: '+name+' '+traffic.importStatus);
      await traffic.selectTraffic(row,new MouseEvent('click'));
      const inspector=direction==='request'?request:response;
      await wait(()=>inspector.detail?.id===row.id&&inspector.bodyChoices.some(choice=>choice.extensionId==='com.transmog.mapi'));
      inspector.showMode('body');choose(inspector,'Body viewer','extension:com.transmog.mapi:body-'+direction);
      await wait(()=>child(inspector)?.view||child(inspector)?.visualizerError);
      const viewer=child(inspector);if(viewer.visualizerError)throw new Error(viewer.visualizerError);
      if(viewer.view.instance.development||viewer.view.instance.extensionId!=='com.transmog.mapi')throw new Error('MAPI did not use its bundled package');
      await expand(viewer);
      if(viewer.view.document.status!=='complete')throw new Error('Synthetic MAPI did not decode completely: '+name+' '+JSON.stringify(viewer.view.document.diagnostics));
      return {viewer,row};
    }
    let {viewer}=await show('properties');
    let values=fields(viewer.view.document).map(node=>node.value?.value);
    if(!values.includes('9007199254740993')||!values.includes('132537600000000000')||!values.includes('2020-12-30T00:00:00.0000000Z'))throw new Error('Bundled MAPI lost exact property values');
    ({viewer}=await show('transformed'));
    if(!viewer.view.document.sources.some(source=>source.exactMapping===false))throw new Error('Transformed RPC lacks decoded evidence');
    ({viewer}=await show('http-gzip'));
    if(!viewer.view.sources.some(source=>source.httpDecoded?.codings.includes('gzip')))throw new Error('HTTP gzip lacks decoded evidence');
    ({viewer}=await show('rows'));
    values=fields(viewer.view.document).map(node=>node.value?.value);
    if(!values.includes('9007199254740993')||!values.includes('Synthetic café Ω'))throw new Error('MAPI table context crossed origin/capture scope or lost its columns');
    const rowNode=fields(viewer.view.document).find(node=>node.value?.value==='9007199254740993');
    viewer.selectNode(rowNode.id);viewer.showSpan(rowNode.spans[0]);
    await wait(()=>!!(viewer.shadowRoot??viewer).querySelector('hex-viewer')?.hasSelection);
    const detailRoot=viewer.shadowRoot??viewer;
    detailRoot.querySelector('button[aria-label="Close structure details"]').click();
    await wait(()=>detailRoot.querySelector('.visualizer-node').hidden);
    const focusedTree=detailRoot.querySelector('.visualizer-tree');
    if(!detailRoot.querySelector('.visualizer-node').hidden||focusedTree!==focusedTree.getRootNode().activeElement)throw new Error('Closing MAPI details did not restore tree space and focus');
    viewer.selectNode(rowNode.id);viewer.showSpan(rowNode.spans[0]);
    await wait(()=>!!(viewer.shadowRoot??viewer).querySelector('hex-viewer')?.hasSelection);
    const byteViewer=detailRoot.querySelector('hex-viewer'),byteGrid=(byteViewer.shadowRoot??byteViewer).querySelector('[role="grid"]');
    byteGrid.focus();byteGrid.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,composed:true,cancelable:true}));
    await wait(()=>!byteViewer.hasSelection);
    if(!viewer.visualizerDetailsOpen)throw new Error('Clearing MAPI byte selection closed structure details');
    viewer.showSpan(rowNode.spans[0]);
    const rowHandle=viewer.view.handle;
    response.showMode('split');request.showMode('split');
    choose(response,'Headers viewer','extension:com.transmog.mapi:headers');choose(request,'Headers viewer','extension:com.transmog.mapi:headers');choose(request,'Body viewer','extension:com.transmog.mapi:body-request');
    const views=()=>[request,response].flatMap(inspector=>[...(inspector.shadowRoot??inspector).querySelectorAll('extension-visualizer')]);
    await wait(()=>views().filter(view=>view.view).length===4);
    if(views().some(view=>view.visualizerError))throw new Error('Concurrent MAPI contributions failed');
    for(const view of views().filter(view=>view.view.instance.visualizerId==='headers'))if(view.view.sources.some(source=>source.name.endsWith(' · body')))throw new Error('MAPI headers acquired body access');
    for(const view of views().filter(view=>!view.visualizerDetailsOpen)){
      const local=view.shadowRoot??view,tree=local.querySelector('.visualizer-tree'),layout=local.querySelector('.visualizer-layout');
      if(tree.getBoundingClientRect().width<layout.getBoundingClientRect().width-2)throw new Error('An unselected MAPI structure reserved an empty detail pane');
    }
    const infoButton=detailRoot.querySelector('button[aria-label="About this visualizer"]'),infoPanel=detailRoot.querySelector('.visualizer-about');
    const inspectionHeight=detailRoot.querySelector('.visualizer-layout').getBoundingClientRect().height;
    infoButton.click();await wait(()=>infoPanel.matches(':popover-open'));
    if(detailRoot.querySelector('.visualizer-layout').getBoundingClientRect().height!==inspectionHeight)throw new Error('MAPI information consumed inspection space');
    infoPanel.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,composed:true,cancelable:true}));
    await wait(()=>!infoPanel.matches(':popover-open'));
    if(infoButton!==infoButton.getRootNode().activeElement)throw new Error('Closing MAPI information lost trigger focus');

    globalThis.__mapiProbe.handles=views().map(view=>view.view.handle);
    const columns=await invoke('classify_traffic_rows',{ids:[traffic.activeId??response.detail.id],columns:['com.transmog.mapi:protocol']});
    if(!columns.some(row=>row.values['com.transmog.mapi:protocol']?.[0]==='MAPI/HTTP'))throw new Error('Bundled MAPI static classification missing');
    globalThis.__mapiProbe.concurrent=true;
    return {properties:true,utcTicks:true,transformedRpc:true,httpGzip:true,retainedRows:true,scopedContext:true,byteSelection:true,concurrent:4,headersReadOnly:true,staticClassification:true,compactDetails:true,compactInformation:true,initialHandle:rowHandle};
  })()`);
  if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-mapi-wide.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
  await call('Emulation.setDeviceMetricsOverride',{width:800,height:600,deviceScaleFactor:1,mobile:false});
  if(screenshotPath)await writeFile(screenshotPath.replace(/\.png$/i,'-mapi-compact.png'),Buffer.from((await call('Page.captureScreenshot',{format:'png',captureBeyondViewport:false})).data,'base64'));
  const unloaded=await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell');shell.shadowRoot.querySelector('a[data-view="settings"]').click();shell.settings.showSettingsSection('extensions');
    const deadline=Date.now()+10000;
    while((await Promise.all(globalThis.__mapiProbe.handles.map(handle=>window.__TAURI_INTERNALS__.invoke('visualizer_active',{handle})))).some(Boolean)){if(Date.now()>deadline)throw new Error('Inactive MAPI workers were not unloaded');await new Promise(resolve=>setTimeout(resolve,25));}
    shell.traffic.preferences=globalThis.__mapiProbe.preferences;delete globalThis.__mapiProbe;return true;
  })()`);
  await call('Emulation.clearDeviceMetricsOverride');
  return {...result,unloaded};
}
