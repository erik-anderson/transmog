import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";

export async function smokeIncidentRecording(main, viewer, traffic, path, screenshot, waitFor) {
  const original = await main.evaluate("window.__TAURI_INTERNALS__.invoke('product_state')");
  try {
    await main.evaluate(
      `(async()=>{const state=await window.__TAURI_INTERNALS__.invoke('product_state');state.preferences.configureSystemProxy=false;state.preferences.decryptHttps=false;await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:state});await window.__TAURI_INTERNALS__.invoke('start_capture',{request:{path:${JSON.stringify(path)},password:null,retainBodySamples:true,redactSensitiveHeaders:false,includeNetworkContext:false}});const shell=document.querySelector('app-shell');await shell.activateView('captures');await shell.shadowRoot.querySelector('capture-workspace').refreshCapture();})()`,
    );
    assert.equal(
      await viewer.evaluate(
        "window.__TAURI_INTERNALS__.invoke('mark_incident',{note:'must fail'}).then(()=>false,()=>true)",
      ),
      true,
      "Viewer marked the main recording",
    );
    await waitFor(
      () =>
        main.evaluate(
          "(()=>{const w=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');w.$flushUpdates();return w.captureActive&&Boolean(w.incidentNote);})()",
        ),
      "Recording marker did not hydrate",
    );
    await main.evaluate(
      `(async()=>{const workspace=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');workspace.incidentNote.value='Native desktop incident <script>inert</script>';await workspace.markIncident();})()`,
    );
    assert.match(
      await main.evaluate(
        "document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace').incidentMarkStatus",
      ),
      /Issue marker recorded/,
    );
    await main.evaluate(
      `(async()=>{const workspace=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace');workspace.$emit('status-changed',await window.__TAURI_INTERNALS__.invoke('app_status'));workspace.$flushUpdates();workspace.querySelector('[aria-label="Incident marker"] button').scrollIntoView({block:'center'});})()`,
    );
    assert.ok(
      await main.evaluate(
        `(()=>{const bounds=document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace').querySelector('[aria-label="Incident marker"] button').getBoundingClientRect();return bounds.top>=0&&bounds.bottom<=innerHeight;})()`,
      ),
    );
    if (screenshot) {
      const image = await main.call("Page.captureScreenshot", { format: "png" });
      await writeFile(screenshot.replace(".png", "-mark.png"), Buffer.from(image.data, "base64"));
    }
    await main.evaluate("window.__TAURI_INTERNALS__.invoke('stop_capture')");
    const metadata = await main.evaluate(
      `window.__TAURI_INTERNALS__.invoke('inspect_trace_metadata',{request:{path:${JSON.stringify(path)},password:null}})`,
    );
    assert.equal(metadata.sessions, 0, "Marking created a fake HTTP exchange");
    assert.ok(JSON.stringify(metadata).includes("Native desktop incident <script>inert</script>"));
    assert.equal(
      await main.evaluate("window.__TAURI_INTERNALS__.invoke('app_status').then(s=>s.lifecycle)"),
      "stopped",
    );
  } finally {
    await main.evaluate(
      `(async()=>{await window.__TAURI_INTERNALS__.invoke('stop_capture');await window.__TAURI_INTERNALS__.invoke('save_product_state',{productState:${JSON.stringify(original)}});await document.querySelector('app-shell').activateView('traffic');})()`,
    );
  }
  process.stdout.write(
    "Native desktop marker, marker-only file, viewer denial and stopped routing verified.\n",
  );
}

export async function smokeIncidentIntake(viewer, traffic, traceId, screenshot) {
  const intake = await viewer.evaluate(
    `window.__TAURI_INTERNALS__.invoke('incident_intake',{traceId:${JSON.stringify(traceId)}})`,
  );
  assert.equal(intake.markers[0].note, "Native issue <img src=x>");
  await viewer.evaluate(
    `(async()=>{const w=${traffic};await w.refreshMarkers();w.markersTraceId=${JSON.stringify(traceId)};w.reviewMarkers();await w.markerIntake.load();})()`,
  );
  assert.equal(
    await viewer.evaluate(`${traffic}.markerIntake.querySelectorAll('img,script').length`),
    0,
  );
  if (screenshot) {
    const image = await viewer.call("Page.captureScreenshot", { format: "png" });
    await writeFile(screenshot.replace(".png", "-incident.png"), Buffer.from(image.data, "base64"));
  }
  await viewer.evaluate(`${traffic}.showSaveTrace()`);
  await viewer.evaluate(
    `new Promise(resolve=>{const check=()=>!${traffic}.exportReviewBusy?resolve():setTimeout(check,30);check();})`,
  );
  assert.ok(
    await viewer.evaluate(
      `${traffic}.exportReviewId.length>0 && ${traffic}.exportReviewSummary.includes(' · 1 marker')`,
    ),
  );
  await viewer.evaluate(`${traffic}.closeSaveTrace()`);
  process.stdout.write("Native source incident intake and reviewed export commands verified.\n");
}

export async function smokeMarkerReview(client, traffic, screenshot) {
  const ready = (count) =>
    client.evaluate(
      `(async()=>{const w=${traffic};w.$flushUpdates();for(let i=0;i<100;i++){const intake=w.markerIntake,review=w.markerReviews.find(r=>r.traceId===intake.traceId);if(w.markerCount===${count}&&!intake.intakeBusy&&review?.revision===intake.markerRevision)return true;await new Promise(resolve=>setTimeout(resolve,30));}return false;})()`,
    );
  const result = await client.evaluate(
    `(async()=>{const w=${traffic};await w.revealSession(w.sessions[0].id);w.flagMessage('response',true);w.flagNote.value='Native reviewer response';await w.addMarker(new Event('submit'));await w.markerIntake.load();return {status:w.flagStatus,count:w.markerCount,review:w.markerReviews.find(r=>r.dirty)};})()`,
  );
  assert.equal(result.count, 1);
  assert.equal(result.review.links[0].boundary, "response");
  assert.equal(result.review.markers[0].origin, "analyst");
  const id = result.review.markers[0].id;
  try {
    for (const [width, height] of [
      [1440, 900],
      [960, 720],
      [800, 600],
    ]) {
      await client.call("Emulation.setDeviceMetricsOverride", {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
      });
      await client.evaluate(
        `(async()=>{const w=${traffic};w.markersOpen=true;w.setTimeline(true,false);await w.refreshTimeline();w.$flushUpdates();})()`,
      );
      const point = await client.evaluate(
        `(()=>{const w=${traffic},flag=w.timeline.querySelector('.timeline-overview .timeline-marker'),b=flag.getBoundingClientRect();return {x:b.x+b.width/2,y:b.y+b.height/2,width:b.width,height:b.height};})()`,
      );
      assert.ok(point.width >= 20 && point.height > 0);
      await client.call("Input.dispatchMouseEvent", {
        type: "mousePressed",
        x: point.x,
        y: point.y,
        button: "left",
        clickCount: 1,
      });
      await client.call("Input.dispatchMouseEvent", {
        type: "mouseReleased",
        x: point.x,
        y: point.y,
        button: "left",
        clickCount: 1,
      });
      const layout = await client.evaluate(
        `(async()=>{const w=${traffic};await w.refreshTimeline();w.$flushUpdates();const panel=w.querySelector('.trace-markers-panel').getBoundingClientRect(),list=w.querySelector('.session-list-pane').getBoundingClientRect(),preview=w.querySelector('.details-pane').getBoundingClientRect(),body=w.querySelector('message-inspector[side=response] .body-preview')?.getBoundingClientRect();return {bodyVisible:!!body&&body.top+20<=innerHeight,markers:panel.height,list:list.height,preview:preview.height,visible:preview.top<innerHeight&&preview.bottom<=innerHeight+1,current:w.markerCurrent,side:w.inspectorSide};})()`,
      );
      if (screenshot) {
        const image = await client.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", `-markers-${width}.png`),
          Buffer.from(image.data, "base64"),
        );
      }
      assert.ok(
        layout.markers <= height * 0.45 &&
          layout.list >= 100 &&
          layout.preview >= 100 &&
          layout.visible &&
          layout.bodyVisible,
        JSON.stringify(layout),
      );
      assert.equal(layout.side, "response");
      const ax = await client.call("Accessibility.getFullAXTree");
      assert.ok(
        ax.nodes.some(
          (n) =>
            n.role?.value === "button" &&
            String(n.name?.value).includes("Native reviewer response"),
        ),
      );
    }
    await client.evaluate(
      `(async()=>{const w=${traffic},intake=w.markerIntake;intake.editMarker(${JSON.stringify(id)});intake.markerEditor.value='Edited native reviewer note';intake.markerEditor.dispatchEvent(new Event('input',{bubbles:true}));await intake.applyPending();await w.refreshMarkers();})()`,
    );
    assert.equal(
      await client.evaluate(`${traffic}.markerReviews.find(r=>r.dirty).markers[0].note`),
      "Edited native reviewer note",
    );
    assert.equal(await ready(1), true, "Marker actions did not finish loading");
    assert.equal(
      await client.evaluate(
        `(async()=>{const w=${traffic},changed=await w.markerIntake.changeMarker('delete',${JSON.stringify(id)});await w.refreshMarkers();return changed;})()`,
      ),
      true,
    );
    assert.equal(await ready(0), true, "Deleted marker remained visible");
    assert.equal(
      await client.evaluate(
        `(async()=>{const w=${traffic},changed=await w.markerIntake.changeMarker('undo');await w.refreshMarkers();return changed;})()`,
      ),
      true,
    );
    assert.equal(await ready(1), true, "Undo did not restore the marker");
    await client.evaluate(`${traffic}.filesMenuButton.focus()`);
    await client.call("Input.dispatchKeyEvent", {
      type: "rawKeyDown",
      key: "w",
      code: "KeyW",
      windowsVirtualKeyCode: 87,
      modifiers: 2,
    });
    await client.call("Input.dispatchKeyEvent", {
      type: "keyUp",
      key: "w",
      code: "KeyW",
      windowsVirtualKeyCode: 87,
      modifiers: 2,
    });
    const guarded = await client.evaluate(
      `(async()=>{const w=${traffic};for(let i=0;i<100&&!w.markerUnsaved.open;i++)await new Promise(resolve=>setTimeout(resolve,30));return w.markerUnsaved.open;})()`,
    );
    assert.equal(guarded, true, "Native Ctrl+W discarded unsaved marker work");
    await client.evaluate(`${traffic}.resolveMarkerUnsaved('stay')`);
  } finally {
    await client.evaluate(
      `(async()=>{const w=${traffic};await window.__TAURI_INTERNALS__.invoke('discard_marker_changes');w.markerIntake.discardDrafts();await w.refreshMarkers();w.markersOpen=false;w.setTimeline(false,false);})()`,
    );
    await client.call("Emulation.clearDeviceMetricsOverride");
  }
  process.stdout.write(
    "Native marker flagging, timeline navigation, editing, deletion, Undo and compact preview layout verified.\n",
  );
}
