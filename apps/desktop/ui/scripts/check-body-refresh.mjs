import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkBodyRefresh(page,root,{reproduce=false}={}) {
  await page.goto('https://workspace.test/');
  const inspector=page.locator('traffic-workspace message-inspector[side="response"]');
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture,invoke=window.__TAURI_INTERNALS__.invoke;
    state.bodyRefresh={calls:[],hold:false,error:false,display:JSON.stringify({response:'Captured navigation response',items:Array.from({length:300},(_,i)=>({id:i,message:'Response data '+i+' <script>globalThis.capturedScriptRan=true</script>'}))},null,2)};
    state.originalBodyInvoke=invoke;
    window.__TAURI_INTERNALS__.invoke=async(command,args)=>{
      if(command!=='inspect_body')return invoke(command,args);
      const probe=state.bodyRefresh;probe.calls.push(structuredClone(args.request));
      const result=await invoke(command,args),display=probe.display;
      if(probe.hold){probe.hold=false;await new Promise(resolve=>probe.release=resolve);}
      if(probe.error)throw new Error('Fixture body read failed');
      const source=document.querySelector('app-shell').traffic.selectedDetail;
      return {...result,metadata:source?.id===args.request.sessionId?source.storedBodies.find(body=>body.boundary===args.request.boundary)??result.metadata:result.metadata,
        display:args.request.boundary==='client-request'?'Captured request body':args.request.boundary==='upstream-response'?'Original response body':args.request.sessionId==='first'?display:'New selected response',displayBytes:display.length};
    };
    state.sessions.find(row=>row.id==='first').responseBytes=48000;
  });
  try {
    await page.locator('tr[data-session-id="first"]').click();
    await inspector.locator('.body-preview').filter({hasText:'Captured navigation response'}).waitFor();
    const before=await inspector.evaluate(element=>{
      const text=element.querySelector('.body-preview'),scroll=text.scrollHeight>text.clientHeight?text:element.querySelector('.body-scroll');
      text.focus();scroll.scrollTop=360;
      const range=document.createRange();range.setStart(text.firstChild,15);range.setEnd(text.firstChild,43);
      const selection=text.getRootNode().getSelection?.()??document.getSelection();selection.removeAllRanges();selection.addRange(range);
      globalThis.__bodyReading={text,node:text.firstChild,scroll,scrollTop:scroll.scrollTop,selection:selection.toString()};
      return {calls:globalThis.__workspaceFixture.bodyRefresh.calls.length,scrollTop:scroll.scrollTop};
    });
    assert.ok(before.scrollTop>0,'Fixture body must scroll');
    await page.evaluate(async()=>{
      const state=globalThis.__workspaceFixture,w=document.querySelector('app-shell').traffic;
      state.bodyRefresh.hold=true;
      for(let i=0;i<6;i++){
        state.sessions.push({...state.sessions[1],id:'unrelated-'+i,requestNumber:String(10+i)});
        await document.querySelector('app-shell').refreshStatus();await w.refreshSessions();await w.refreshInspection();
        await new Promise(resolve=>setTimeout(resolve,25));
      }
    });
    const stable=await inspector.evaluate(element=>{
      const reading=globalThis.__bodyReading,text=element.querySelector('.body-preview'),selection=text?.getRootNode().getSelection?.()??document.getSelection();
      return {sameNode:text===reading.text&&text.firstChild===reading.node,scrollTop:reading.scroll.scrollTop,selection:selection.toString(),expectedSelection:reading.selection,loading:element.loading,calls:globalThis.__workspaceFixture.bodyRefresh.calls.length};
    });
    if(reproduce){assert.ok(stable.calls>before.calls,'Old repeated body fetch did not reproduce');assert.ok(!stable.sameNode||stable.selection!==stable.expectedSelection||stable.loading,'Old viewer replacement did not reproduce');process.stdout.write(JSON.stringify({bodyReloadReproduced:{before,after:stable}})+'\n');return;}
    assert.equal(stable.calls,before.calls,'Unchanged body was fetched again during live traffic refreshes');
    assert.equal(stable.sameNode,true,'Unchanged body text was replaced');
    assert.equal(stable.scrollTop,before.scrollTop,'Live refresh reset reading position');
    assert.equal(stable.selection,stable.expectedSelection,'Live refresh lost selected body text');
    assert.equal(stable.loading,false,'Live refresh displayed loading for unchanged body');
    await page.evaluate(()=>{const p=globalThis.__workspaceFixture.bodyRefresh;p.hold=false;p.release?.();});

    // Message direction and stage have independent evidence, even within one exchange.
    await page.evaluate(()=>{
      const w=document.querySelector('app-shell').traffic,d=structuredClone(w.selectedDetail),body=d.storedBodies[0];
      d.storedBodies.push({...body,boundary:'client-request'},{...body,boundary:'upstream-response'});
      d.responses.push({...d.responses[0],boundary:'upstream-response'});w.applyDetail(d);
      w.querySelector('message-inspector[side="request"]').showMode('body');
    });
    const request=page.locator('traffic-workspace message-inspector[side="request"]');
    await request.getByText('Captured request body',{exact:true}).waitFor();
    await inspector.getByLabel('Message stage',{exact:true}).selectOption('upstream-response');
    await inspector.getByText('Original response body',{exact:true}).waitFor();
    const stageCalls=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);
    await page.evaluate(()=>{const w=document.querySelector('app-shell').traffic;for(let i=0;i<3;i++)w.applyDetail(structuredClone(w.selectedDetail));});
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),stageCalls,'Identical stage/request evidence reloaded');
    await inspector.getByLabel('Message stage',{exact:true}).selectOption('client-response');
    await inspector.locator('.body-preview').filter({hasText:'Captured navigation response'}).waitFor();
    await request.getByRole('button',{name:'Headers',exact:true}).click();
    const evidenceCalls=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);

    // Body changes must still invalidate the preview, even at an unchanged size.
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;s.bodyRefresh.display='Updated retained response';s.sessions[0].storedBodies=[{...document.querySelector('app-shell').traffic.selectedDetail.storedBodies[0],sha256:'changed-body-digest'}];await document.querySelector('app-shell').traffic.refreshInspection();});
    await inspector.getByText('Updated retained response',{exact:true}).waitFor();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),evidenceCalls+1);
    await inspector.getByLabel('Body viewer',{exact:true}).selectOption('original-text');
    await page.waitForFunction(()=>globalThis.__workspaceFixture.bodyRefresh.calls.at(-1).representation==='original-text');
    await inspector.getByRole('button',{name:'Headers',exact:true}).click();
    await page.evaluate(async()=>{await document.querySelector('app-shell').traffic.refreshInspection();});
    await inspector.getByRole('button',{name:'Body',exact:true}).click();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),evidenceCalls+2,'Returning to body reloaded an unchanged preview');

    for(const [bytes,availability] of [[48001,'capturing'],[48002,'capturing'],[48002,'complete']]) {
      const prior=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);
      await page.evaluate(async({bytes,availability})=>{const s=globalThis.__workspaceFixture;s.bodyRefresh.display='Retained bytes '+bytes+' '+availability;Object.assign(s.sessions[0].storedBodies[0],{retainedBytes:bytes,observedBytes:bytes,availability});await document.querySelector('app-shell').traffic.refreshInspection();},{bytes,availability});
      await inspector.getByText(new RegExp('Retained bytes '+bytes+' '+availability)).waitFor();
      assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),prior+1,'Changed capture evidence did not reload');
    }
    await inspector.locator('.body-options > summary').click();
    await inspector.getByLabel('Decode',{exact:true}).uncheck();
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.at(-1).decodeContent),false);
    await inspector.getByLabel('Preview limit (bytes)',{exact:true}).fill('1024');await inspector.getByLabel('Preview limit (bytes)',{exact:true}).press('Tab');
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.at(-1).maxBytes),1024);
    await inspector.locator('.body-options > summary').click();

    // Hex keeps its reader, scroll and caret during identical updates.
    await page.evaluate(()=>{globalThis.__workspaceFixture.hexBytes=Array.from({length:12000},(_,i)=>i%256);});
    await inspector.getByLabel('Body viewer',{exact:true}).selectOption('bytes');
    await inspector.locator('hex-viewer').waitFor({state:'visible'});
    await inspector.locator('hex-viewer').getByRole('grid').press('Control+End');
    const hexBefore=await inspector.locator('hex-viewer').evaluate(v=>{globalThis.__bodyHex=v;return {caret:v.caret,top:v.viewport.scrollTop,calls:globalThis.__workspaceFixture.calls.read_body_range};});
    await page.evaluate(async()=>{for(let i=0;i<4;i++)await document.querySelector('app-shell').traffic.refreshInspection();});
    assert.deepEqual(await inspector.locator('hex-viewer').evaluate(v=>({caret:v.caret,top:v.viewport.scrollTop,calls:globalThis.__workspaceFixture.calls.read_body_range})),hexBefore);
    assert.equal(await inspector.locator('hex-viewer').evaluate(v=>v===globalThis.__bodyHex),true,'Hex viewer remounted');
    await inspector.getByLabel('Body viewer',{exact:true}).selectOption('image');
    await inspector.locator('img').waitFor({state:'visible'});
    await inspector.locator('img').evaluate(img=>globalThis.__bodyImage=img);
    const imageCalls=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);
    await page.evaluate(async()=>{for(let i=0;i<4;i++)await document.querySelector('app-shell').traffic.refreshInspection();});
    assert.equal(await inspector.locator('img').evaluate(img=>img===globalThis.__bodyImage),true,'Image preview remounted');
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),imageCalls);
    await inspector.getByLabel('Body viewer',{exact:true}).selectOption('original-text');

    // Slow results cannot overwrite a newer selected exchange.
    await page.evaluate(()=>{globalThis.__workspaceFixture.bodyRefresh.hold=true;void document.querySelector('app-shell').traffic.querySelector('message-inspector[side="response"]').inspectBody();});
    await page.waitForFunction(()=>Boolean(globalThis.__workspaceFixture.bodyRefresh.release));
    const pendingCalls=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);
    await page.evaluate(async()=>{for(let i=0;i<4;i++)await document.querySelector('app-shell').traffic.refreshInspection();});
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),pendingCalls,'Identical updates duplicated an in-flight body read');
    await page.locator('tr[data-session-id="second"]').click();
    await inspector.getByText('New selected response',{exact:true}).waitFor();
    await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.release());
    await page.waitForTimeout(30);
    assert.equal(await inspector.locator('.body-preview').textContent(),'New selected response');

    // A failed explicit refresh stays visible through identical detail updates and can be retried.
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;s.bodyRefresh.error=true;await document.querySelector('app-shell').traffic.querySelector('message-inspector[side="response"]').inspectBody();});
    await inspector.getByText(/Preview unavailable: Fixture body read failed/).waitFor();
    const failedCalls=await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length);
    await page.evaluate(async()=>{for(let i=0;i<3;i++)await document.querySelector('app-shell').traffic.refreshInspection();});
    assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.bodyRefresh.calls.length),failedCalls,'Identical updates repeatedly retried a failed preview');
    await page.evaluate(()=>{globalThis.__workspaceFixture.bodyRefresh.error=false;});
    await inspector.locator('.body-options > summary').click();await inspector.getByRole('button',{name:'Refresh preview',exact:true}).click();
    await inspector.getByText('New selected response',{exact:true}).waitFor();
    assert.equal(await page.evaluate(()=>globalThis.capturedScriptRan),undefined,'Captured script executed');
    await page.locator('tr[data-session-id="first"]').click();
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;Object.assign(s.sessions[0].storedBodies[0],{availability:'evicted',retainedBytes:0});await document.querySelector('app-shell').traffic.refreshInspection();});
    await inspector.getByText(/removed to make room in the circular response cache/).waitFor();
    assert.equal(await inspector.locator('hex-viewer,img').count(),0,'Evicted body retained a stale viewer');
    await page.evaluate(async()=>{const s=globalThis.__workspaceFixture;Object.assign(s.sessions[0].storedBodies[0],{availability:'complete',retainedBytes:48002});await document.querySelector('app-shell').traffic.refreshInspection();});
    await page.evaluate(async()=>{globalThis.__workspaceFixture.bodyRefresh.display=JSON.stringify({response:'Captured navigation response',items:Array.from({length:200},(_,i)=>({id:i,message:'Long response data '+i}))},null,2);await document.querySelector('app-shell').traffic.querySelector('message-inspector[side="response"]').inspectBody();});
    if(await inspector.locator('.body-options[open]').count())await inspector.locator('.body-options > summary').click();
    const divider=page.getByRole('separator',{name:'Resize traffic list and inspector',exact:true});
    await divider.press('Shift+ArrowUp');await divider.press('Shift+ArrowUp');
    for(const [width,height] of [[1440,900],[960,720],[800,600]]) {await page.setViewportSize({width,height});await page.screenshot({path:resolve(root,`../../../target/body-viewer-reload/browser-${width}.png`)});}
  } finally {
    await page.evaluate(()=>{const s=globalThis.__workspaceFixture;s.bodyRefresh.release?.();window.__TAURI_INTERNALS__.invoke=s.originalBodyInvoke;});
    await page.setViewportSize({width:1280,height:800});
    await page.goto('https://workspace.test/');
  }
}
