import assert from 'node:assert/strict';
import {resolve} from 'node:path';

export async function checkResponseSnapshots(page,root) {
  await page.goto('https://workspace.test/');
  await page.waitForFunction(()=>document.querySelector('app-shell').traffic.queryLoaded);
  await page.locator('app-shell a[data-view="traffic"]').click();
  await page.evaluate(()=>{
    const row=globalThis.__workspaceFixture.sessions[0];
    globalThis.__workspaceFixture.snapshotEntryId=row.id;
    row.autoResponse={ruleId:'historical-rule',ruleName:'External stylesheet experiment with a long descriptive name',ruleRevision:7,position:1,assetReference:'stylesheet@3',status:200,bodyBytes:4,snapshot:{status:200,bodyBytes:4,sha256:'a'.repeat(64),contentCodings:['gzip']}};
    row.storedBodies=[{exchangeId:row.id,boundary:'client-response',observedBytes:4,retainedBytes:4,availability:'complete',sha256:'a'.repeat(64),contentCodings:['gzip'],mediaType:'text/css',reason:null}];
  });
  await page.evaluate(()=>document.querySelector('app-shell').traffic.revealSession(globalThis.__workspaceFixture.snapshotEntryId));
  const button=page.locator('.selection-actions').getByRole('button',{name:'Details…',exact:true});
  await button.click();
  const dialog=page.locator('.timing-dialog[open]'),evidence=dialog.locator('.response-file-evidence');
  await evidence.locator('summary').click();
  await evidence.getByText('Captured response bytes match the selected file snapshot.',{exact:false}).waitFor();
  for(const [width,height] of [[1440,900],[960,720],[800,600]]) {
    await page.setViewportSize({width,height});
    await page.screenshot({path:resolve(root,`../../../target/ui-check/file-snapshot-${width}.png`)});
    assert.equal(await evidence.evaluate(node=>node.scrollWidth<=node.clientWidth+1),true);
    const bounds=await dialog.boundingBox();assert.ok(bounds.width<=width&&bounds.height<=height);
    assert.equal(await dialog.getByRole('button',{name:'Close',exact:true}).isVisible(),true);
  }
  await page.emulateMedia({forcedColors:'active'});
  await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').style.zoom='1.5');
  try {const close=await dialog.getByRole('button',{name:'Close',exact:true}).boundingBox();assert.ok(close.y>=0&&close.y+close.height<=600,'Zoom must keep Close inside the viewport');await page.screenshot({path:resolve(root,'../../../target/ui-check/file-snapshot-zoom-contrast.png')});}
  finally {await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').style.zoom='');await page.emulateMedia({forcedColors:'none'});}
  const update=async(availability,sha256)=>{
    await page.evaluate(({availability,sha256})=>{
      const state=globalThis.__workspaceFixture,row=state.sessions.find(row=>row.id===state.snapshotEntryId);
      Object.assign(row.storedBodies[0],{availability,sha256});
    },{availability,sha256});
    await dialog.getByRole('button',{name:'Refresh measurements',exact:true}).click();
  };
  await update('complete',null);
  await evidence.getByText(/Complete response bytes are retained/).waitFor();
  await evidence.getByText('Not computed',{exact:false}).waitFor();
  await update('complete','b'.repeat(64));
  await evidence.getByText(/Captured response bytes differ from the selected snapshot/).waitFor();
  await update('truncated','c'.repeat(64));
  await evidence.getByText(/Check body retention limits or record another request/).waitFor();
  assert.equal(await evidence.getByText('c'.repeat(64),{exact:true}).count(),0,'Prefix hash must not be presented as a complete response hash');
  await page.keyboard.press('Escape');
  assert.equal(await button.evaluate(node=>node===node.getRootNode().activeElement),true);
  await page.evaluate(async()=>{const t=document.querySelector('app-shell').traffic,detail=await window.__TAURI_INTERNALS__.invoke('session_detail',{id:globalThis.__workspaceFixture.snapshotEntryId});detail.traceId='saved-source';t.applyDetail(detail);});
  await page.locator('.selection-actions button').filter({hasText:'Show External stylesheet experiment'}).waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').traffic.selectedDetail.autoResponse.ruleRevision),7,'Imported rule identity must remain visible as evidence');
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture,row=state.sessions.find(row=>row.id===state.snapshotEntryId);row.autoResponse=null;delete row.storedBodies;});
  await button.click();await evidence.waitFor({state:'detached'});await page.keyboard.press('Escape');
  await page.setViewportSize({width:1280,height:800});
}
