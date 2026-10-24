import { writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import {
  referenceCases,
  startReferenceService,
} from "../../../../scripts/binary-reference-fixtures.mjs";

/** Actual packaged modules, isolated workers, retained HTTP decoding and host rendering. */
async function smokePackagedBinary(call, evaluate, screenshotPath, tracePath, serviceUrl) {
  const path = join(dirname(tracePath), "binary-fixture.saz");
  await call("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),traffic=shell.traffic,invoke=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const wait=async(test,label)=>{const end=Date.now()+30000;while(!test()){if(Date.now()>end)throw new Error('Bundled binary viewer did not settle: '+label);await new Promise(resolve=>setTimeout(resolve,25));}};
    globalThis.__binaryProbe={preferences:traffic.preferences,inspectorSide:traffic.inspectorSide,handles:[]};
    shell.shadowRoot.querySelector('a[data-view="traffic"]').click();
    await traffic.importTrace(${JSON.stringify(path)});
    traffic.preferences={...traffic.preferences,layout:'stacked',listSplit:30};
    const root=traffic.shadowRoot??traffic;
    const inspectors={request:root.querySelector('message-inspector[side=request]'),response:root.querySelector('message-inspector[side=response]')};
    const local=element=>element.shadowRoot??element;
    const child=inspector=>local(inspector).querySelector('extension-visualizer[slot$="body"]');
    const walk=node=>[node,...(node.children??[]).flatMap(walk)];
    async function show(name,side,automatic=true){
      const row=(await invoke('query_sessions',{query:{search:'https://binary.invalid/'+name,limit:100,offset:0}})).sessions.find(row=>row.url==='https://binary.invalid/'+name);
      if(!row)throw new Error('Binary fixture missing: '+name);
      await traffic.revealSession(row.id);
      const inspector=inspectors[side],protocol=name.split('-')[0];
      await wait(()=>inspector.detail?.id===row.id&&inspector.bodyChoices.some(choice=>choice.extensionId==='net.transmog.binary'),name+' '+side+' choices');
      inspector.showMode('body');
      const select=local(inspector).querySelector('select[aria-label="Body viewer"]');
      select.value=automatic?'auto':'extension:net.transmog.binary:'+protocol+'-'+side;select.dispatchEvent(new Event('change',{bubbles:true}));
      await wait(()=>child(inspector)?.visualizerError||child(inspector)?.view&&child(inspector).view.instance.extensionId==='net.transmog.binary'&&!child(inspector).visualizerBusy,name+' '+side+' result');
      const viewer=child(inspector);if(viewer.visualizerError)throw new Error(viewer.visualizerError);
      const doc=viewer.view.document,nodes=doc.roots.flatMap(walk);
      if(viewer.view.instance.development||viewer.view.live.available||viewer.view.live.enabled||doc.updates!=='final')throw new Error('Binary viewer bypassed the bundled snapshot contract');
      globalThis.__binaryProbe.handles.push(viewer.view.handle);
      if(name==='protobuf-malformed'){
        if(doc.status!=='malformed'||!nodes.some(node=>node.name==='Unparsed bytes'))throw new Error('Malformed Protobuf lost partial evidence');
      }else if(name==='protobuf-node-limit'){
        const group=nodes.find(node=>node.name==='Partial Field 1 · group');
        if(doc.status!=='limited'||group?.children?.length!==2000)throw new Error('Parent display limit discarded completed group fields');
      }else if(name.endsWith('-negative-zero')){
        if(doc.status!=='complete'||nodes.filter(node=>node.value?.kind==='string'&&node.value.value==='-0').length!==2)throw new Error('Binary float sign was lost at the native JSON boundary');
      }else if(name==='protobuf-duplicate-identity'||name==='protobuf-invalid-identity'){
        if(doc.status!=='unavailable'||doc.roots.length||!doc.diagnostics.some(item=>item.message.includes('metadataUnavailable')))throw new Error('Invalid identity bypassed host content-coding validation');
      }else if(name==='bond-version-limited'||name==='bond-framing-limited'){
        if(doc.status!=='partial'||!doc.diagnostics.some(item=>item.code===(name==='bond-version-limited'?'ambiguous-version':'ambiguous-framing')))throw new Error('Indexed Bond lost an unresolved interpretation');
      }else if(name==='bond-record-ranges'||name==='bond-deep-structure'){
        if(doc.status!=='complete'||!nodes.some(node=>node.childrenToken))throw new Error('Large Bond records were not indexed into deferred ranges');
      }else if(name==='bond-large-string'){
        if(doc.status!=='complete'||!nodes.some(node=>node.name.includes('preview')&&node.value?.value.length===1024))throw new Error('Large Bond string preview missing');
      }else if(name==='bond-sequence-deflate'){
        if(doc.status!=='complete'||doc.roots.length!==4||doc.roots.some((node,index)=>node.spans[0].offset!==String(index*6)||node.spans[0].length!=='6'))throw new Error('Consecutive Bond struct boundaries missing after raw DEFLATE');
      }else if(name==='bond-ambiguous'||name==='bond-framing'){
        if(doc.status!=='partial'||!doc.diagnostics.some(item=>item.code===(name==='bond-ambiguous'?'ambiguous-version':'ambiguous-framing')))throw new Error('Ambiguous Bond interpretation was presented as established');
      }else{
        if(doc.status!=='complete')throw new Error('Binary parse failed: '+JSON.stringify(doc.diagnostics));
        if(protocol==='protobuf'&&name!=='protobuf-packed'&&!nodes.some(node=>node.name==='Possible UTF-8 text'&&node.value?.value==='hello'))throw new Error('Schema-free Protobuf text or ambiguity label missing');
        if(name==='protobuf-packed'&&!nodes.some(node=>node.name==='Possible packed ZigZag integers'))throw new Error('Packed Protobuf previews missing');
        if(protocol==='bond'&&!nodes.some(node=>node.name==='Field 1 · string'&&node.value?.value==='3.0'))throw new Error('Bond typed string missing');
        if(['msgpack','cbor','bson'].includes(name)&&!nodes.some(node=>node.value?.value==='hello'))throw new Error('Independent serializer value missing');
      }
      if(name.includes('gzip')||name.includes('deflate'))if(!viewer.view.sources.some(source=>source.httpDecoded?.codings.length))throw new Error('Binary content decoding lacks provenance');
      for(const node of doc.roots)if(node.children?.length)await viewer.toggleNode(node.id);
      return viewer;
    }
    await show('protobuf','request');await show('protobuf-gzip','response');
    await show('bond','request');await show('bond-deflate','response');
    await show('bond-v2','response',false);await show('protobuf-malformed','request',false);
    await show('protobuf-node-limit','response',false);
    await show('bond-ambiguous','response',false);await show('bond-framing','response',false);
    await show('protobuf-negative-zero','request',false);await show('bond-negative-zero','response',false);
    await show('protobuf-duplicate-identity','request',false);await show('protobuf-invalid-identity','response',false);
    await show('bond-version-limited','request',false);await show('bond-framing-limited','response',false);
    await show('bond-sequence-deflate','request');await show('bond-large-string','response');
    const deep=await show('bond-deep-structure','request');
    for(let count=0;count<20;count++){
      const all=deep.view.document.roots.flatMap(walk),last=all.find(node=>node.name.includes('Field 1 · uint8')&&node.value?.value==='42');
      if(last){if(!last.name.includes('Depth 64'))throw new Error('Flattened deep Bond field lost its nesting depth');break;}
      const pending=all.find(node=>node.childrenToken&&node.spans.some(span=>Number(span.offset)<=68&&Number(span.offset)+Number(span.length)>68));
      if(!pending)throw new Error('No range reaches deep Bond value');await deep.toggleNode(pending.id);
      if(count===19)throw new Error('Deep Bond expansion made no progress');
    }
    for(const name of ['msgpack','msgpack-extension','cbor','cbor-seq-empty','bson','protobuf-packed'])await show(name,'response');
    await show('protobuf','request',false);await show('bond','response',false);
    const ranged=await show('bond-record-ranges','request');
    for(const offset of [0,149997])for(let count=0;count<20;count++){
      const all=ranged.view.document.roots.flatMap(walk),last=all.find(node=>node.name==='Field 1 · uint8'&&node.spans[0].offset===String(offset));
      if(last){if(last.value.value!=='7')throw new Error('Bond record value changed');ranged.selectNode(last.id,offset===149997);break;}
      const pending=all.find(node=>node.childrenToken&&node.spans.some(span=>Number(span.offset)<=offset&&Number(span.offset)+Number(span.length)>offset));
      if(!pending)throw new Error('No deferred range leads to Bond record');
      const previous=ranged.visualizerRows.find(row=>row.selected)?.id;
      await ranged.toggleNode(pending.id);
      if(offset&&ranged.visualizerDetailsOpen)throw new Error('Paging reopened closed Bond details');
      if(previous&&previous!=='$document'&&!ranged.view.document.roots.flatMap(walk).some(node=>node.id===previous)&&ranged.visualizerRows.find(row=>row.selected)?.id!==pending.id)throw new Error('Paging lost the explicit expansion target');
      if(count===19)throw new Error('Bond range expansion made no progress');
    }
    if(ranged.view.document.roots.flatMap(walk).length>1600)throw new Error('Bond range projection grew without bound');
    globalThis.__binaryProbe.ranged=ranged;
    traffic.showInspectorSide('request');
    globalThis.__binaryProbe.inspectors=inspectors;
  })()`);
  for (const [width, height] of [
    [1440, 900],
    [960, 720],
    [800, 600],
  ]) {
    await call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await evaluate(
      `(()=>{const viewer=globalThis.__binaryProbe.ranged,root=viewer.shadowRoot??viewer;if(root.querySelectorAll('[role=treeitem]').length>100)throw new Error('Structured tree stopped virtualizing rendered rows');if(!viewer.visualizerDetailsOpen)throw new Error('Selected Bond field details missing');})()`,
    );
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, `-bond-ranges-${width}.png`),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
  }
  await call("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });
  const qualification = await evaluate(`(async()=>{
    const invoke=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args),results=[];
    for(const sample of ${JSON.stringify(referenceCases.filter((item) => ["bond-reference", "msgpack", "cbor", "bson", "protobuf-packed"].includes(item.name)))}){
      const response=await invoke('execute_composer',{request:{method:'GET',url:${JSON.stringify(serviceUrl)}+'/'+sample.name,headers:[],body:''}});
      if(response.status!==200)throw new Error('Local reference service failed');
      const pageId=await invoke('visualizer_page'),format=sample.name.split('-')[0],slot='binary-service-'+sample.name;
      const view=await invoke('activate_visualizer',{request:{slot,generation:'1',pageId,selection:{sessionId:response.detail.id,boundary:'client-response',surface:'body'},extensionId:'net.transmog.binary',visualizerId:format+'-response'}});
      if(view.document.status!=='complete')throw new Error('Live service parse failed: '+JSON.stringify(view.document.diagnostics));
      const source=view.sources.find(source=>source.httpDecoded?.codings.includes('gzip'));
      if(!source)throw new Error('Live service decoding provenance missing');
      const page=await invoke('visualizer_source',{handle:view.handle,request:{sourceId:source.id,offset:'0',maxBytes:Number(source.length)}});
      const actual=Array.from(atob(page.bytesBase64),c=>c.charCodeAt(0).toString(16).padStart(2,'0')).join('');
      if(actual!==sample.bodyHex)throw new Error('Retained service bytes differ from independent encoder');
      await invoke('deactivate_visualizer',{slot,generation:'2',pageId});
      if(await invoke('visualizer_active',{handle:view.handle}))throw new Error('Service reader remained active');
      results.push({format,status:view.document.status,decodedBytes:Number(source.length),transport:'live localhost HTTP with gzip',encoder:format==='bond'?'Microsoft reference wire example':'independent reference library'});
    }
    const shell=document.querySelector('app-shell'),c=shell.comparison;
    const find=async name=>(await invoke('query_sessions',{query:{search:'https://binary.invalid/'+name,limit:100,offset:0}})).sessions.find(row=>row.url==='https://binary.invalid/'+name).id;
    const left=await find('msgpack'),right=await find('msgpack-right');
    await c.open({left:{kind:'exchange',id:left},right:{kind:'exchange',id:right}});
    c.querySelector('.comparison-fields').open=true;c.comparisonFieldsOpen=true;c.comparisonFieldsFormat='msgpack';
    await c.compareFields();
    if(!c.comparisonFields||c.comparisonFields.document.status!=='complete'||!c.comparisonFields.document.roots.some(node=>node.name.includes('greeting')))throw new Error('Native field comparison missing changed fields: '+c.comparisonFieldsStatus);
    const wait=async()=>{const end=Date.now()+10000;while(!c.querySelector('extension-visualizer')?.visualizerTitle){if(Date.now()>end)throw new Error('Native document did not reach shared presentation');await new Promise(resolve=>setTimeout(resolve,25));}};await wait();
    const shared=c.querySelector('extension-visualizer');if(shared.view||shared.choice||!shared.snapshot)throw new Error('Native document fabricated an extension instance');
    if(!shared.querySelector('.visualizer-outcome')?.textContent.includes('field findings'))throw new Error('Native field counts are hidden');
    if(shared.querySelector('.visualizer-node option[value=hex]'))throw new Error('Native result offered Hex without sources');
    for(const node of c.comparisonFields.document.roots)await shared.toggleNode(node.id);
    for(const format of ['cbor','bson']){
      const a=await find(format),b=await find(format+'-right');
      const snapshot=await invoke('compare_evidence',{input:{operationId:'binary-compare-'+format,left:{kind:'exchange',id:a},right:{kind:'exchange',id:b}}});
      const fields=await invoke('comparison_fields',{id:snapshot.id,section:'Response',format,pageId:await invoke('visualizer_page'),operationId:'binary-fields-'+format});
      if(fields.document.status!=='complete'||!fields.document.roots.some(node=>node.name.includes('greeting')))throw new Error('Native '+format+' comparison failed');
      if(await invoke('comparison_fields',{id:snapshot.id,section:'Response',format,pageId:'0',operationId:'binary-stale-'+format}).then(()=>false,()=>true)!==true)throw new Error('Expired page started a native field reader');
      await invoke('cancel_comparison',{operationId:'binary-queued-'+format});
      if(await invoke('comparison_fields',{id:snapshot.id,section:'Response',format,pageId:await invoke('visualizer_page'),operationId:'binary-queued-'+format}).then(()=>false,()=>true)!==true)throw new Error('Pre-start field cancellation was forgotten');
      await invoke('close_comparison',{id:snapshot.id});
    }
    const empty=await find('cbor-seq-empty'),emptyPair=await invoke('compare_evidence',{input:{operationId:'empty-cbor-pair',left:{kind:'exchange',id:empty},right:{kind:'exchange',id:empty}}});
    const emptyFields=await invoke('comparison_fields',{id:emptyPair.id,section:'Response',format:'cbor',pageId:await invoke('visualizer_page'),operationId:'empty-cbor-fields'});
    if(emptyFields.document.status!=='complete'||emptyFields.document.roots[0]?.name!=='No field changes')throw new Error('Empty CBOR sequence comparison lost its content type');
    await invoke('close_comparison',{id:emptyPair.id});
    return results;
  })()`);
  for (const [name, width, height] of [
    ["wide", 1440, 900],
    ["compact", 800, 600],
  ]) {
    await call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await evaluate(
      `(()=>{const c=document.querySelector('app-shell').comparison;c.comparisonDialog.querySelector('.comparison-field-view').scrollIntoView({block:'center'});c.comparisonDialog.querySelector('extension-visualizer').treeViewport.focus();})()`,
    );
    await call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "End",
      code: "End",
      windowsVirtualKeyCode: 35,
    });
    await call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "End",
      code: "End",
      windowsVirtualKeyCode: 35,
    });
    await evaluate(
      `(()=>{const c=document.querySelector('app-shell').comparison,viewer=c.comparisonDialog.querySelector('extension-visualizer'),viewport=c.comparisonScroll.getBoundingClientRect(),tree=viewer.treeViewport.getBoundingClientRect(),selected=viewer.treeViewport.querySelector('[aria-selected=true]').getBoundingClientRect();if(tree.top<viewport.top-2||tree.bottom>viewport.bottom+2||selected.top<viewport.top-2||selected.bottom>viewport.bottom+2)throw new Error('Native field tree or keyboard selection escaped its visible viewport');if(viewer.visualizerDetailsOpen)throw new Error('Keyboard browsing opened native details without an explicit action');})()`,
    );
    await call("Input.dispatchKeyEvent", {
      type: "keyDown",
      key: "Home",
      code: "Home",
      windowsVirtualKeyCode: 36,
    });
    await call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "Home",
      code: "Home",
      windowsVirtualKeyCode: 36,
    });
    if (screenshotPath)
      await writeFile(
        screenshotPath.replace(/\.png$/i, `-binary-${name}.png`),
        Buffer.from(
          (await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false }))
            .data,
          "base64",
        ),
      );
  }
  await evaluate(`(async()=>{
    const shell=document.querySelector('app-shell'),probe=globalThis.__binaryProbe;
    shell.comparison.closeComparison();
    shell.shadowRoot.querySelector('a[data-view="settings"]').click();
    const end=Date.now()+10000;
    while((await Promise.all(probe.handles.map(handle=>window.__TAURI_INTERNALS__.invoke('visualizer_active',{handle})))).some(Boolean)){if(Date.now()>end)throw new Error('Hidden binary workers remained active');await new Promise(resolve=>setTimeout(resolve,25));}
    for(const inspector of Object.values(probe.inspectors))if([...(inspector.shadowRoot??inspector).querySelectorAll('extension-visualizer')].some(view=>view.view||view.visualizerRows.length))throw new Error('Hidden binary viewer retained captured data');
    shell.traffic.preferences=probe.preferences;shell.traffic.showInspectorSide(probe.inspectorSide);delete globalThis.__binaryProbe;
  })()`);
  await call("Emulation.clearDeviceMetricsOverride");
  if (screenshotPath)
    await writeFile(
      screenshotPath.replace(/\.png$/i, "-binary-qualification.json"),
      JSON.stringify(
        {
          localService: qualification,
          productionServiceCaptures: "not supplied",
          nativeComparison: true,
          sharedPresentation: true,
          viewportAndKeyboard: true,
          stalePageRejected: true,
          queuedCancellation: true,
          emptyCborSequence: true,
          nodeLimitRetention: true,
          bondRawSequence: true,
          bondLargeString: true,
          bondRecordRanges: true,
        },
        null,
        2,
      ),
    );
  return {
    bundled: true,
    schemas: false,
    httpDecoded: true,
    malformed: true,
    unloaded: true,
    localService: qualification,
  };
}

export async function smokeBinary(call, evaluate, screenshotPath, tracePath) {
  const service = await startReferenceService();
  try {
    return await smokePackagedBinary(call, evaluate, screenshotPath, tracePath, service.url);
  } finally {
    await service.close();
  }
}
