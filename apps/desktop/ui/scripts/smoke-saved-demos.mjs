import assert from 'node:assert/strict';
import {mkdir,writeFile,rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {resolve} from 'node:path';

// Uses the same captured entry and capture lifetime through Clear/Undo. Durable
// playback must stay separate from ordinary preview sources after Traffic expiry.
export async function smokeSavedDemoAssociations({main,waitFor,directory}) {
  const output=resolve(directory,'demo-associations');await mkdir(output,{recursive:true});
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const id='native-demo-association',url='https://native-demo-association.invalid/',text='<!doctype html><html><body><h1>Same captured page</h1></body></html>',capture=resolve(output,'same-capture.har');
  const entry={startedDateTime:'2026-10-18T18:00:00Z',time:1,request:{method:'GET',url,httpVersion:'HTTP/1.1',headers:[],queryString:[],cookies:[],headersSize:-1,bodySize:0},response:{status:200,statusText:'OK',httpVersion:'HTTP/1.1',headers:[{name:'Content-Type',value:'text/html'}],cookies:[],content:{size:text.length,mimeType:'text/html',text},redirectURL:'',headersSize:-1,bodySize:text.length},cache:{},timings:{send:0,wait:0,receive:1}};
  await writeFile(capture,JSON.stringify({log:{version:'1.2',creator:{name:'Saved-demo association regression',version:'1'},entries:[entry]}}));
  await waitFor("document.querySelector('app-shell')?.workspaceLoaded");
  await main.evaluate(`(async()=>{const s=document.querySelector('app-shell');await s.activateView('traffic');await s.traffic.importPaths([${JSON.stringify(capture)}]);})()`);
  const before=await invoke('automation_status'),status=await invoke('change_response_set',{generation:before.generation,change:{action:'create',id,name:'Demo association source',notes:'Same captured identity',rules:[]}}),owner=status.responseSets.find(set=>set.id===id);
  const choices=await invoke('experiment_preview_pages',{id,selectedId:null,includePages:true}),page=choices.pages.find(page=>page.url===url);assert.ok(page,'Complete captured HTML unavailable');
  await invoke('set_experiment_preview_page',{id,revision:choices.revision,page});
  const variant={experimentId:id,incarnation:owner.incarnation,variantId:'',revision:owner.revision},options={scope:'original-trace',variant,pageReference:page};
  const prepared=await invoke('prepare_demo_save',{operationId:crypto.randomUUID(),input:{ownerId:id,name:'Frozen association demo',entryId:page.entryId,options,replace:null}}),demo=await invoke('commit_demo_save',{token:prepared.token,reviewedMissing:true,operationId:crypto.randomUUID()});
  const openDemo=()=>invoke('open_saved_demo',{ownerId:id,demo:{id:demo.id,revision:demo.revision},enableScripts:false,operationId:crypto.randomUUID()});
  const demoOnlyLabel=await openDemo();await invoke('clear_traffic');
  const demoOnly=await invoke('experiment_preview_pages',{id,selectedId:null,includePages:false});
  assert.deepEqual(demoOnly.page,page,'Clear rebound the associated capture');assert.equal(demoOnly.available,false,'A saved demo falsely made an expired captured page available');assert.equal(demoOnly.sourceLabel,null);
  const rejected=await main.evaluate(`window.__TAURI_INTERNALS__.invoke('open_captured_page',${JSON.stringify({id:page.entryId,enableScripts:false,options,sourceLabel:demoOnlyLabel,operationId:crypto.randomUUID()})}).then(()=>null,error=>typeof error==='string'?error:JSON.stringify(error))`);assert.match(rejected,/Open this frozen demo from Saved demos/,'Ordinary playback reapplied current rules to a saved demo');
  await invoke('remove_traffic_entries',{ids:[page.entryId],restore:true});
  const restored=await invoke('experiment_preview_pages',{id,selectedId:null,includePages:false});assert.equal(restored.available,true);assert.deepEqual(restored.page,page,'Undo did not restore the exact capture identity');
  const ordinaryLabel=await invoke('open_captured_page',{id:page.entryId,enableScripts:false,options,sourceLabel:null,operationId:crypto.randomUUID()}),newerDemoLabel=await openDemo();
  await invoke('clear_traffic');const mixed=await invoke('experiment_preview_pages',{id,selectedId:null,includePages:false});
  assert.equal((await invoke('query_sessions',{query:{limit:10}})).sessions.length,0);assert.deepEqual(mixed.page,page);assert.equal(mixed.available,true);assert.equal(mixed.sourceLabel,ordinaryLabel,'A newer saved demo shadowed an earlier usable ordinary preview');assert.notEqual(mixed.sourceLabel,newerDemoLabel);
  const ordinaryReport=await invoke('captured_page_report',{label:ordinaryLabel}),newerDemoReport=await invoke('captured_page_report',{label:newerDemoLabel});assert.equal(ordinaryReport.savedDemo,null);assert.equal(newerDemoReport.savedDemo.id,demo.id);assert.equal(ordinaryReport.bytesAvailable,true);assert.equal(ordinaryReport.selectedEntryId,page.entryId);
  const result={samePage:page,demoOnlyLabel,ordinaryLabel,newerDemoLabel,demoOnly,mixed,ordinaryReport,newerDemoReport,frozenPlaybackGuard:rejected};
  await writeFile(resolve(output,'native-demo-association-result.json'),JSON.stringify(result,null,2));
  return {demoOnlyUnavailable:true,newerDemoDoesNotShadowOrdinary:true,sameCaptureIdentity:true,frozenPlaybackGuardPreserved:true};
}

// The caller owns the unique executable, isolated profile and restart lifecycle.
export async function smokeSavedDemos(context) {
  let {main,waitFor}=context;const output=resolve(context.directory,'demos');await mkdir(output,{recursive:true});
  const shell="document.querySelector('app-shell')",a=shell+'.automation',sets=a+'.responseSets',life=sets+'.lifecycle',demos=a+'.responseSetDemos',save=demos+'.saveDialog',list=demos+'.listDialog',traffic=shell+'.traffic';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const button=(container,text)=>`[...${container}.querySelectorAll('button')].find(node=>node.textContent.trim()===${JSON.stringify(text)}&&node.getClientRects().length)`;
  const pointer=async expression=>{await main.evaluate(`${expression}.scrollIntoView({block:'nearest'})`);const point=await main.evaluate(`(()=>{const r=${expression}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);for(const type of ['mouseMoved','mousePressed','mouseReleased'])await main.call('Input.dispatchMouseEvent',{type,button:'left',buttons:type==='mousePressed'?1:0,clickCount:1,...point});};
  const key=async(key,code,vk,modifiers=0)=>{for(const type of ['keyDown','keyUp'])await main.call('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:vk,modifiers,...(key==='Enter'&&type==='keyDown'?{text:'\r'}:{})});};
  const input=async(expression,text)=>{await pointer(expression);await key('a','KeyA',65,2);await main.call('Input.insertText',{text});};
  const idle=()=>waitFor(`!${life}.pending&&!${sets}.busy&&!${demos}.busy&&!${demos}.pagesLoading&&!${demos}.loading`);
  const more=async label=>{await pointer(life+'.moreButton');await pointer(button(life+'.moreMenu',label));};
  const shot=async name=>{const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(resolve(output,name),Buffer.from(image.data,'base64'));};
  let networkRequests=0;const origin=createServer((_,response)=>{networkRequests++;response.end('must not contact');});await new Promise(resolve=>origin.listen(0,'127.0.0.1',resolve));
  const external=resolve(output,'linked.css'),capture=resolve(output,'site.har'),red='body{color:rgb(200,0,0)}',redHash=createHash('sha256').update(red).digest('hex');
  try {
    await waitFor(`${shell}?.workspaceLoaded`);
    await writeFile(external,red);
    const html=`<!doctype html><html><head><link rel="stylesheet" href="/theme.css"></head><body><h1>Frozen native demo</h1><img src="/missing.png"><img src="http://127.0.0.1:${origin.address().port}/uncaptured"><script>fetch('/script-ran').catch(()=>{})</script></body></html>`;
    const row=(url,mime,text,status=200)=>({startedDateTime:'2026-10-18T18:00:00Z',time:1,request:{method:'GET',url,httpVersion:'HTTP/1.1',headers:[],queryString:[],cookies:[],headersSize:-1,bodySize:0},response:{status,statusText:status===304?'Not Modified':'OK',httpVersion:'HTTP/1.1',headers:[{name:'Content-Type',value:mime}],cookies:[],content:{size:Buffer.byteLength(text),mimeType:mime,text},redirectURL:'',headersSize:-1,bodySize:Buffer.byteLength(text)},cache:{},timings:{send:0,wait:0,receive:1}});
    await writeFile(capture,JSON.stringify({log:{version:'1.2',creator:{name:'Durable native demo qualification',version:'1'},entries:[row('https://native-demo.invalid/','text/html',html),row('https://native-demo.invalid/theme.css','text/css','body{color:black}'),row('https://native-demo.invalid/cache.css','text/css','',304)]}}));
    await main.evaluate(`(async()=>{await ${shell}.activateView('traffic');await ${traffic}.importPaths([${JSON.stringify(capture)}]);})()`);
    const asset=await invoke('import_response_asset',{input:{id:'native-demo-linked',revision:1,status:200,headers:[],bodyPath:external,bodyFileMode:'linked',mediaType:'text/css'}}),action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
    const rule={id:'native-demo-style',displayName:'Frozen linked style',enabled:true,revision:1,priority:-1000000,matcher:{method:'GET',url:{kind:'exact',value:'https://native-demo.invalid/theme.css'},requestHeaders:[],responseHeaders:[]},request:{...action,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:action};
    const before=await invoke('automation_status'),id='native-durable-demo-owner';
    await invoke('change_response_set',{generation:before.generation,change:{action:'create',id,name:'Durable native demonstration',notes:'Exact linked response and captured page',rules:[rule]}});
    await main.evaluate(`(async()=>{await ${shell}.activateView('automation');await ${a}.refreshAutomation();await ${a}.showAutomationSection('experiments');await ${a}.openResponseSet(new CustomEvent('set-open',{detail:{id:${JSON.stringify(id)}}}),true);})()`);
    await pointer(button(life,'Use on live traffic'));await idle();let status=await invoke('automation_status');assert.equal(status.responseSets.find(set=>set.id===id).applied.revision,1);
    await invoke('set_autoresponses_enabled',{enabled:false,generation:status.generation});await main.evaluate(a+'.refreshAutomation()');await pointer(button(life,'Pause'));await idle();await pointer(button(life,'Resume'));await idle();assert.equal((await invoke('automation_status')).autoresponsesEnabled,false);
    await more('Versions…');await input(life+'.versionNameInput','Native original');await pointer(button(life+'.versionsDialog','Save named version…'));await idle();await pointer(button(life+'.versionsDialog','Close'));
    status=await invoke('automation_status');let owner=status.responseSets.find(set=>set.id===id);
    await invoke('change_response_set',{generation:status.generation,change:{action:'save-current',id,expectedRevision:owner.revision,name:owner.name,notes:'A newer saved note',rules:owner.rules}});
    await main.evaluate(`(async()=>{await ${a}.refreshAutomation();await ${a}.openResponseSet(new CustomEvent('set-open',{detail:{id:${JSON.stringify(id)}}}),true);})()`);
    await more('Versions…');await pointer(button(life+'.versionsDialog','Restore to current set…'));await pointer(button(life+'.confirmDialog','Restore to current set'));await idle();await pointer(button(life+'.versionsDialog','Close'));status=await invoke('automation_status');owner=status.responseSets.find(set=>set.id===id);assert.equal(owner.revision,3);assert.equal(owner.applied.revision,1);assert.equal(status.autoresponsesEnabled,false);
    await pointer(life+'.tryButton');await pointer(button(life+'.tryMenu','Save demo…'));await waitFor(`${save}.open`);await idle();await input(demos+'.nameInput','Native frozen red demo');assert.equal(await main.evaluate(`${save}.querySelectorAll('input[type="radio"]:checked').length`),0);
    await pointer(save+`.querySelector('input[type="radio"]')`);await pointer(button(save,'Review demo'));await idle();assert.equal(await main.evaluate(`${demos}.review.report.skipped`),1);assert.equal(await main.evaluate(`${demos}.review.report.rules[0].sha256`),redHash);
    const token=await main.evaluate(demos+'.review.token');await writeFile(external,'body{color:blue}');await invoke('clear_traffic');
    for(const [width,height] of [[1440,900],[960,720],[800,600]]){await main.call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});await shot(`native-demo-review-${width}.png`);assert.equal(await main.evaluate(`(()=>{const r=${button(save,'Save demo')}.getBoundingClientRect();return r.top>=0&&r.bottom<=innerHeight;})()`),true);}
    await main.call('Emulation.clearDeviceMetricsOverride');await pointer(button(save,'Save demo'));await waitFor(`!${save}.open`);const saved=(await invoke('saved_demos',{ownerId:id}))[0];assert.ok(saved);assert.equal(saved.selection.revision,3);assert.equal((await invoke('automation_status')).responseSets.find(set=>set.id===id).previewPage,null);assert.equal((await invoke('query_sessions',{query:{limit:100}})).sessions.length,0);
    const open=async scripts=>{await more('Saved demos…');await waitFor(`${list}.open`);await idle();await pointer(button(list,'Open demo…'));await waitFor(`${traffic}.previewPageDialog.open`);assert.equal(await main.evaluate(`${traffic}.previewPageScripts.checked`),false);if(scripts)await pointer(traffic+'.previewPageScripts');await pointer(button(traffic+'.previewPageDialog','Open preview'));await waitFor(`!${traffic}.previewPageBusy&&!${traffic}.previewPageDialog.open`);const label=await main.evaluate(traffic+'.previewReportLabel');await waitFor(`(async()=>{const r=await window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(label)}});return r.requests.some(row=>row.url.endsWith('/theme.css'));})()`);return {label,report:await invoke('captured_page_report',{label})};};
    const first=await open(false);assert.equal(first.report.scriptsEnabled,false);assert.equal(first.report.requests.find(row=>row.ruleId==='native-demo-style').sha256,redHash);assert.equal(first.report.requests.some(row=>row.url.endsWith('/script-ran')),false);assert.equal(first.report.requests.some(row=>row.outcome==='missing'),true);
    const second=await open(true);await waitFor(`(async()=>{const r=await window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(second.label)}});return r.requests.some(row=>row.url.endsWith('/script-ran'));})()`);assert.equal((await invoke('captured_page_report',{label:second.label})).scriptsEnabled,true);assert.equal(networkRequests,0);
    // The source file and Traffic can disappear before restarting the app.
    await rm(external);await rm(capture);const restarted=await context.restart();main=restarted.main;waitFor=restarted.waitFor;await waitFor(`${shell}?.workspaceLoaded`);
    status=await invoke('automation_status');owner=status.responseSets.find(set=>set.id===id);assert.equal(owner.applied.revision,1);assert.equal(owner.versions.length,1);assert.equal(status.autoresponsesEnabled,false);assert.equal((await invoke('query_sessions',{query:{limit:100}})).sessions.length,0);
    await main.evaluate(`(async()=>{await ${shell}.activateView('automation');await ${a}.refreshAutomation();await ${a}.showAutomationSection('experiments');await ${a}.openResponseSet(new CustomEvent('set-open',{detail:{id:${JSON.stringify(id)}}}),true);})()`);
    const afterRestart=await open(false);assert.equal(afterRestart.report.requests.find(row=>row.ruleId==='native-demo-style').sha256,redHash);
    await more('Delete response set…');await pointer(button(life+'.confirmDialog','Delete response set'));await idle();status=await invoke('automation_status');assert.equal(status.responseSets.some(set=>set.id===id),false);assert.deepEqual(status.rules,before.rules);assert.equal(status.autoresponsesEnabled,false);
    // Existing native playback still owns immutable copied bytes after owner GC.
    const oldHits=afterRestart.report.hits,preview=await context.preview('https://native-demo.invalid/');
    await preview.call('Page.reload',{ignoreCache:true});await waitFor(`(async()=>{const r=await window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(afterRestart.label)}});return r.hits>${oldHits}&&r.requests.filter(row=>row.ruleId==='native-demo-style').length>=2;})()`);preview.close();
    const retained=await invoke('captured_page_report',{label:afterRestart.label});assert.equal(retained.bytesAvailable,true);assert.equal(retained.requests.findLast(row=>row.ruleId==='native-demo-style').sha256,redHash);assert.equal(networkRequests,0);
    await writeFile(resolve(output,'native-demo-result.json'),JSON.stringify({saved,reviewToken:token,redHash,first:first.report,second:await invoke('captured_page_report',{label:afterRestart.label}),afterRestart:afterRestart.report,retained,final:status,noOriginRequests:networkRequests===0,sourceTrafficAndLinkedFileAbsent:true,lifecycle:{applyPauseResume:true,globalPausePreserved:true,namedRestoreIndependentApplied:true,restart:true,delete:true}},null,2));
    return {savedDemo:saved.id,restart:true,exactFrozenHash:redHash,globalPausePreserved:true,noOriginRequests:true};
  }finally{origin.close();}
}
