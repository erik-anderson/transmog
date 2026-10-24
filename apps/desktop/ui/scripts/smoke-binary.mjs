import { writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

/** Actual packaged modules, isolated workers, retained HTTP decoding and host rendering. */
export async function smokeBinary(call, evaluate, screenshotPath, tracePath) {
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
    globalThis.__binaryProbe={preferences:traffic.preferences,handles:[]};
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
      const inspector=inspectors[side],protocol=name.startsWith('protobuf')?'protobuf':'bond';
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
      }else if(name.endsWith('-negative-zero')){
        if(doc.status!=='complete'||nodes.filter(node=>node.value?.kind==='string'&&node.value.value==='-0').length!==2)throw new Error('Binary float sign was lost at the native JSON boundary');
      }else if(name==='protobuf-duplicate-identity'||name==='protobuf-invalid-identity'){
        if(doc.status!=='unavailable'||doc.roots.length||!doc.diagnostics.some(item=>item.message.includes('metadataUnavailable')))throw new Error('Invalid identity bypassed host content-coding validation');
      }else if(name==='bond-version-limited'||name==='bond-framing-limited'){
        if(doc.status!=='limited'||!doc.diagnostics.some(item=>item.code===(name==='bond-version-limited'?'version-limited':'framing-limited')))throw new Error('A parser limit established an unresolved Bond interpretation');
      }else if(name==='bond-ambiguous'||name==='bond-framing'){
        if(doc.status!=='partial'||!doc.diagnostics.some(item=>item.code===(name==='bond-ambiguous'?'ambiguous-version':'ambiguous-framing')))throw new Error('Ambiguous Bond interpretation was presented as established');
      }else{
        if(doc.status!=='complete')throw new Error('Binary parse failed: '+JSON.stringify(doc.diagnostics));
        if(protocol==='protobuf'&&!nodes.some(node=>node.name==='Possible UTF-8 text'&&node.value?.value==='hello'))throw new Error('Schema-free Protobuf text or ambiguity label missing');
        if(protocol==='bond'&&!nodes.some(node=>node.name==='Field 1 · string'&&node.value?.value==='3.0'))throw new Error('Bond typed string missing');
      }
      if(name.includes('gzip')||name.includes('deflate'))if(!viewer.view.sources.some(source=>source.httpDecoded?.codings.length))throw new Error('Binary content decoding lacks provenance');
      for(const node of doc.roots)if(node.children?.length)await viewer.toggleNode(node.id);
      return viewer;
    }
    await show('protobuf','request');await show('protobuf-gzip','response');
    await show('bond','request');await show('bond-deflate','response');
    await show('bond-v2','response',false);await show('protobuf-malformed','request',false);
    await show('bond-ambiguous','response',false);await show('bond-framing','response',false);
    await show('protobuf-negative-zero','request',false);await show('bond-negative-zero','response',false);
    await show('protobuf-duplicate-identity','request',false);await show('protobuf-invalid-identity','response',false);
    await show('bond-version-limited','request',false);await show('bond-framing-limited','response',false);
    await show('protobuf','request',false);await show('bond','response',false);
    globalThis.__binaryProbe.inspectors=inspectors;
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
    shell.shadowRoot.querySelector('a[data-view="settings"]').click();
    const end=Date.now()+10000;
    while((await Promise.all(probe.handles.map(handle=>window.__TAURI_INTERNALS__.invoke('visualizer_active',{handle})))).some(Boolean)){if(Date.now()>end)throw new Error('Hidden binary workers remained active');await new Promise(resolve=>setTimeout(resolve,25));}
    for(const inspector of Object.values(probe.inspectors))if([...(inspector.shadowRoot??inspector).querySelectorAll('extension-visualizer')].some(view=>view.view||view.visualizerRows.length))throw new Error('Hidden binary viewer retained captured data');
    shell.traffic.preferences=probe.preferences;delete globalThis.__binaryProbe;
  })()`);
  await call("Emulation.clearDeviceMetricsOverride");
  return { bundled: true, schemas: false, httpDecoded: true, malformed: true, unloaded: true };
}
