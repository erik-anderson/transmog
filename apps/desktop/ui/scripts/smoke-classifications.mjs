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
  const wait=async(condition)=>{const deadline=Date.now()+20000;while(!condition()){if(Date.now()>deadline)throw new Error('Native classification update did not settle');await new Promise(resolve=>setTimeout(resolve,25));}};
  await wait(()=>traffic.classifications.choices.some(column=>column.key===choice.key));
  const preferences=traffic.preferences;
  traffic.preferences={...preferences,extensionColumns:[choice.key]};
  const deadline=Date.now()+20000;
  while(!traffic.sessions.find(row=>row.id===matched.id)?.extensionCells.some(cell=>cell.key===choice.key&&cell.text==='Synthetic protocol')){
    if(Date.now()>deadline)throw new Error('Native static column did not render');
    await new Promise(resolve=>setTimeout(resolve,25));
  }
  const root=traffic.shadowRoot??traffic;
  const heading=root.querySelector('.extension-column-trigger');
  if(!heading?.querySelector('svg[aria-hidden=true]'))throw new Error('Extension heading has no compact icon');
  const counts=await invoke('extension_traffic_facets',{request:{query:traffic.trafficQuery(),column:choice.key}});
  const option=counts.options.find(option=>option.label==='Synthetic protocol');
  const unmatched=counts.options.find(option=>option.label==='No match');
  if(!option?.count||!unmatched||(other&&unmatched.count===0))throw new Error('Native extension filter counts omitted matching or unrelated headers: '+JSON.stringify({counts,query:traffic.trafficQuery(),matched:matched.id,other:other?.id}));
  heading.click();
  await wait(()=>!traffic.pickerLoading);
  const checkbox=root.querySelector('#'+CSS.escape('filter-option-'+option.key));
  if(!checkbox)throw new Error('Native extension filter option missing');
  checkbox.click();traffic.filterPicker.querySelector('button[type=submit]').click();
  await wait(()=>!traffic.queryPending&&traffic.totalMatched===option.count&&traffic.filterGroups.some(group=>group.dimension==='extension:'+choice.key));
  if(traffic.trafficQuery().filterGroups.some(group=>group.dimension.startsWith('extension:')))throw new Error('Extension key escaped into ordinary facets');
  heading.click();await wait(()=>!traffic.pickerLoading);
  root.querySelector('#'+CSS.escape('filter-option-'+option.key)).click();
  root.querySelector('#filter-option-unavailable').click();
  traffic.filterPicker.querySelector('button[type=submit]').click();
  await wait(()=>!traffic.queryPending&&traffic.totalMatched===unmatched.count);
  await traffic.removeFilterDimension('extension:'+choice.key);
  await invoke('enable_extension',{id:'test.development',enabled:false});
  if((await invoke('extension_traffic_columns')).some(column=>column.key===choice.key))throw new Error('Disabled column remained available');
  if((await invoke('classify_traffic_rows',{ids,columns:[choice.key]})).length)throw new Error('Disabled declaration classified traffic');
  await wait(()=>!traffic.extensionTableColumns.length&&!traffic.classifications.choices.some(column=>column.key===choice.key));
  await invoke('enable_extension',{id:'test.development',enabled:true});
  await wait(()=>traffic.extensionTableColumns.some(column=>column.key===choice.key));
  traffic.preferences=preferences;
  globalThis.__nativeClassificationVerified={pairedHeaders:true,staticMetadata:true,boundedRows:ids.length,disabled:true,rendered:true,automaticUpdates:true,filtered:option.count,unmatched:unmatched.count,contextualCounts:true,headingIcon:true};
}
