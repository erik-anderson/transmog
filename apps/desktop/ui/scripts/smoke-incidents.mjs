import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';

export async function smokeIncidentRecording(main,viewer,traffic,path,screenshot,waitFor){
 const original=await main.evaluate("window.__TAURI_INTERNALS__.invoke('product_state')");
 try{
  await main.evaluate(`(async()=>{const state=await window.__TAURI_INTERNALS__.invoke('product_state');state.preferences.configureSystemProxy=false;state.preferences.decryptHttps=false;await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:state});await window.__TAURI_INTERNALS__.invoke('start_capture',{request:{path:${JSON.stringify(path)},password:null,retainBodySamples:true,redactSensitiveHeaders:false,includeNetworkContext:false}});const shell=document.querySelector('app-shell');await shell.activateView('captures');await shell.shadowRoot.querySelector('capture-workspace').refreshCapture();})()`);
  assert.equal(await viewer.evaluate("window.__TAURI_INTERNALS__.invoke('mark_incident',{note:'must fail'}).then(()=>false,()=>true)"),true,'Viewer marked the main recording');
  await waitFor(()=>main.evaluate("(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');w.$flushUpdates();return w.captureActive&&Boolean(w.incidentNote);})()"),'Recording marker did not hydrate');
  await main.evaluate(`(async()=>{const workspace=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');workspace.incidentNote.value='Native desktop incident <script>inert</script>';await workspace.markIncident();})()`);
  assert.match(await main.evaluate("document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace').incidentMarkStatus"),/Issue marker recorded/);
  await main.evaluate(`(async()=>{const workspace=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');workspace.$emit('status-changed',await window.__TAURI_INTERNALS__.invoke('app_status'));workspace.$flushUpdates();workspace.querySelector('[aria-label="Incident marker"] button').scrollIntoView({block:'center'});})()`);
  assert.ok(await main.evaluate(`(()=>{const bounds=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace').querySelector('[aria-label="Incident marker"] button').getBoundingClientRect();return bounds.top>=0&&bounds.bottom<=innerHeight;})()`));
  if(screenshot){const image=await main.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-mark.png'),Buffer.from(image.data,'base64'));}
  await main.evaluate("window.__TAURI_INTERNALS__.invoke('stop_capture')");
  const metadata=await main.evaluate(`window.__TAURI_INTERNALS__.invoke('inspect_trace_metadata',{request:{path:${JSON.stringify(path)},password:null}})`);
  assert.equal(metadata.sessions,0,'Marking created a fake HTTP exchange');
  assert.ok(JSON.stringify(metadata).includes('Native desktop incident <script>inert</script>'));
  assert.equal(await main.evaluate("window.__TAURI_INTERNALS__.invoke('app_status').then(s=>s.lifecycle)"),'stopped');
 }finally{
  await main.evaluate(`(async()=>{await window.__TAURI_INTERNALS__.invoke('stop_capture');await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:${JSON.stringify(original)}});await document.querySelector('app-shell').activateView('traffic');})()`);
 }
 process.stdout.write('Native desktop marker, marker-only file, viewer denial and stopped routing verified.\n');
}

export async function smokeIncidentIntake(viewer,traffic,traceId,screenshot){
 const intake=await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('incident_intake',{traceId:${JSON.stringify(traceId)}})`);
 assert.equal(intake.markers[0].note,'Native issue <img src=x>');
 await viewer.evaluate(`${traffic}.showTraceMetadata(${JSON.stringify(traceId)})`);
 await viewer.evaluate(`new Promise(resolve=>{const check=()=>{const intake=${traffic}.metadataDialog.querySelector('incident-intake');if(intake?.intakeMarkers.length)resolve();else setTimeout(check,30)};check();})`);
 assert.equal(await viewer.evaluate(`${traffic}.metadataDialog.querySelector('incident-intake').querySelectorAll('img,script').length`),0);
 if(screenshot){const image=await viewer.call('Page.captureScreenshot',{format:'png'});await writeFile(screenshot.replace('.png','-incident.png'),Buffer.from(image.data,'base64'));}
 await viewer.evaluate(`${traffic}.closeMetadata();${traffic}.showSaveTrace()`);
 await viewer.evaluate(`new Promise(resolve=>{const check=()=>!${traffic}.exportReviewBusy?resolve():setTimeout(check,30);check();})`);
 assert.ok(await viewer.evaluate(`${traffic}.exportReviewId.length>0 && ${traffic}.exportReviewSummary.includes('1 incident markers')`));
 await viewer.evaluate(`${traffic}.closeSaveTrace()`);
 process.stdout.write('Native source incident intake and reviewed export commands verified.\n');
}
