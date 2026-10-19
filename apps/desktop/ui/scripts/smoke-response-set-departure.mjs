import assert from 'node:assert/strict';
import {mkdir,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';

// The caller owns the native executable, fresh profile, CDP endpoint and process.
export async function smokeResponseSetDeparture({main,waitFor,directory}) {
  const output=resolve(directory,'departure');await mkdir(output,{recursive:true});
  const shell="document.querySelector('app-shell')",a=shell+'.automation',sets=a+'.responseSets',editor=a+'.autoResponseForm';
  const invoke=(command,args={})=>main.evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
  const button=(container,text)=>`[...${container}.querySelectorAll('button')].find(node=>node.textContent.trim()===${JSON.stringify(text)}&&node.getClientRects().length)`;
  const pointer=async expression=>{await waitFor(`(()=>{const node=${expression};return node&&node.getClientRects().length&&!node.disabled;})()`);await main.evaluate(`${expression}.scrollIntoView({block:'center'})`);const point=await main.evaluate(`(()=>{const r=${expression}.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);for(const type of ['mouseMoved','mousePressed','mouseReleased'])await main.call('Input.dispatchMouseEvent',{type,button:'left',buttons:type==='mousePressed'?1:0,clickCount:1,...point});};
  const input=async(expression,text)=>{await pointer(expression);assert.equal(await main.evaluate(`${expression}.getRootNode().activeElement===${expression}`),true);for(const type of ['keyDown','keyUp'])await main.call('Input.dispatchKeyEvent',{type,key:'a',code:'KeyA',windowsVirtualKeyCode:65,modifiers:2});await main.call('Input.insertText',{text});assert.equal(await main.evaluate(`${expression}.value`),text);};
  const path=resolve(output,'linked-body.txt');await writeFile(path,'Linked post-save departure bytes');
  const asset=await invoke('import_response_asset',{input:{id:'native-departure-asset',revision:1,status:200,headers:[],bodyPath:path,bodyFileMode:'linked',mediaType:'text/plain'}}),action={headers:[],replaceBody:null,discardBody:false,abortReason:null};
  const rule={id:'native-departure-rule',displayName:'Linked departure response',enabled:true,revision:1,priority:0,matcher:{method:'GET',url:{kind:'exact',value:'https://departure.invalid/linked'},requestHeaders:[],responseHeaders:[],scheme:null,host:null,port:null,pathPrefix:null,query:null,responseStatus:null,responseStatusClass:null},request:{...action,responseAsset:asset.id+'@'+asset.revision,allowNonIdempotentBodyReplacement:false},response:{...action}};
  const before=await invoke('automation_status'),created=await invoke('change_response_set',{generation:before.generation,change:{action:'create',id:'native-departure-set',name:'Native linked departure',notes:'Saved linked notes',rules:[rule]}});
  await main.evaluate(`(async()=>{await ${shell}.activateView('automation');${a}.renderAutomation(${JSON.stringify(created)});await ${a}.showAutomationSection('experiments');})()`);await waitFor(`${sets}.status.responseSets.some(set=>set.id==='native-departure-set')`);await main.evaluate(`${a}.openResponseSet(new CustomEvent('set-open',{detail:{id:'native-departure-set'}}))`);
  const results=[];
  for(const destination of ['back','scripts']) {
    await pointer(button(sets,'Linked departure response'));await waitFor(`!${a}.loadingSavedResponse&&!${a}.fileChoiceBusy`);await input(sets+'.notesInput','Native linked departure '+destination);
    await main.evaluate(`(()=>{delete globalThis.releaseDepartureFile;globalThis.departureCallbackResults=[];let saved=false;const callbacks=window.__TAURI_INTERNALS__.callbacks,original=callbacks.set;callbacks.set=function(id,callback){return original.call(this,id,result=>{if(globalThis.departureCallbackResults.length<128)globalThis.departureCallbackResults.push({keys:result&&typeof result==='object'?Object.keys(result):[],path:result?.path,bytes:result?.bytes,notes:result?.responseSets?.map(set=>({id:set.id,notes:set.notes}))});if(result?.responseSets?.some(set=>set.id==='native-departure-set'&&set.notes===${JSON.stringify('Native linked departure '+destination)}))saved=true;if(saved&&result?.path===${JSON.stringify(path)}&&typeof result.bytes==='number')globalThis.releaseDepartureFile=()=>callback(result);else callback(result);});};})()`);
    if(destination==='back')await pointer(sets+'.backButton');else await pointer(button(a+".querySelector('.workspace-tabs')",'Scripts'));
    await pointer(button(a+'.unsavedDialog','Save changes'));await waitFor(`globalThis.releaseDepartureFile&&!${a}.collectionOpen`);assert.equal(await main.evaluate(a+'.fileChoiceBusy'),true);assert.equal(await main.evaluate(a+'.editorHidden'),true);
    await main.evaluate('delete window.__TAURI_INTERNALS__.callbacks.set;globalThis.releaseDepartureFile()');await waitFor(`!${a}.fileChoiceBusy`);
    if(destination==='scripts')await pointer(button(a+".querySelector('.workspace-tabs')",'Response sets'));
    await pointer(button(sets,'Native linked departure'));assert.equal(await main.evaluate(sets+'.addButton.disabled'),false);await pointer(button(sets,'Linked departure response'));await waitFor(`!${a}.loadingSavedResponse&&!${a}.fileChoiceBusy`);assert.equal(await main.evaluate(`${button(editor,'Responses')}.disabled`),false);
    const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(resolve(output,'native-linked-departure-'+destination+'.png'),Buffer.from(image.data,'base64'));await pointer(button(editor,'Responses'));results.push({destination,busyRecovered:true,reopenedAndEditable:true});
  }
  const final=await invoke('automation_status');assert.deepEqual(final.rules,before.rules);assert.equal(final.responseSets.find(set=>set.id==='native-departure-set').notes,'Native linked departure scripts');await pointer(sets+'.backButton');
  await writeFile(resolve(output,'native-linked-departure-result.json'),JSON.stringify({results,noLiveChanges:true},null,2));return {results,noLiveChanges:true};
}
