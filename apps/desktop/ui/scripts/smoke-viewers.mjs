import { createHash } from "node:crypto";
import {
  smokeIncidentRecording,
  smokeIncidentIntake,
  smokeMarkerReview,
} from "./smoke-incidents.mjs";
import { smokeComparison, smokePreviewComparison } from "./smoke-comparison.mjs";
import assert from "node:assert/strict";
import { smokePreviewRules } from "./smoke-preview-rules.mjs";
import { smokeResponseSetPages } from "./smoke-response-set-pages.mjs";
import { writeFile, readFile, glob } from "node:fs/promises";
import { join, dirname } from "node:path";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { smokeTrafficList } from "./smoke-traffic-list.mjs";
import { smokeCertificates } from "./smoke-certificates.mjs";
import { hexReachability } from "./check-hex-viewer.mjs";
import { smokeTraceSave } from "./smoke-trace-save.mjs";
import { smokeConnectTunnels } from "./smoke-connect-tunnels.mjs";

import { nativeFixture } from "./native-fixture.mjs";
import { smokeWindowShortcuts } from "./smoke-window-shortcuts.mjs";

const argument = (name) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
};
const port = argument("--port"),
  source = argument("--source"),
  executable = argument("--executable"),
  screenshot = argument("--screenshot");
const nativeSource = argument("--native-source");
const trafficSource = argument("--traffic-source");
const pageSource = argument("--page-source");
const profileRoot = argument("--profile-root");
const processId = argument("--process-id"),
  closeHelper = argument("--close-helper");
async function closeNative(title) {
  if (!processId || !closeHelper)
    throw new Error("Native close verification requires an isolated process identity.");
  const child = spawn(
    "pwsh.exe",
    [
      "-NoProfile",
      "-NonInteractive",
      "-File",
      closeHelper,
      "-ProbeProcessId",
      processId,
      "-WindowTitle",
      title,
    ],
    { windowsHide: true, stdio: "inherit" },
  );
  await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0 ? resolve() : reject(new Error("Native close probe exited " + code)),
    );
  });
}
if (!port || !source || !executable)
  throw new Error(
    "Viewer verification requires a DevTools port, isolated executable, and fixture source.",
  );
const sockets = [];
async function connect(target) {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  sockets.push(socket);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const result = JSON.parse(String(data));
    if (result.id) {
      const reply = pending.get(result.id);
      pending.delete(result.id);
      if (reply) {
        clearTimeout(reply.timer);
        if (result.error) reply.reject(new Error(result.error.message));
        else reply.resolve(result.result);
      }
    }
  });
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(method + " timed out"));
      }, 15000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    let result;
    try {
      result = await call("Runtime.evaluate", {
        expression,
        awaitPromise: true,
        returnByValue: true,
      });
    } catch (error) {
      throw new Error("Native evaluation failed: " + expression.slice(0, 300), { cause: error });
    }
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result?.value;
  };
  return { call, evaluate, close: () => socket.close() };
}
async function targets() {
  const ports = new Set([port]);
  if (profileRoot)
    for await (const path of glob("**/DevToolsActivePort", { cwd: profileRoot })) {
      try {
        ports.add((await readFile(join(profileRoot, path), "utf8")).split(/\r?\n/)[0]);
      } catch {
        /* A closed profile may be removed during discovery. */
      }
    }
  const pages = await Promise.all(
    [...ports].map(async (value) => {
      try {
        return await fetch(`http://127.0.0.1:${value}/json/list`).then((response) =>
          response.json(),
        );
      } catch {
        return [];
      }
    }),
  );
  return pages.flat().filter((target) => target.type === "page");
}
async function waitFor(check, message) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    const value = await check();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(message);
}
const root = 'document.querySelector("app-shell")';
const traffic = root + '.shadowRoot.querySelector("traffic-workspace")';
try {
  const pages = await targets();
  assert.equal(pages.length, 1, "File launch opened extra windows");
  const viewer = await connect(pages[0]);
  await waitFor(
    () =>
      viewer.evaluate(
        `${root}?.viewerMode && ${traffic}?.sessions.length===1 && !${traffic}.importingTrace`,
      ),
    "Saved-file viewer did not finish automatic import",
  );
  process.stdout.write("Viewer file import verified.\n");
  await smokeMarkerReview(viewer, traffic, screenshot);
  const traceSave = await smokeTraceSave(viewer, screenshot);
  if (process.argv.includes("--trace-save-only")) {
    process.stdout.write(JSON.stringify({ nativeTraceSave: traceSave }) + "\n");
  } else {
    const result = await viewer.evaluate(`(async()=>{
    const shell=${root}, workspace=${traffic};
    const controls=[...shell.shadowRoot.querySelectorAll('proxy-toggle,settings-workspace,breakpoint-workspace,automation-workspace')];
    if(controls.length)throw new Error('Viewer exposed proxy controls');
    const id=workspace.sessions[0].id;
    let denial='';try{await window.__TAURI_INTERNALS__.invoke('start_proxy',{request:{}});}catch(error){denial=String(error);}
    const detail=await window.__TAURI_INTERNALS__.invoke('session_detail',{id});
    const headers=await window.__TAURI_INTERNALS__.invoke('copy_all_headers',{id});
    const url=await window.__TAURI_INTERNALS__.invoke('copy_message_url',{id,boundary:'client-response'});
    const composer=await window.__TAURI_INTERNALS__.invoke('composer_source',{id});
    await workspace.selectTraffic(workspace.sessions[0],new MouseEvent('click'));
    return {denial,sourceIp:detail.sourceIp,headers,url,body:composer.body,traceId:detail.traceId};
  })()`);
    assert.match(result.denial, /only in the main proxy window/);
    assert.equal(result.sourceIp, "192.0.2.25");
    assert.match(result.headers, /\r\n\r\n\r\nHTTP\/1\.1 200 Fixture/);
    assert.equal(result.body, "");
    assert.ok(result.traceId);
    assert.equal(result.url, "https://example.invalid/path");
    process.stdout.write("Viewer inspection and permissions verified.\n");
    await waitFor(
      () => viewer.evaluate(`${traffic}.selectedDetail?.id===${traffic}.sessions[0]?.id`),
      "Selected native fixture detail did not finish loading",
    );
    await smokeConnectTunnels(viewer);
    const search = await viewer.evaluate(
      `(async()=>{const workspace=${traffic};await workspace.changeTrafficSearchKind({currentTarget:{value:'contents'}});workspace.searchMetadata=false;workspace.searchRequestHeaders=false;workspace.searchResponseHeaders=false;workspace.searchBodies=true;workspace.selectSearchMatches=true;workspace.searchInput.value='viewer response';await workspace.runContentSearch();return {matches:workspace.contentMatchCount,selected:workspace.selectedTrafficCount,status:workspace.contentSearchStatus};})()`,
    );
    assert.equal(search.matches, 1);
    assert.equal(search.selected, 1);
    assert.doesNotMatch(search.status, /binary/i);
    await viewer.evaluate(
      `(async()=>{const workspace=${traffic};workspace.searchMode='regex';workspace.searchInput.value='(';await workspace.runContentSearch();if(!workspace.contentSearchStatus.startsWith('Search failed:'))throw new Error('Invalid regex was not explained');await workspace.clearContentSearch();workspace.searchMode='text';})()`,
    );
    await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('open_main_window')`);
    const mainTarget = await waitFor(async () => {
      const all = await targets();
      return all.find((target) => target.id !== pages[0].id);
    }, "Main window did not open");
    const main = await connect(mainTarget);
    await waitFor(
      () => main.evaluate(`${root} && !${root}.viewerMode && ${traffic}?.queryLoaded`),
      "Main window did not initialize",
    );
    assert.equal(
      await main.evaluate(`${traffic}.sessions.length`),
      0,
      "Viewer imported into the main catalog",
    );
    await smokeWindowShortcuts(main, viewer, targets, connect, waitFor, closeNative);
    await smokeIncidentRecording(
      main,
      viewer,
      traffic,
      join(dirname(source), "desktop-marker.tmcap"),
      screenshot,
      waitFor,
    );
    await smokeComparison(viewer, main, traffic, screenshot);
    process.stdout.write("Main window opened with an independent catalog.\n");
    const child = spawn(executable, [source], { windowsHide: true, stdio: "ignore" });
    await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) =>
        code === 0 ? resolve() : reject(new Error("File handoff exited " + code)),
      );
    });
    await waitFor(
      () => main.evaluate(`${traffic}.openTraceDialog.open`),
      "Existing main window did not offer an import/viewer choice",
    );
    await main.evaluate(`${traffic}.chooseOpenedTrace(false)`);
    await waitFor(
      () => main.evaluate(`${traffic}.sessions.length===1 && !${traffic}.importingTrace`),
      "Choice did not import into current session",
    );
    assert.equal(
      await viewer.evaluate(`${traffic}.sessions.length`),
      1,
      "Main import changed the separate viewer",
    );
    await viewer.evaluate(`${traffic}.showTraceMetadata(${JSON.stringify(result.traceId)})`);
    await waitFor(
      () => viewer.evaluate(`${traffic}.metadataTraceName==='viewer-fixture.saz'`),
      "Request trace metadata navigation failed",
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(screenshot, Buffer.from(image.data, "base64"));
    }
    await viewer.evaluate(`${traffic}.closeMetadata()`);
    assert.equal(
      await viewer.evaluate(`${traffic}.matchedRuleId`),
      "",
      "A saved rule identity must not navigate to an unrelated local definition",
    );
    await viewer.evaluate(`${traffic}.showTimings()`);
    await waitFor(
      () => viewer.evaluate(`!${traffic}.timingBusy && ${traffic}.timingSaved.length>0`),
      "Imported SAZ timer evidence did not appear",
    );
    const timingState = await viewer.evaluate(
      `({open:${traffic}.timingDialog.open,summary:${traffic}.timingSummary,unknown:${traffic}.timingPhases[0].value})`,
    );
    assert.ok(timingState.open);
    assert.match(timingState.summary, /no measured proxy timeline/);
    assert.equal(timingState.unknown, "Unavailable");
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", "-timings.png"),
        Buffer.from(image.data, "base64"),
      );
    }

    await viewer.evaluate(`${traffic}.closeTimings()`);
    // A real native capture exercises import, timing persistence and WebView2
    // geometry together; it uses only deterministic, nonsensitive fixture data.
    const timingSource = join(dirname(source), "timing-fixture.tmcap");
    const perf = {
      points: [
        { milestone: "request-headers", unixMillis: 1800000000000, offsetMicros: 0 },
        { milestone: "exchange-done", unixMillis: 1800000000004, offsetMicros: 4000 },
      ],
      protocols: [],
      transports: [
        {
          leg: "upstream",
          connectionId: "native-tcp-stats",
          shared: true,
          outcome: "connected",
          sampledOffsetMicros: 3500,
          bytesRead: 8192,
          bytesWritten: 4096,
          tcpSampledOffsetMicros: 3000,
          tcp: {
            rttMicros: 2400,
            congestionWindow: 65536,
            sendWindow: 32768,
            retransmittedBytes: 2048,
          },
          socketIo: {
            writeWaitMicros: 1750,
            writeWaits: 2,
            lastWriteOffsetMicros: 2800,
            lastFlushOffsetMicros: 2900,
          },
        },
      ],
      work: [
        {
          kind: "hook",
          label: "Request headers · native support script",
          beganOffsetMicros: 0,
          endedOffsetMicros: 1000,
          busyNanos: 1000000,
          calls: 1,
        },
        {
          kind: "transform",
          label: "Response body · native support script",
          beganOffsetMicros: 1200,
          endedOffsetMicros: 3500,
          busyNanos: 500000,
          calls: 3,
        },
      ],
    };
    const fixture = [
      {
        kind: "incident-marker",
        exchange_id: 0,
        sequence: 0,
        payload: {
          version: 1,
          id: "1".repeat(32),
          runId: "2".repeat(32),
          unixMillis: 1800000000000,
          offsetMillis: 1,
          origin: "cli",
          creationKind: "user-mark",
          note: "Native issue <img src=x>",
        },
      },
      {
        kind: "request-head",
        payload: {
          boundary: "client-request",
          method: "GET",
          target: "https://example.invalid/native-waterfall",
          headers: [],
        },
      },
      { kind: "response-head", payload: { boundary: "client-response", status: 200, headers: [] } },
      {
        kind: "body-segment",
        payload: { boundary: "client-response", byte_count: 0, bytes: [], truncated: false },
      },
      { kind: "performance", payload: perf },
      { kind: "completed", payload: null },
    ];
    const responseHash = createHash("sha256").update("body").digest("hex");
    Object.assign(fixture.find((row) => row.kind === "body-segment").payload, {
      byte_count: 4,
      bytes: [98, 111, 100, 121],
    });
    fixture.splice(fixture.length - 1, 0, {
      kind: "hook-effect",
      payload: {
        hook_id: "automation/native-file@4/request/asset/native-style@2",
        hook_name: "Native linked-file experiment",
        phase: "request-head",
        action: "respond",
        changed: true,
        chain_position: 0,
        effect_sequence: 1,
        header_edit: null,
        response_snapshot: { status: 200, bodyBytes: 4, sha256: responseHash, contentCodings: [] },
      },
    });
    fixture.splice(fixture.length - 1, 0, {
      kind: "hook-effect",
      payload: {
        hook_id: "automation/historical-header@7/response",
        hook_name: "Historical response policy",
        phase: "response-head",
        action: "replace-response-head",
        changed: true,
        chain_position: 1,
        effect_sequence: 2,
        header_edit: {
          kind: "response",
          changes: {
            statusChanged: false,
            headers: { added: ["X-Historical"], removed: ["Content-Security-Policy"], changed: [] },
          },
        },
        response_snapshot: null,
      },
    });
    fixture.splice(fixture.length - 1, 0, {
      kind: "hook-effect",
      payload: {
        hook_id: "automation/historical-text@8/response",
        hook_name: "Historical literal edit",
        phase: "response-body",
        action: "body-edit",
        changed: false,
        chain_position: 2,
        effect_sequence: 3,
        header_edit: null,
        body_edit: {
          outcome: "not-found",
          matches: 0,
          replacements: 0,
          beforeBytes: 4,
          afterBytes: 4,
          beforeSha256: responseHash,
          afterSha256: responseHash,
        },
        response_snapshot: null,
      },
    });
    await writeFile(timingSource, nativeFixture(fixture, 99));
    await viewer.evaluate(`void ${traffic}.importTrace(${JSON.stringify(timingSource)})`);
    await waitFor(async () => {
      const state = await viewer.evaluate(
        `({busy:${traffic}.importingTrace,found:${traffic}.sessions.some(row=>row.path==='/native-waterfall'),status:${traffic}.importStatus})`,
      );
      if (!state.busy) {
        assert.ok(state.found, "Native timing fixture import failed: " + state.status);
        return true;
      }
      return false;
    }, "Native timing fixture did not import");
    await smokeIncidentIntake(
      viewer,
      traffic,
      await viewer.evaluate(
        `${traffic}.sessions.find(row=>row.path==='/native-waterfall').traceId`,
      ),
      screenshot,
    );
    const timingId = await viewer.evaluate(
      `(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/native-waterfall');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`,
    );
    await viewer.evaluate(`${traffic}.showTimings()`);
    await waitFor(
      () =>
        viewer.evaluate(
          `!${traffic}.timingBusy && ${traffic}.timingWaterfall.length+${traffic}.timingMinor.length>=3`,
        ),
      "Native timing waterfall was not restored",
    );
    assert.equal(
      await viewer.evaluate(`${traffic}.bodyEditRows.length`),
      1,
      "Native body edit outcome was lost",
    );
    assert.ok(
      await viewer.evaluate(
        `${traffic}.timingReportText.includes('Not applied: text not found')&&${traffic}.timingReportText.includes('historical-text@8')`,
      ),
    );
    assert.equal(
      await viewer.evaluate(`${traffic}.headerEffectRows.length`),
      1,
      "Native historical header evidence was lost",
    );
    assert.ok(
      await viewer.evaluate(
        `${traffic}.timingReportText.includes('historical-header@7')&&${traffic}.timingReportText.includes('X-Historical')`,
      ),
    );
    assert.equal(
      await viewer.evaluate(
        `Array.from(${traffic}.timingDialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent.includes('Proxy operations under 1 ms'))?.open`,
      ),
      false,
    );
    await viewer.evaluate(
      `Array.from(${traffic}.timingDialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent.includes('Proxy operations under 1 ms')).open=true`,
    );
    const chart = await viewer.evaluate(
      `(()=>{const workspace=${traffic};const bars=Array.from(workspace.timingDialog.querySelectorAll('.waterfall-bar'));return {rows:workspace.timingWaterfall.length+workspace.timingMinor.length,svg:bars.every(node=>node.namespaceURI==='http://www.w3.org/2000/svg'),painted:bars.every(node=>node.getBoundingClientRect().width>0),report:workspace.timingReportText.includes('native support script'),footer:workspace.timingDialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight};})()`,
    );
    assert.ok(
      chart.rows >= 3 && chart.svg && chart.painted && chart.report && chart.footer,
      "Native timing waterfall failed to render or left its actions unreachable: " +
        JSON.stringify(chart),
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", "-waterfall.png"),
        Buffer.from(image.data, "base64"),
      );
    }
    await viewer.evaluate(
      `(()=>{const dialog=${traffic}.timingDialog;const section=Array.from(dialog.querySelectorAll('details')).find(node=>node.querySelector('summary')?.textContent==='Proxy ↔ upstream');section.open=true;const label=Array.from(section.querySelectorAll('dt')).find(node=>node.textContent==='TCP estimated round trip');label.scrollIntoView({block:'center'});})()`,
    );
    assert.ok(
      await viewer.evaluate(
        `${traffic}.timingReportText.includes('TCP estimated round trip: 2.4 ms')`,
      ),
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(screenshot.replace(".png", "-tcp.png"), Buffer.from(image.data, "base64"));
    }
    await viewer.evaluate(
      `(()=>{const workspace=${traffic};workspace.showDetailsTab('timings');const evidence=workspace.timingDialog.querySelector('.response-file-evidence');evidence.querySelector('summary').click();evidence.scrollIntoView({block:'start'});})()`,
    );
    assert.ok(
      await viewer.evaluate(
        `${traffic}.timingDialog.querySelector('.response-file-evidence').textContent.includes('Complete response bytes are retained.')`,
      ),
    );
    assert.equal(
      await viewer.evaluate(
        `${traffic}.responseEvidenceRows.find(row=>row.id==='selected-hash').value`,
      ),
      responseHash,
    );
    const retainedBody = await viewer.evaluate(
      `window.__TAURI_INTERNALS__.invoke('inspect_body',{request:{sessionId:${JSON.stringify(timingId)},boundary:'client-response',representation:'original-text',decodeContent:true,maxBytes:1024}})`,
    );
    assert.equal(retainedBody.display, "body");
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", "-file-snapshot.png"),
        Buffer.from(image.data, "base64"),
      );
    }
    await viewer.call("Emulation.setDeviceMetricsOverride", {
      width: 800,
      height: 600,
      deviceScaleFactor: 1,
      mobile: false,
    });
    assert.ok(
      await viewer.evaluate(
        `(()=>{const dialog=${traffic}.timingDialog,close=dialog.querySelector('footer').getBoundingClientRect();return dialog.getBoundingClientRect().height<=innerHeight&&close.bottom<=innerHeight;})()`,
      ),
    );
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(
        screenshot.replace(".png", "-file-snapshot-compact.png"),
        Buffer.from(image.data, "base64"),
      );
    }
    await viewer.call("Emulation.clearDeviceMetricsOverride");
    await viewer.evaluate(
      `(async()=>{const workspace=${traffic};workspace.closeTimings();await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(timingId)}],restore:false});await workspace.refreshSessions(undefined,true);})()`,
    );

    await viewer.evaluate(`${traffic}.closeTimings()`);
    // Real IPC and indexed TMCap pages must reach beyond the bounded preview limit.
    await smokeCertificates(viewer, traffic, source, nativeFixture, screenshot);
    const byteSource = join(dirname(source), "large-byte-fixture.tmcap"),
      byteLength = 32 * 1024 * 1024 + 17;
    const byteChunk = Array.from({ length: 1024 * 1024 }, (_, index) => index % 256);
    const byteRecords = [
      {
        kind: "request-head",
        payload: {
          boundary: "client-request",
          method: "GET",
          target: "https://example.invalid/large-byte-body",
          headers: [],
        },
      },
      {
        kind: "response-head",
        payload: {
          boundary: "client-response",
          status: 200,
          headers: [
            {
              name: Array.from(Buffer.from("Content-Type")),
              value: Array.from(Buffer.from("application/octet-stream")),
            },
          ],
        },
      },
    ];
    for (let offset = 0; offset < byteLength; offset += byteChunk.length) {
      const bytes = byteChunk.slice(0, Math.min(byteChunk.length, byteLength - offset));
      byteRecords.push({
        kind: "body-segment",
        payload: { boundary: "client-response", byte_count: bytes.length, bytes, truncated: false },
      });
    }
    byteRecords.push({ kind: "completed", payload: null });
    await writeFile(byteSource, nativeFixture(byteRecords, 103));
    await viewer.evaluate(`void ${traffic}.importTrace(${JSON.stringify(byteSource)})`);
    await waitFor(
      () => viewer.evaluate(`!${traffic}.importingTrace`),
      "Large byte fixture import did not finish",
    );
    assert.ok(
      await viewer.evaluate(`${traffic}.sessions.some(row=>row.path==='/large-byte-body')`),
      "Large byte import failed: " + (await viewer.evaluate(`${traffic}.importStatus`)),
    );
    const byteId = await viewer.evaluate(
      `(async()=>{const workspace=${traffic},row=workspace.sessions.find(row=>row.path==='/large-byte-body');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`,
    );
    const responseInspector =
      root + '.shadowRoot.querySelector("message-inspector[side=response]")';
    try {
      await waitFor(
        () =>
          viewer.evaluate(
            `${responseInspector}.detail?.id===${JSON.stringify(byteId)} && ${responseInspector}.pagedHex && !${responseInspector}.loading`,
          ),
        "Large raw Hex did not load",
      );
    } catch (error) {
      const state = await viewer.evaluate(
        `({id:${responseInspector}.detail?.id,mode:${responseInspector}.mode,viewer:${responseInspector}.viewer,bodies:${responseInspector}.detail?.storedBodies,text:${responseInspector}.bodyText,facts:${responseInspector}.bodyFacts,loading:${responseInspector}.loading})`,
      );
      throw new Error("Large raw Hex inspection: " + JSON.stringify(state), { cause: error });
    }
    const byteKey = async (key, code, modifiers = 0) => {
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyDown",
        key,
        windowsVirtualKeyCode: code,
        modifiers,
      });
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyUp",
        key,
        windowsVirtualKeyCode: code,
        modifiers,
      });
    };
    try {
      await viewer.evaluate(
        `(()=>{const inspector=${responseInspector};globalThis.__largeByteProbe={viewer:inspector.querySelector('hex-viewer'),clipboard:Object.getOwnPropertyDescriptor(navigator,'clipboard')};Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>globalThis.__largeByteProbe.copied=text}});globalThis.__largeByteProbe.viewer.viewport.focus();})()`,
      );
      await byteKey("End", 35, 2);
      await waitFor(
        () =>
          viewer.evaluate(
            `globalThis.__largeByteProbe.viewer.querySelector('[data-byte="${byteLength - 1}"][data-column=hex]')?.textContent==='10'`,
          ),
        "Far native byte page did not load",
      );
      await waitFor(
        () =>
          viewer.evaluate(
            `!${responseInspector}.loading&&${responseInspector}.bodyFacts.includes('All retained bytes available')`,
          ),
        "Native paged Hex summary did not finish updating",
      );
      const byteView = await viewer.evaluate(
        `(()=>{const v=globalThis.__largeByteProbe.viewer;return {caret:v.caret,rows:v.hexRows.length,height:v.viewport.scrollHeight,full:v.preview.source.totalBytes,facts:${responseInspector}.bodyFacts};})()`,
      );
      assert.equal(byteView.caret, String(byteLength - 1));
      assert.equal(byteView.full, String(byteLength));
      assert.ok(byteView.rows < 100 && byteView.height <= 4_000_100);
      assert.doesNotMatch(byteView.facts, /preview truncated/);
      await byteKey("ArrowLeft", 37, 8);
      await byteKey("c", 67, 2);
      await waitFor(
        () => viewer.evaluate(`globalThis.__largeByteProbe.copied==='DxA='`),
        "Native paged selection did not copy exact tail bytes",
      );
      await byteKey("g", 71, 2);
      assert.equal(
        await viewer.evaluate("globalThis.__largeByteProbe.viewer.offsetField.value"),
        String(byteLength - 2),
      );
      await byteKey("Escape", 27);
      assert.ok(
        await viewer.evaluate(
          "globalThis.__largeByteProbe.viewer.viewport===globalThis.__largeByteProbe.viewer.getRootNode().activeElement",
        ),
      );
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-hex-pages.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.call("Emulation.setDeviceMetricsOverride", {
        width: 640,
        height: 720,
        deviceScaleFactor: 1,
        mobile: false,
      });
      await waitFor(
        () => viewer.evaluate("innerWidth===640"),
        "Compact byte inspection did not resize",
      );
      const messageSwitch = await viewer.evaluate(
        `(()=>{const workspace=${traffic},toggle=workspace.querySelector('.compact-inspector-side');toggle.querySelector('[value=request]').click();const request=workspace.inspectorSide==='request';toggle.querySelector('[value=response]').click();globalThis.__largeByteProbe.viewer.viewport.focus();return request&&workspace.inspectorSide==='response';})()`,
      );
      assert.ok(messageSwitch, "Compact message pill did not switch request and response");
      await viewer.evaluate(
        `${traffic}.querySelector('.compact-inspector-side [value=response]').focus()`,
      );
      await byteKey("ArrowLeft", 37);
      assert.ok(
        await viewer.evaluate(
          `(()=>{const t=${traffic};return t.querySelector('.compact-inspector-side [value=request]').checked&&getComputedStyle(t.querySelector('message-inspector[side=request]')).display!=='none'&&getComputedStyle(t.querySelector('message-inspector[side=response]')).display==='none';})()`,
        ),
        "Native arrow key did not select and reveal Request",
      );
      await byteKey("ArrowRight", 39);
      assert.ok(
        await viewer.evaluate(
          `${traffic}.querySelector('.compact-inspector-side [value=response]').checked`,
        ),
        "Native arrow key did not select Response",
      );
      await viewer.evaluate("globalThis.__largeByteProbe.viewer.viewport.focus()");
      await byteKey("End", 35, 2);
      const saveButton = await viewer.evaluate(
        `(()=>{const b=${responseInspector}.querySelector('.body-toolbar>button');b.focus();const r=b.getBoundingClientRect(),c=b.closest('.message-content').getBoundingClientRect();return {visible:r.top>=c.top&&r.bottom<=c.bottom&&r.bottom<=innerHeight,enabled:!b.disabled};})()`,
      );
      assert.ok(
        saveButton.visible && saveButton.enabled,
        "Compact response save action became unreachable",
      );
      await viewer.evaluate(
        'globalThis.__largeByteProbe.viewer.querySelector(".hex-status button").focus()',
      );
      const reachability = await viewer.evaluate(
        `(${hexReachability.toString()})(globalThis.__largeByteProbe.viewer,${JSON.stringify(String(byteLength - 1))})`,
      );
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(
            ".png",
            reachability.tailVisible ? "-hex-pages-compact.png" : "-hex-failure.png",
          ),
          Buffer.from(image.data, "base64"),
        );
      }
      assert.ok(
        reachability.visible && reachability.rows < 100 && reachability.tailVisible,
        "Compact Hex bytes or actions became unreachable: " + JSON.stringify(reachability),
      );
      await viewer.evaluate(
        'globalThis.__largeByteProbe.viewer.querySelector(".hex-status button").click()',
      );
      assert.equal(
        await viewer.evaluate("globalThis.__largeByteProbe.viewer.offsetField.value"),
        String(byteLength - 1),
      );
      await byteKey("Escape", 27);
      assert.ok(
        await viewer.evaluate(
          "globalThis.__largeByteProbe.viewer.viewport===globalThis.__largeByteProbe.viewer.getRootNode().activeElement",
        ),
      );
      const inspectorLayout = await viewer.evaluate(
        `(()=>{const t=${traffic};return {preferences:structuredClone(t.preferences),method:t.selectedMethodText,status:t.selectedStatusText};})()`,
      );
      try {
        await viewer.call("Emulation.setDeviceMetricsOverride", {
          width: 1440,
          height: 900,
          deviceScaleFactor: 1,
          mobile: false,
        });
        await viewer.evaluate(
          `(()=>{const t=${traffic};t.preferences={...t.preferences,layout:'side-by-side',listSplit:75};t.selectedMethodText='PROPPATCH';t.selectedStatusText='No response';})()`,
        );
        const layout = await viewer.evaluate(
          `(()=>{const t=${traffic},pane=t.querySelector('.details-pane').getBoundingClientRect(),url=t.querySelector('.selected-url').getBoundingClientRect(),pill=t.querySelector('.compact-inspector-side').getBoundingClientRect();return {urlWidth:url.width,pillVisible:pill.left>=pane.left&&pill.right<=pane.right};})()`,
        );
        assert.ok(
          layout.urlWidth >= 48 && layout.pillVisible,
          "Native minimum-width inspector hid its URL or message pill: " + JSON.stringify(layout),
        );
        await viewer.evaluate(`${traffic}.querySelector('.selected-url').click()`);
        assert.ok(
          await viewer.evaluate(
            `${traffic}.querySelector('#selected-url-full').matches(':popover-open')`,
          ),
          "Native minimum-width URL did not open",
        );
        await byteKey("Escape", 27);
        if (screenshot) {
          const image = await viewer.call("Page.captureScreenshot", { format: "png" });
          await writeFile(
            screenshot.replace(".png", "-minimum-inspector.png"),
            Buffer.from(image.data, "base64"),
          );
        }
      } finally {
        await viewer.evaluate(
          `(()=>{const t=${traffic},saved=${JSON.stringify(inspectorLayout)};t.preferences=saved.preferences;t.selectedMethodText=saved.method;t.selectedStatusText=saved.status;})()`,
        );
        await viewer.call("Emulation.setDeviceMetricsOverride", {
          width: 640,
          height: 720,
          deviceScaleFactor: 1,
          mobile: false,
        });
        await viewer.evaluate("globalThis.__largeByteProbe.viewer.viewport.focus()");
      }
      process.stdout.write(
        "Native paged Hex, full-body end navigation, byte selection, clipboard and compact layout verified.\n",
      );
    } finally {
      await viewer.call("Emulation.clearDeviceMetricsOverride");
      await viewer.evaluate(
        `(()=>{const saved=globalThis.__largeByteProbe;if(saved?.clipboard)Object.defineProperty(navigator,'clipboard',saved.clipboard);else delete navigator.clipboard;delete globalThis.__largeByteProbe;})()`,
      );
      await viewer.evaluate(
        `(async()=>{await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(byteId)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`,
      );
    }
    // Exercise a real viewer Composer upload. The only contacted endpoint is
    // this owned loopback fixture; no captured URL or outside service is replayed.
    const replayLength = 5 * 1024 * 1024 + 7;
    let replayBytes = 0,
      replayError = "";
    const composerImage = Buffer.from(
      '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="green"/></svg>',
    );
    const composerBinary = Buffer.alloc(1024 * 1024 + 17);
    for (let index = 0; index < composerBinary.length; index++) composerBinary[index] = index % 256;
    const replayServer = createServer(async (request, response) => {
      if (request.url === "/composer-image.svg") {
        request.resume();
        response.setHeader("Content-Type", "image/svg+xml");
        response.end(composerImage);
        return;
      }
      if (request.url === "/composer-binary.bin") {
        request.resume();
        response.setHeader("Content-Type", "application/octet-stream");
        response.end(composerBinary);
        return;
      }
      try {
        for await (const chunk of request) {
          replayBytes += chunk.length;
          if (!chunk.every((byte) => byte === 7)) throw new Error("Captured replay bytes changed");
        }
        if (replayBytes !== replayLength) throw new Error("Captured replay was incomplete");
        response.end("received " + replayBytes);
      } catch (error) {
        replayError = String(error);
        response.statusCode = 500;
        response.end("fixture upload failed");
      }
    });
    await new Promise((resolve) => replayServer.listen(0, "127.0.0.1", resolve));
    try {
      const url = "http://127.0.0.1:" + replayServer.address().port + "/native-replay";
      const records = [
        {
          kind: "request-head",
          payload: {
            boundary: "client-request",
            method: "POST",
            target: url,
            headers: [
              {
                name: Array.from(Buffer.from("Content-Length")),
                value: Array.from(Buffer.from(String(replayLength))),
              },
            ],
          },
        },
        {
          kind: "response-head",
          payload: { boundary: "client-response", status: 200, headers: [] },
        },
      ];
      for (let remaining = replayLength; remaining > 0;) {
        const count = Math.min(remaining, 512 * 1024);
        records.push({
          kind: "body-segment",
          payload: {
            boundary: "client-request",
            byte_count: count,
            bytes: Array(count).fill(7),
            truncated: false,
          },
        });
        remaining -= count;
      }
      records.push(
        {
          kind: "body-segment",
          payload: { boundary: "client-response", byte_count: 0, bytes: [], truncated: false },
        },
        { kind: "completed", payload: null },
      );
      const replaySource = join(dirname(source), "replay-fixture.tmcap");
      await writeFile(replaySource, nativeFixture(records, 101));
      await viewer.evaluate(`${traffic}.importTrace(${JSON.stringify(replaySource)})`);
      const replayImportStatus = await viewer.evaluate(`${traffic}.importStatus`);
      assert.ok(!replayImportStatus.startsWith("Import failed"), replayImportStatus);
      await waitFor(
        () =>
          viewer.evaluate(
            `!${traffic}.importingTrace && ${traffic}.sessions.some(row=>row.path==='/native-replay')`,
          ),
        "Native replay fixture did not import",
      );
      const replayId = await viewer.evaluate(
        `(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/native-replay');await workspace.selectTraffic(row,new MouseEvent('click'));await workspace.replaySelected();return row.id;})()`,
      );
      const composer = root + '.shadowRoot.querySelector("composer-workspace")';
      await waitFor(
        () =>
          viewer.evaluate(
            `${composer}?.composerBodyMode==='captured' && !${composer}.composerBodyMissing`,
          ),
        "Native captured body did not load as a streamed Composer draft",
      );
      assert.equal(
        await viewer.evaluate(`${composer}.input('body').value.length`),
        0,
        "Large captured body entered the text editor",
      );
      await viewer.evaluate(
        `(async()=>{const workspace=${composer};workspace.input('nonIdempotent').checked=true;await workspace.executeComposer(new Event('submit',{cancelable:true}));})()`,
      );
      assert.equal(replayBytes, replayLength);
      assert.equal(replayError, "");
      const replayResult = await viewer.evaluate(
        `({status:${composer}.composerResult?.status,body:${composer}.composerResult?.body,source:${composer}.composerResult?.source,error:${composer}.composerError})`,
      );
      assert.equal(replayResult.error, "");
      assert.equal(replayResult.status, 200);
      assert.equal(replayResult.body, "received " + replayLength);
      assert.equal(replayResult.source.entryId, replayId);
      assert.ok(replayResult.source.traceId);
      const composerResponse = composer + '.querySelector("message-inspector")';
      await waitFor(
        () => viewer.evaluate(`${composerResponse}?.bodyText==='received ${replayLength}'`),
        "Composer text did not use the shared message inspector",
      );
      const composerFixtureUrl = "http://127.0.0.1:" + replayServer.address().port;
      await viewer.evaluate(
        `(async()=>{const workspace=${composer};workspace.input('method').value='GET';workspace.input('url').value=${JSON.stringify(composerFixtureUrl + "/composer-image.svg")};workspace.input('headers').value='';workspace.input('bodyHex').checked=false;workspace.useEmptyBody();workspace.updateRisk();await workspace.executeComposer(new Event('submit',{cancelable:true}));})()`,
      );
      await waitFor(
        () => viewer.evaluate(`${composerResponse}.querySelector('img')?.naturalWidth===64`),
        "Composer safe image preview did not render through the native preview worker",
      );
      const imageResult = await viewer.evaluate(
        `({handle:${composerResponse}.imageUrl,facts:${composerResponse}.bodyFacts,status:${composer}.composerResult?.status,error:${composer}.composerError,protocol:${composer}.composerResult?.detail.responses[0].protocol})`,
      );
      assert.equal(imageResult.error, "");
      assert.equal(imageResult.status, 200);
      assert.match(imageResult.handle, /transmog-preview/);
      assert.match(imageResult.facts, /image\/svg\+xml/);
      assert.equal(imageResult.protocol, "HTTP/1.1");
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-composer-image.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.evaluate(
        `(async()=>{const workspace=${composer};workspace.input('url').value=${JSON.stringify(composerFixtureUrl + "/composer-binary.bin")};await workspace.executeComposer(new Event('submit',{cancelable:true}));})()`,
      );
      await waitFor(
        () => viewer.evaluate(`${composerResponse}.pagedHex && !${composerResponse}.loading`),
        "Composer binary response did not use paged Hex",
      );
      await viewer.evaluate(`${composerResponse}.querySelector('hex-viewer').viewport.focus()`);
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyDown",
        key: "End",
        windowsVirtualKeyCode: 35,
        modifiers: 2,
      });
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyUp",
        key: "End",
        windowsVirtualKeyCode: 35,
        modifiers: 2,
      });
      await waitFor(
        () =>
          viewer.evaluate(
            `${composerResponse}.querySelector('[data-byte="${composerBinary.length - 1}"][data-column=hex]')?.textContent==='10'`,
          ),
        "Composer did not expose bytes beyond its legacy text preview",
      );
      const binaryResult = await viewer.evaluate(
        `({total:${composerResponse}.hexPreview.source.totalBytes,rows:${composerResponse}.querySelectorAll('.hex-data-row').length,legacyTruncated:${composer}.composerResult.truncated,text:${composer}.composerText,detail:${composer}.composerResult.detail.id})`,
      );
      assert.equal(binaryResult.total, String(composerBinary.length));
      assert.ok(binaryResult.rows < 100 && binaryResult.legacyTruncated);
      assert.doesNotMatch(binaryResult.text, /truncated/);
      const bytePage = await viewer.evaluate(
        `(async()=>{const buffer=await window.__TAURI_INTERNALS__.invoke('read_body_range',{request:{sessionId:${JSON.stringify(binaryResult.detail)},boundary:'client-response',offset:'1048576',maxBytes:17}});if(!(buffer instanceof ArrayBuffer))throw new Error('Body IPC did not return an ArrayBuffer');const length=new DataView(buffer).getUint32(0,true);return {header:JSON.parse(new TextDecoder().decode(new Uint8Array(buffer,4,length))),bytes:Array.from(new Uint8Array(buffer,4+length))};})()`,
      );
      assert.equal(bytePage.header.offset, "1048576");
      assert.equal(bytePage.header.totalBytes, String(composerBinary.length));
      assert.deepEqual(Buffer.from(bytePage.bytes), composerBinary.subarray(1048576));
      process.stdout.write(
        "Native Composer shared text, safe image preview and complete paged binary response verified.\n",
      );
      await viewer.evaluate(
        `(async()=>{const workspace=${composer};workspace.composerPane='request';const details=workspace.shadowRoot?.querySelector('.composer-history')??workspace.querySelector('.composer-history');details.open=true;await workspace.refreshHistory();})()`,
      );
      await waitFor(
        () =>
          viewer.evaluate(
            `${composer}.composerHistory.length>0 && !${composer}.composerHistoryBusy`,
          ),
        "Latest Composer history refresh did not complete",
      );
      assert.equal(await viewer.evaluate(`${composer}.composerHistory[0].sourceId`), replayId);
      assert.ok(
        await viewer.evaluate(
          `${composer}.composerHistory[0].sourceLabel.startsWith('replay-fixture.tmcap')`,
        ),
      );
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-composer.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.call("Emulation.setDeviceMetricsOverride", {
        width: 760,
        height: 520,
        deviceScaleFactor: 1,
        mobile: false,
      });
      assert.ok(
        await viewer.evaluate(`${composer}.composerForm.getBoundingClientRect().width<760`),
      );
      await viewer.evaluate(`${composer}.composerPane='response'`);
      await waitFor(
        () =>
          viewer.evaluate(
            `${composerResponse}.querySelector('hex-viewer').viewport.clientHeight>40`,
          ),
        "Compact Composer hid its byte rows",
      );
      await viewer.evaluate(`${composerResponse}.querySelector('hex-viewer').viewport.focus()`);
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyDown",
        key: "End",
        windowsVirtualKeyCode: 35,
        modifiers: 2,
      });
      await viewer.call("Input.dispatchKeyEvent", {
        type: "keyUp",
        key: "End",
        windowsVirtualKeyCode: 35,
        modifiers: 2,
      });
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-composer-compact.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.call("Emulation.clearDeviceMetricsOverride");
      await viewer.evaluate(`${composer}.composerPane='request'`);
      await viewer.evaluate(
        `(async()=>{await ${root}.activateView('traffic');const workspace=${traffic};workspace.searchInput.value='Content-Length';workspace.searchMetadata=false;workspace.searchRequestHeaders=true;workspace.searchResponseHeaders=false;workspace.searchBodies=false;workspace.searchRequestBodies=false;workspace.selectSearchMatches=true;await workspace.runContentSearch();})()`,
      );
      assert.ok(
        await viewer.evaluate(`${traffic}.searchMatchIds.includes(${JSON.stringify(replayId)})`),
        "Native scoped search did not find the retained request header",
      );
      await viewer.evaluate(`${traffic}.showMatches()`);
      assert.equal(await viewer.evaluate(`${traffic}.matchText`), "Content-Length");
      assert.ok(
        await viewer.evaluate(
          `${traffic}.matchField.startsWith('Client request') && ${traffic}.matchDialog.querySelector('mark').textContent==='Content-Length'`,
        ),
      );
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-matches.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.call("Emulation.setDeviceMetricsOverride", {
        width: 760,
        height: 520,
        deviceScaleFactor: 1,
        mobile: false,
      });
      assert.ok(
        await viewer.evaluate(
          `(()=>{const dialog=${traffic}.matchDialog;return dialog.getBoundingClientRect().width<=760&&dialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight;})()`,
        ),
      );
      if (screenshot) {
        const image = await viewer.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-matches-compact.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await viewer.call("Emulation.clearDeviceMetricsOverride");
      await viewer.evaluate(
        `(async()=>{${traffic}.closeMatches();await ${traffic}.clearContentSearch();})()`,
      );
      await viewer.evaluate(
        `(async()=>{await ${root}.activateView('traffic');await window.__TAURI_INTERNALS__.invoke('remove_traffic_entries',{ids:[${JSON.stringify(replayId)}],restore:false});await ${traffic}.refreshSessions(undefined,true);})()`,
      );
      process.stdout.write(
        "Viewer streamed captured binary replay, original trace association and history verified.\n",
      );
    } finally {
      replayServer.closeAllConnections();
      await new Promise((resolve) => replayServer.close(resolve));
    }

    await viewer.evaluate(`${traffic}.showSaveTrace()`);
    const saveChoices = await viewer.evaluate(
      `({open:${traffic}.saveTraceDialog.open,network:${traffic}.saveTraceForm.elements.namedItem('networkContext').checked,redact:${traffic}.saveTraceForm.elements.namedItem('redactHeaders').checked})`,
    );
    assert.ok(saveChoices.open);
    assert.equal(saveChoices.network, false);
    assert.equal(saveChoices.redact, false);
    if (screenshot) {
      const image = await viewer.call("Page.captureScreenshot", { format: "png" });
      await writeFile(screenshot.replace(".png", "-save.png"), Buffer.from(image.data, "base64"));
    }
    await viewer.evaluate(`${traffic}.closeSaveTrace()`);
    const before = (await targets()).map((target) => target.id);
    await main.evaluate(
      `window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(source)}]})`,
    );
    const additional = await waitFor(async () => {
      const all = await targets();
      return all.find((target) => !before.includes(target.id));
    }, "Additional viewer did not open");
    const second = await connect(additional);
    await waitFor(
      () =>
        second.evaluate(
          `${root}?.viewerMode && ${traffic}?.sessions.length===1 && !${traffic}.importingTrace`,
        ),
      "Additional viewer did not load its saved capture",
    );
    if (trafficSource) {
      const existing = (await targets()).map((target) => target.id);
      await main.evaluate(
        `window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(trafficSource)}]})`,
      );
      const target = await waitFor(async () => {
        const all = await targets();
        return all.find((target) => !existing.includes(target.id));
      }, "Large traffic viewer did not open");
      const large = await connect(target);
      await smokeTrafficList(large, screenshot);
    }
    if (nativeSource) {
      const existing = (await targets()).map((target) => target.id);
      await main.evaluate(
        `window.__TAURI_INTERNALS__.invoke('open_trace_viewer',{paths:[${JSON.stringify(nativeSource)}]})`,
      );
      const target = await waitFor(async () => {
        const all = await targets();
        return all.find((target) => !existing.includes(target.id));
      }, "Native capture viewer did not open");
      const capture = await connect(target);
      await waitFor(
        () =>
          capture.evaluate(
            `${root}?.viewerMode && ${traffic}?.sessions.length>=1 && !${traffic}.importingTrace`,
          ),
        "Native capture did not import",
      );
      const htmlId = await capture.evaluate(
        `(()=>{const rows=${traffic}.sessions.filter(row=>row.status===200&&row.contentType?.includes('text/html'));rows.sort((a,b)=>(b.responseBytes??0)-(a.responseBytes??0));return rows[0]?.id;})()`,
      );
      if (htmlId) {
        const beforePreview = (await targets()).map((row) => row.id);
        const label = await capture.evaluate(
          `window.__TAURI_INTERNALS__.invoke('open_captured_page',{id:${JSON.stringify(htmlId)},enableScripts:false,options:{scope:'original-trace'},operationId:'external-page-probe'})`,
        );
        const previewTarget = await waitFor(async () => {
          const all = await targets();
          return all.find((row) => !beforePreview.includes(row.id));
        }, "Saved HTML preview did not open");
        const preview = await connect(previewTarget);
        await waitFor(
          () =>
            preview.evaluate(
              "document.readyState==='complete' && document.body?.innerText.length>0",
            ),
          "Saved HTML preview did not render",
        );
        const report = await capture.evaluate(
          `window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(label)}})`,
        );
        const sheets = await preview.evaluate(
          "Array.from(document.querySelectorAll('link[rel=stylesheet]')).map(link=>({url:link.href,loaded:!!link.sheet}))",
        );
        for (const sheet of sheets) {
          if (report.resources?.some((row) => row.url === sheet.url && row.bytes !== null))
            assert.ok(sheet.loaded, "Captured stylesheet did not load");
        }
        await preview.call("Page.reload", { ignoreCache: true });
        await waitFor(
          () =>
            preview.evaluate(
              "document.readyState==='complete' && document.body?.innerText.length>0",
            ),
          "Saved HTML reload did not render",
        );
        for (const sheet of sheets.filter((row) => row.loaded))
          assert.ok(
            await preview.evaluate(
              `Array.from(document.querySelectorAll('link[rel=stylesheet]')).some(link=>link.href===${JSON.stringify(sheet.url)}&&!!link.sheet)`,
            ),
            "Saved stylesheet did not survive reload",
          );
        if (screenshot) {
          const image = await preview.call("Page.captureScreenshot", { format: "png" });
          await writeFile(
            screenshot.replace(".png", "-external-page.png"),
            Buffer.from(image.data, "base64"),
          );
        }
        await closeNative("Baseline preview · Scripts off — Transmog");
        process.stdout.write("Saved HTML preview, captured stylesheets and reload verified.\n");
      }
    }
    if (pageSource) {
      await second.evaluate(`${traffic}.importTrace(${JSON.stringify(pageSource)})`);
      await waitFor(
        () =>
          second.evaluate(
            `${traffic}.sessions.some(row=>row.path==='/captured-page') && !${traffic}.importingTrace`,
          ),
        "HTML fixture import did not finish",
      );
      const navigation = await second.evaluate(
        `(()=>{const workspace=${traffic};return {pages:workspace.sessions.filter(row=>row.topLevelNavigation).map(row=>row.path),badges:workspace.getRootNode().querySelectorAll('.navigation-badge').length};})()`,
      );
      assert.deepEqual(navigation, { pages: ["/captured-page"], badges: 1 });
      if (screenshot) {
        const image = await second.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-navigation.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await second.evaluate(
        `(()=>{const root=${traffic}.getRootNode();${traffic}.openFilterPicker('fetch-destination');root.getElementById('filter-option-text:document').click();root.getElementById('filter-option-text:style').click();root.querySelector('#traffic-filter-picker button[type=submit]').click();})()`,
      );
      await waitFor(
        () =>
          second.evaluate(
            `${traffic}.sessions.length===2 && ${traffic}.sessions.every(row=>['document','style'].includes(row.fetchDestination))`,
          ),
        "Native multi-destination filter did not select document and stylesheet",
      );
      assert.equal(
        await second.evaluate(
          `${traffic}.filterGroups.find(group=>group.dimension==='fetch-destination').values.length`,
        ),
        2,
      );
      if (screenshot) {
        const image = await second.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-destinations.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await second.evaluate(
        `(async()=>{const root=${traffic}.getRootNode();await ${traffic}.clearFetchDestinations();})()`,
      );
      await waitFor(
        () => second.evaluate(`${traffic}.sessions.some(row=>row.path==='/captured-frame')`),
        "Clearing the native filter did not restore iframe traffic",
      );
      const htmlId = await second.evaluate(
        `(async()=>{const workspace=${traffic};const row=workspace.sessions.find(row=>row.path==='/captured-page');await workspace.selectTraffic(row,new MouseEvent('click'));return row.id;})()`,
      );
      await waitFor(
        () =>
          second.evaluate(
            `${traffic}.selectedDetail?.id===${JSON.stringify(htmlId)} && ${traffic}.previewPageAvailable`,
          ),
        "Selected HTML inspection did not finish",
      );
      await second.evaluate(`${traffic}.showCapturedPage()`);
      assert.equal(await second.evaluate(`${traffic}.previewPageScripts.checked`), false);
      if (screenshot) {
        const image = await second.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-page-warning.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      const old = (await targets()).map((target) => target.id);
      await second.evaluate(`${traffic}.openCapturedPage()`);
      await waitFor(
        () => second.evaluate(`!${traffic}.previewPageBusy`),
        "Captured preview preparation did not finish",
      );
      const disabledTarget = await waitFor(async () => {
        const all = await targets();
        return all.find(
          (target) => !old.includes(target.id) && target.url.includes("/captured-page"),
        );
      }, "Captured preview did not navigate");
      const disabled = await connect(disabledTarget);
      await waitFor(
        () =>
          disabled.evaluate(
            `document.readyState==='complete' && document.getElementById('captured-heading')?.textContent==='Captured page fixture'`,
          ),
        "Captured HTML and resources did not finish rendering",
      );
      const disabledState = await disabled.evaluate(
        `({script:globalThis.capturedScriptRan===true,color:getComputedStyle(document.getElementById('captured-heading')).color,image:document.getElementById('captured-image').naturalWidth})`,
      );
      if (disabledState.color !== "rgb(0, 128, 0)")
        process.stderr.write(
          JSON.stringify(
            await second.evaluate(
              `window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${traffic}.previewReportLabel})`,
            ),
          ) + "\n",
        );
      assert.equal(disabledState.script, false);
      assert.equal(disabledState.color, "rgb(0, 128, 0)");
      assert.equal(disabledState.image, 20);
      assert.equal(await disabled.evaluate("navigator.userAgent"), "Placeholder/1");
      await disabled.call("Emulation.setUserAgentOverride", { userAgent: "Placeholder/2" });
      await disabled.call("Page.reload", { ignoreCache: true });
      await waitFor(
        () =>
          disabled.evaluate(
            "document.readyState==='complete' && document.getElementById('captured-heading') && getComputedStyle(document.getElementById('captured-heading')).color==='rgb(0, 128, 0)'",
          ),
        "Captured preview reload did not restore HTML and styles",
      );
      assert.equal(await disabled.evaluate("navigator.userAgent"), "Placeholder/2");
      const oldEnabled = (await targets()).map((target) => target.id);
      const enabledLabel = await second.evaluate(
        `window.__TAURI_INTERNALS__.invoke('open_captured_page',{id:${JSON.stringify(htmlId)},enableScripts:true,options:{scope:'all-loaded'},operationId:'native-enabled-probe'})`,
      );
      const enabledTarget = await waitFor(async () => {
        const all = await targets();
        return all.find(
          (target) => !oldEnabled.includes(target.id) && target.url.includes("/captured-page"),
        );
      }, "Script-enabled preview did not navigate");
      const enabled = await connect(enabledTarget);
      await waitFor(
        () => enabled.evaluate(`globalThis.missingResult`),
        "Captured script did not complete its missing request",
      );
      assert.deepEqual(await enabled.evaluate(`globalThis.missingResult`), { failed: true });
      await waitFor(
        () => enabled.evaluate("globalThis.experimentApi"),
        "Captured external script/API dependencies did not finish",
      );
      assert.equal(await enabled.evaluate("globalThis.experimentScriptValue"), "original");
      assert.deepEqual(await enabled.evaluate("globalThis.experimentApi"), {
        token: "captured-token",
        value: 42,
      });
      await waitFor(
        () => enabled.evaluate(`globalThis.variantResults`),
        "Captured POST variants did not finish",
      );
      assert.deepEqual(await enabled.evaluate(`globalThis.variantResults`), [
        { status: 200, body: "dark variant" },
        { status: 200, body: "light variant" },
      ]);
      const sequence = ["first response", "next response", "next response"];
      await waitFor(
        () => enabled.evaluate(`globalThis.sequenceResults`),
        "Repeated captured requests did not finish",
      );
      assert.deepEqual(await enabled.evaluate(`globalThis.sequenceResults`), sequence);
      await enabled.evaluate(`globalThis.beforeSequenceReload=true`);
      await enabled.call("Page.reload", { ignoreCache: true });
      await waitFor(
        () => enabled.evaluate(`!globalThis.beforeSequenceReload && globalThis.sequenceResults`),
        "Captured sequence did not restart on reload",
      );
      assert.deepEqual(await enabled.evaluate(`globalThis.sequenceResults`), sequence);
      const report = await second.evaluate(
        `window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(enabledLabel)}})`,
      );
      assert.ok(report.hits >= 5 && report.misses >= 1);
      assert.equal(report.scriptsEnabled, true);
      assert.equal(report.scope, "all-loaded");
      assert.ok(report.requests.some((row) => row.method === "POST" && row.outcome === "captured"));
      assert.equal(
        await main.evaluate(
          `window.__TAURI_INTERNALS__.invoke('captured_page_report',{label:${JSON.stringify(enabledLabel)}}).then(()=>false,()=>true)`,
        ),
        true,
        "Another window read a preview report",
      );
      await smokePreviewComparison(second, main, enabledLabel);
      await second.evaluate(
        `(async()=>{const workspace=${traffic};workspace.previewReportLabel=${JSON.stringify(enabledLabel)};await workspace.showPreviewReport();})()`,
      );
      if (screenshot) {
        const image = await second.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-preview-report.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await second.call("Emulation.setDeviceMetricsOverride", {
        width: 760,
        height: 520,
        deviceScaleFactor: 1,
        mobile: false,
      });
      assert.ok(
        await second.evaluate(
          `${traffic}.previewReportDialog.querySelector('footer').getBoundingClientRect().bottom<=innerHeight`,
        ),
      );
      if (screenshot) {
        const image = await second.call("Page.captureScreenshot", { format: "png" });
        await writeFile(
          screenshot.replace(".png", "-preview-report-compact.png"),
          Buffer.from(image.data, "base64"),
        );
      }
      await second.call("Emulation.clearDeviceMetricsOverride");
      await second.evaluate(`${traffic}.closePreviewReport()`);
      assert.equal(
        await enabled.evaluate(
          `(()=>{try{new RTCPeerConnection();return false;}catch{return true;}})()`,
        ),
        true,
      );
      assert.equal(
        await enabled.evaluate(
          `(()=>{try{new WebSocket('ws://127.0.0.1:9');return false;}catch{return true;}})()`,
        ),
        true,
      );
      const denied = await enabled.evaluate(
        `Promise.race([Promise.resolve().then(()=>window.__TAURI_INTERNALS__.invoke('app_status')).then(()=>false,()=>true),new Promise(resolve=>setTimeout(()=>resolve(true),500))])`,
      );
      assert.equal(denied, true, "Captured content accessed application IPC");
      let contacted = 0;
      const sink = createServer((_, response) => {
        contacted++;
        response.end("unexpected");
      });
      await new Promise((resolve) => sink.listen(0, "127.0.0.1", resolve));
      try {
        const url = "http://127.0.0.1:" + sink.address().port + "/uncaptured";
        await enabled.evaluate(
          `fetch(${JSON.stringify(url)},{mode:'no-cors'}).then(()=>true,()=>false)`,
        );
        await new Promise((resolve) => setTimeout(resolve, 100));
        assert.equal(contacted, 0, "Preview contacted an uncaptured endpoint");
      } finally {
        await new Promise((resolve) => sink.close(resolve));
      }
      assert.equal(
        await main.evaluate(
          `window.__TAURI_INTERNALS__.invoke('app_status').then(status=>status.lifecycle)`,
        ),
        "stopped",
        "Preview enabled the proxy",
      );
      if (screenshot) {
        const image = await enabled.call("Page.captureScreenshot", { format: "png" });
        await writeFile(screenshot.replace(".png", "-page.png"), Buffer.from(image.data, "base64"));
      }
      await closeNative("Baseline preview · Scripts on — Transmog");
      enabled.close();
      await waitFor(
        async () =>
          (await targets()).filter((target) =>
            [disabledTarget.id, enabledTarget.id].includes(target.id),
          ).length === 1,
        "First native preview did not close",
      );
      await closeNative("Baseline preview · Scripts off — Transmog");
      disabled.close();
      await waitFor(
        async () =>
          !(await targets()).some((target) =>
            [disabledTarget.id, enabledTarget.id].includes(target.id),
          ),
        "Closed captured previews retained browser targets",
      );
      await smokePreviewRules({
        main,
        viewer: second,
        traffic,
        pageSource,
        targets,
        connect,
        waitFor,
        closeNative,
        screenshot,
      });
      if (profileRoot)
        await waitFor(async () => {
          for await (const _ of glob("temp/transmog-captured-webview-*", { cwd: profileRoot }))
            return false;
          return true;
        }, "Closed captured preview profiles were not cleaned up");
      process.stdout.write(
        "Captured HTML, CSS across UAs, image, script choice, response sequence/reset, network failures, IPC and uncaptured egress verified.\n",
      );
    }
    const beforeReopen = (await targets()).map((target) => target.id);
    await closeNative("Transmog");
    await waitFor(
      async () => !(await targets()).some((target) => target.id === mainTarget.id),
      "Main window did not close with saved viewers remaining",
    );
    await viewer.evaluate(`window.__TAURI_INTERNALS__.invoke('open_main_window')`);
    const reopenedTarget = await waitFor(async () => {
      const all = await targets();
      return all.find(
        (target) => target.url.includes("transmog-ui") && !beforeReopen.includes(target.id),
      );
    }, "Main window did not reopen from the viewer");
    const reopened = await connect(reopenedTarget);
    await waitFor(
      () =>
        reopened.evaluate(
          `(()=>{const workspace=${root}?.shadowRoot?.querySelector('traffic-workspace');return workspace?.queryLoaded&&workspace.sessions.length===0;})()`,
        ),
      "Reopened main window retained ephemeral traffic",
    );
    assert.equal(
      await reopened.evaluate(
        `window.__TAURI_INTERNALS__.invoke('app_status').then(status=>status.lifecycle)`,
      ),
      "stopped",
    );
    if (pageSource)
      await smokeResponseSetPages({
        main: reopened,
        waitFor,
        directory: join(dirname(screenshot ?? pageSource), "response-set-native"),
        restart: true,
      });
    process.stdout.write(
      JSON.stringify({
        viewerLaunch: true,
        mainChoice: true,
        isolatedCatalogs: true,
        proxyPermissionDenied: true,
        additionalViewer: true,
        traceMetadata: true,
        nativeCapture: !!nativeSource,
        traceSave,
      }) + "\n",
    );
  }
} finally {
  for (const socket of sockets) socket.close();
}
