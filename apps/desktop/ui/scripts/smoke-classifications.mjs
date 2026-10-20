/** Runs inside the native WebView before any visualizer is activated. */
export async function smokeClassifications(traffic) {
  const invoke=(command,args)=>window.__TAURI_INTERNALS__.invoke(command,args);
  const choices=await invoke('extension_traffic_columns');
  const choice=choices.find(column=>column.key==='test.development:protocol');
  if(!choice)throw new Error('Native static column declaration missing');
  const matched=traffic.sessions.find(row=>row.path==='/extension-fixture');
  const other=traffic.sessions.find(row=>row.path!=='/extension-fixture');
  const ids=[matched.id,...(other?[other.id]:[])];
  const rows=await invoke('classify_traffic_rows',{ids,columns:[choice.key]});
  if(rows.find(row=>row.id===matched.id)?.values[choice.key]?.[0]!=='Synthetic protocol')throw new Error('Paired native header classification failed');
  if(other&&rows.find(row=>row.id===other.id)?.values[choice.key]?.length)throw new Error('Unrelated headers were classified');
  const preferences=traffic.preferences;
  traffic.preferences={...preferences,extensionColumns:[choice.key]};
  traffic.refreshExtensionColumns();
  const deadline=Date.now()+20000;
  while(!traffic.sessions.find(row=>row.id===matched.id)?.extensionCells.some(cell=>cell.key===choice.key&&cell.text==='Synthetic protocol')){
    if(Date.now()>deadline)throw new Error('Native static column did not render');
    await new Promise(resolve=>setTimeout(resolve,25));
  }
  await invoke('enable_extension',{id:'test.development',enabled:false});
  if((await invoke('extension_traffic_columns')).some(column=>column.key===choice.key))throw new Error('Disabled column remained available');
  if((await invoke('classify_traffic_rows',{ids,columns:[choice.key]})).length)throw new Error('Disabled declaration classified traffic');
  await invoke('enable_extension',{id:'test.development',enabled:true});
  traffic.preferences=preferences;traffic.refreshExtensionColumns();
  globalThis.__nativeClassificationVerified={pairedHeaders:true,staticMetadata:true,boundedRows:ids.length,disabled:true,rendered:true};
}
