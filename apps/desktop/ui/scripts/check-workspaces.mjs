import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, Protocol } from '@microsoft/webui';

// Use the repository's existing, locked browser test installation.
const { chromium } = createRequire(new URL('../../../../e2e/playwright/package.json', import.meta.url))('playwright');
const root = fileURLToPath(new URL('../', import.meta.url));
const initial = JSON.parse(await readFile(resolve(root, 'src/initial-state.json'), 'utf8'));
const metadata = JSON.parse(await readFile(resolve(root, 'dist/client-metafile.json'), 'utf8'));
const assets = JSON.parse(await readFile(resolve(root, 'dist/client-assets.json'), 'utf8'));
const built = build({ appDir: resolve(root, 'src'), plugin: 'webui', css: 'link', cssPublicBase: '/', cssFileNameTemplate: '[name]-[hash].[ext]', projectionManifests: [resolve(root, 'dist/webui-projection.json')] });
assert.deepEqual(built.warnings, []);
const protocol = new Protocol(built.protocol, { plugin: 'webui' });
const html = protocol.render({ ...initial, language: 'en', pageTitle: 'Transmog', heading: 'Inspect traffic without losing the thread', lifecycleLabel: 'Stopped', lifecycleKind: 'stopped', listener: 'Not listening' }).toString().replace(/<script(?=[\s>])/g, '<script nonce="workspace-check"');
const resources = new Map(assets.map((asset) => [asset.path, { file: resolve(root, 'dist', asset.file), contentType: asset.contentType }]));
resources.set('/document.css', { file: resolve(root, 'src/document.css'), contentType: 'text/css' });
resources.set('/transmog-icon.svg', { file: resolve(root, '../icons/icon.svg'), contentType: 'image/svg+xml' });
for (let index = 0; index < built.cssFiles.length; index += 2) resources.set('/' + built.cssFiles[index], { body: built.cssFiles[index + 1], contentType: 'text/css' });
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const errors = [];
const requests = [];
page.on('pageerror', (error) => errors.push(error.message));
page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
page.on('request', (request) => requests.push(new URL(request.url()).pathname));
await page.route('http://workspace.test/**', async (route) => {
  const path = new URL(route.request().url()).pathname;
  if (path === '/') {
    await route.fulfill({ contentType: 'text/html', body: html, headers: { 'Content-Security-Policy': "default-src 'none'; base-uri 'none'; object-src 'none'; script-src 'self' 'nonce-workspace-check'; worker-src 'self'; style-src 'self' 'unsafe-inline'; font-src data:; img-src 'self' data:; connect-src 'self'; require-trusted-types-for 'script'; trusted-types webui monaco" } });
  } else {
    if (path === '/preview/pixel.png') { await route.fulfill({contentType:'image/png',body:Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==','base64')}); return; }
    const asset = resources.get(path);
    if (!asset) { await route.fulfill({ status: 404, body: 'Not found' }); return; }
    await route.fulfill({ contentType: asset.contentType, body: asset.body ?? await readFile(asset.file) });
  }
});
await page.addInitScript((workspace) => {
  const caller = { kind: 'local-process', processName: 'Fixture', processId: 42 };
  const summary = (id, index = 0) => ({ id, caller, method: 'GET', host: 'example.test', path: '/' + id, url:'http://example.test/'+id, startedAt:1000+index, contentType:id==='second'?'application/json':id==='image'?'image/png':'text/plain', protocol: 'HTTP/1.1', status: id==='cached'?304:200, durationMs: index+1, requestBytes: 0, responseBytes: id==='cached'?0:4, terminal: 'completed', loss: false, capturing: false, autoResponse: null });
  const detail = (id) => {
    const row = state.sessions.find(row => row.id===id) ?? summary(id);
    return { id, caller, requests: [{ boundary: 'client-request', method: 'GET', target: row.url, status: null, protocol: 'HTTP/1.1', headers: [{name:'Accept',value:'*/*',sensitive:false,binary:false},{name:'Authorization',value:'[redacted]',sensitive:true,binary:false}] }], responses: [{ boundary: 'client-response', method: null, target: null, status: row.status, protocol: 'HTTP/1.1', headers: [{name:'Content-Type',value:row.contentType,sensitive:false,binary:false}] }], bodies: [], storedBodies: [{ exchangeId: id, boundary: 'client-response', observedBytes: row.responseBytes, retainedBytes: row.responseBytes, availability: 'complete', mediaType: row.contentType, charset: 'utf-8', contentCodings: [], sha256: null, reason: null }], diagnostics: [], hookEffects: [], routeSelection: null, routeAttempts: [], terminal: row.terminal, websocket: null, sequenceLoss: 0, autoResponse: null };
  };
  const state = globalThis.__workspaceFixture = { calls: {}, workspace:JSON.parse(localStorage.getItem('workspace')??JSON.stringify(workspace)), lifecycle:'stopped', sessions: ['first','second','cached','image'].map(summary), paused: [], queryDelay: 0, slowDetail: false, slowBody:false };
  const callbacks = new Map();
  let callbackId = 0;
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { const id = ++callbackId; callbacks.set(id, callback); return id; },
    unregisterCallback(id) { callbacks.delete(id); },
    convertFileSrc(path) { return 'http://workspace.test/' + path; },
    async invoke(command, args) {
      state.calls[command] = (state.calls[command] ?? 0) + 1;
      switch (command) {
        case 'record_frontend_diagnostic': throw new Error('Unexpected frontend diagnostic: ' + args.message);
        case 'desktop_bootstrap': return { caCertificatePath: 'fixture.pem', caPrivateKeyPath: 'fixture.key', caFilesPresent: true, ownedCaSha256: '0'.repeat(64), ownedCaTrusted: true, hostRestorePending: false, diagnosticsPath: 'fixture.jsonl' };
        case 'product_state': return { schemaVersion: 4, workspace:structuredClone(state.workspace), preferences: { theme: 'system', sessionPageSize: 100, configureSystemProxy: true }, privacy: { retainResponseBodies: true, retainBodySamples: false, rememberRecentArtifacts: false, includePathsInSupportBundles: false }, window: {}, recentArtifacts: [] };
        case 'save_workspace_preferences': state.workspace = structuredClone(args.preferences); localStorage.setItem('workspace',JSON.stringify(state.workspace)); return state.workspace;
        case 'app_status': return { lifecycle: state.lifecycle, listener: state.lifecycle==='running'?'127.0.0.1:8888':null, summary: 'Proxy '+state.lifecycle, hostRestorePending: false };
        case 'start_proxy': await new Promise(resolve => setTimeout(resolve,80)); state.lifecycle='running'; return {lifecycle:'running',listener:'127.0.0.1:8888',summary:'Proxy running',hostRestorePending:false};
        case 'stop_application': await new Promise(resolve => setTimeout(resolve,80)); state.lifecycle='stopped'; return {lifecycle:'stopped',listener:null,summary:'Proxy stopped',hostRestorePending:false};
        case 'watch_sessions': state.channel = args.onEvent; return;
        case 'query_sessions': {
          if (state.queryError) throw new Error('Fixture query failure');
          await new Promise((resolve) => setTimeout(resolve, state.queryDelay)); state.lastQuery=args.query;
          const key = (row, column) => ({method:row.method,status:row.status,process:row.caller.processName,host:row.host,path:row.path,duration:row.durationMs,'response-bytes':row.responseBytes,'started-at':row.startedAt,pid:row.caller.processId,url:row.url})[column];
          let rows = structuredClone(state.sessions).filter(row => !args.query.search || (row.url+' '+row.caller.processName+' '+row.caller.processId).toLowerCase().includes(args.query.search.toLowerCase()));
          for (const filter of args.query.filters ?? []) rows = rows.filter(row => filter.operator==='minimum'?key(row,filter.column)>=Number(filter.value):filter.operator==='maximum'?key(row,filter.column)<=Number(filter.value):filter.operator==='equals'?String(key(row,filter.column)).toLowerCase()===filter.value.toLowerCase():String(key(row,filter.column)).toLowerCase().includes(filter.value.toLowerCase()));
          const {sort,offset=0,limit=100} = args.query;
          if (sort) rows.sort((a,b) => {const left=key(a,sort.column),right=key(b,sort.column);const result=typeof left==='number'?left-right:String(left).localeCompare(String(right));return (sort.direction==='descending'?-result:result)||a.id.localeCompare(b.id);});
          return {sessions:rows.slice(offset,offset+limit),totalMatched:rows.length,retainedCount:state.sessions.length,nextCursor:null,evicted:0,sequenceGaps:0,subscriberLag:0};
        }
        case 'session_detail': if (state.slowDetail && args.id === 'first') await new Promise((resolve) => setTimeout(resolve, 100)); return detail(args.id);
        case 'inspect_body': {
          if (state.slowBody && args.request.sessionId==='first') await new Promise(resolve => setTimeout(resolve,100));
          const metadata=detail(args.request.sessionId).storedBodies[0];
          if (state.bodyOverride) Object.assign(metadata,state.bodyOverride);
          state.lastBodyRequest=structuredClone(args.request);
          if (metadata.contentCodings.length && metadata.availability!=='complete' && args.request.decodeContent) throw new Error('Content decoding needs a complete body');
          const representation=args.request.representation==='auto'?(metadata.mediaType==='application/json'?'formatted-json':metadata.mediaType==='image/png'?'image':'original-text'):args.request.representation;
          return { metadata, representation, decoded:true, textEncoding:'utf-8', display:representation==='formatted-json'?'{\n  "fixture": true\n}':'body for '+args.request.sessionId, displayBytes:4,truncated:false,nextOffset:null,warning:null,previewHandle:representation==='image'?'pixel.png':null,previewMimeType:representation==='image'?'image/png':null };
        }
        case 'automation_status': return { generation: 0, rules: [], candidateCount: 0, historyCount: 0 };
        case 'script_declarations': return '';
        case 'script_status': return { generation: 0, active: [], saved: [], candidateCount: 0, historyCount: 0 };
        case 'breakpoint_status': return { enabled: true, paused: structuredClone(state.paused) };
        case 'decide_breakpoint': state.paused = state.paused.filter((item) => item.decisionId !== args.decision.decisionId); return { enabled: true, paused: structuredClone(state.paused) };
        default: throw new Error('Unexpected fixture command: ' + command);
      }
    },
  };
  globalThis.__cspViolations = [];
  addEventListener('securitypolicyviolation', (event) => globalThis.__cspViolations.push(event.effectiveDirective));
}, initial.workspace);

const view = async (name) => {
  await page.locator('app-shell a[data-view="' + name + '"]').click();
  await page.locator('#' + name).waitFor({ state: 'visible' });
};
try {
  await page.goto('http://workspace.test/');
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.session-status').textContent.includes('Loaded 4 of 4 exchanges.'));
  assert.match(await page.locator('.session-status').textContent(),/Proxy stopped/,'Traffic banner claims live capture while the proxy is stopped');
  assert.match(await page.locator('.list-footer').textContent(),/Showing captured traffic/);
  assert.doesNotMatch(await page.locator('.list-footer').textContent(),/Following live|capture continues/);
  assert.equal(await page.locator('#traffic').isVisible(), true);
  assert.equal(await page.locator('a[data-view="traffic"]').getAttribute('aria-current'), 'page');
  const editorOutputs = Object.entries(metadata.outputs).filter(([, output]) => Object.keys(output.inputs).some((input) => input.includes('node_modules/monaco-editor'))).map(([file]) => '/' + file.replace(/^dist\//, ''));
  assert.equal(requests.some((path) => path === '/monaco.js' || path === '/monaco.css' || editorOutputs.includes(path)), false, 'Monaco loaded at startup');
  assert.equal(await page.evaluate(() => customElements.get('automation-workspace') !== undefined), false, 'Automation hydrated at startup');
  const startupRequests = requests.length;
  assert.equal(await page.evaluate(() => CSS.supports('width','attr(data-width type(<length>))')), true, 'Typed CSS attributes required by resizable panels are unavailable');
  assert.deepEqual(await page.locator('.traffic-table th').evaluateAll(headers => headers.map(header=>header.dataset.columnId)), ['method','status','process','host','path','duration','response-bytes']);
  assert.match(await page.locator('tr[data-session-id="first"] td[data-column-id="process"]').textContent(), /Fixture \(42\)/);
  assert.equal(await page.locator('tr[data-session-id="cached"]').getAttribute('data-tone'),'not-modified');
  const toggle = page.getByRole('button',{name:'Toggle navigation labels'});
  await toggle.click();
  assert.equal(await toggle.getAttribute('aria-expanded'),'false');
  const methodEdge = page.getByRole('separator',{name:'Resize Method column',exact:true});
  const originalWidth = await page.locator('#header-method').evaluate(header=>header.getBoundingClientRect().width);
  await methodEdge.focus(); await page.keyboard.press('ArrowRight');
  const resizedWidth = await page.locator('#header-method').evaluate(header=>header.getBoundingClientRect().width);
  assert.ok(Math.abs(resizedWidth-originalWidth-10)<1,'Keyboard resizing stretched the column beyond the requested width');
  const edgeBounds = await methodEdge.boundingBox();
  await page.mouse.move(edgeBounds.x+2,edgeBounds.y+10); await page.mouse.down();
  await page.mouse.move(edgeBounds.x+3,edgeBounds.y+10);
  assert.ok(Math.abs(await page.locator('#header-method').evaluate(header=>header.getBoundingClientRect().width)-resizedWidth-1)<1,'Column snapped at the start of a resize');
  await page.mouse.move(edgeBounds.x+42,edgeBounds.y+10,{steps:5}); await page.mouse.up();
  assert.ok(Math.abs(await page.locator('#header-method').evaluate(header=>header.getBoundingClientRect().width)-resizedWidth-40)<1,'Pointer resizing did not track cursor movement');
  const processHeader=await page.locator('#header-process .column-trigger').boundingBox();
  const pathHeader=await page.locator('#header-path .column-trigger').boundingBox();
  await page.mouse.move(processHeader.x+15,processHeader.y+12); await page.mouse.down();
  await page.mouse.move(processHeader.x+25,processHeader.y+12,{steps:3});
  assert.equal(await page.locator('#header-process').getAttribute('data-dragging'),'','Header button did not enter drag mode');
  await page.mouse.move(pathHeader.x+pathHeader.width/2,pathHeader.y+12,{steps:8});
  assert.equal(await page.locator('#header-path').getAttribute('data-drop-target'),'','Column drop target is not visible');
  await page.mouse.up();
  assert.deepEqual(await page.locator('.traffic-table th').evaluateAll(headers=>headers.map(header=>header.dataset.columnId)),['method','status','host','path','process','duration','response-bytes']);
  await page.waitForTimeout(300); // Let the accidental post-drag click guard expire.
  const dragCancelBounds=await page.locator('#header-process .column-trigger').boundingBox();
  await page.mouse.move(dragCancelBounds.x+15,dragCancelBounds.y+12); await page.mouse.down();
  await page.mouse.move(dragCancelBounds.x-50,dragCancelBounds.y+12,{steps:5}); await page.keyboard.press('Escape'); await page.mouse.up();
  assert.deepEqual(await page.locator('.traffic-table th').evaluateAll(headers=>headers.map(header=>header.dataset.columnId)),['method','status','host','path','process','duration','response-bytes'],'Cancelling a column drag changed the layout');
  await page.waitForTimeout(300);
  await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').setColumnWidth('method',32,true));
  assert.ok(Math.abs(await page.locator('#header-method').evaluate(header=>header.getBoundingClientRect().width)-32)<1,'Minimum column width is stretched by table or header content');
  await methodEdge.press('Home');
  const openColumn = async id => { await page.locator('#header-'+id+' .column-trigger').click(); await page.locator('#column-actions').waitFor({state:'visible'}); };
  await openColumn('host'); await page.locator('#column-actions').getByRole('button',{name:'Move left',exact:true}).click();
  assert.deepEqual(await page.locator('.traffic-table th').evaluateAll(headers=>headers.slice(0,3).map(header=>header.dataset.columnId)),['method','host','status'],'Moving across pinned columns did not change visible order');
  await openColumn('host'); await page.locator('#column-actions').getByRole('button',{name:'Move right',exact:true}).click();
  await openColumn('host'); await page.locator('#column-actions').getByRole('button',{name:'Unpin column',exact:true}).click();
  await openColumn('status'); await page.locator('#column-actions').getByRole('button',{name:'Highest first',exact:false}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('.traffic-table tbody tr').dataset.sessionId==='cached');
  assert.equal(await page.locator('#header-status').getAttribute('aria-sort'),'descending');
  await openColumn('host'); await page.locator('#column-actions').getByRole('button',{name:'Pin column',exact:true}).click();
  assert.equal(await page.locator('#header-host').getAttribute('data-pinned'),'');
  await openColumn('response-bytes'); await page.locator('#column-actions').getByRole('button',{name:'Hide column',exact:true}).click();
  assert.equal(await page.locator('#header-response-bytes').count(),0);
  await page.getByRole('button',{name:'Table settings',exact:true}).click();
  await page.locator('#table-settings').getByLabel('Response size',{exact:true}).check();
  await page.keyboard.press('Escape');
  assert.equal(await page.locator('#header-response-bytes').count(),1);
  await openColumn('status'); await page.getByRole('textbox',{name:'Column filter value'}).fill('304'); await page.locator('#column-actions').getByRole('button',{name:'Apply filter'}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelectorAll('.traffic-table tbody tr[data-session-id]').length===1);
  assert.equal(await page.locator('.traffic-table tbody tr[data-session-id]').getAttribute('data-session-id'),'cached');
  await page.getByRole('button',{name:'Clear filters',exact:true}).click();
  await page.getByRole('button',{name:'Follow live',exact:true}).click();
  const listDivider = page.getByRole('separator',{name:'Resize traffic list and inspector',exact:true});
  const listBefore = await page.locator('.session-list-pane').evaluate(pane=>pane.getBoundingClientRect().height);
  await listDivider.focus(); await page.keyboard.press('ArrowDown');
  assert.ok(await page.locator('.session-list-pane').evaluate(pane=>pane.getBoundingClientRect().height)>listBefore+5,'Panel divider did not resize list');
  await page.getByLabel('Traffic layout',{exact:true}).selectOption('side-by-side');
  const panes = await page.locator('.traffic-workspace').evaluate(grid=>({list:grid.querySelector('.session-list-pane').getBoundingClientRect().toJSON(),details:grid.querySelector('.details-pane').getBoundingClientRect().toJSON()}));
  assert.ok(panes.details.x>panes.list.x+panes.list.width,'Side-by-side layout did not arrange panes horizontally');
  await page.getByLabel('Traffic layout',{exact:true}).selectOption('stacked');
  const proxy = page.locator('.top-actions proxy-toggle button');
  await proxy.click();
  await page.getByRole('button',{name:'Starting…',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Starting proxy\./);
  assert.equal(await proxy.isEnabled(),false);
  await page.getByRole('button',{name:'Stop proxy',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Proxy running\./);
  const runningColor=await proxy.evaluate(button=>getComputedStyle(button).backgroundColor);
  await proxy.click();
  await page.getByRole('button',{name:'Stopping…',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Stopping proxy\./);
  await page.getByRole('button',{name:'Start proxy',exact:true}).first().waitFor({state:'visible'});
  assert.notEqual(await proxy.evaluate(button=>getComputedStyle(button).backgroundColor),runningColor);
  await page.evaluate(async()=>{globalThis.__workspaceFixture.lifecycle='running';await document.querySelector('app-shell').refreshStatus();});
  await page.getByRole('button',{name:'Stop proxy',exact:true}).first().waitFor({state:'visible'});
  await proxy.click();
  await page.getByRole('button',{name:'Start proxy',exact:true}).first().waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.stop_application),2,'Proxy action did not track refreshed lifecycle state');

  const retainedRows = await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture;
    const rows=state.sessions; state.sessions=[];
    await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);
    return rows;
  });
  assert.equal(await page.locator('.session-status').textContent(),'Proxy stopped. Start the proxy to capture traffic.');
  await page.evaluate(async()=>{globalThis.__workspaceFixture.lifecycle='running';await document.querySelector('app-shell').refreshStatus();});
  assert.equal(await page.locator('.session-status').textContent(),'Proxy running. Waiting for proxied requests.');
  for (const lifecycle of ['stopping','failed']) {
    await page.evaluate(async(lifecycle)=>{globalThis.__workspaceFixture.lifecycle=lifecycle;await document.querySelector('app-shell').refreshStatus();},lifecycle);
    assert.doesNotMatch(await page.locator('.session-status').textContent(),/Waiting for proxied requests|Watching live/);
    assert.doesNotMatch(await page.locator('.list-footer').textContent(),/Following live|capture continues/);
  }
  await page.evaluate(async()=>{
    const shell=document.querySelector('app-shell'),state=globalThis.__workspaceFixture;
    state.lifecycle='running'; await shell.refreshStatus();
    state.queryDelay=80;
    const refresh=shell.shadowRoot.querySelector('traffic-workspace').refreshSessions();
    state.lifecycle='stopped'; await shell.refreshStatus(); await refresh;
    state.queryDelay=0;
  });
  assert.equal(await page.locator('.session-status').textContent(),'Proxy stopped. Start the proxy to capture traffic.','A delayed query restored a stale running message');
  await page.evaluate(async(rows)=>{globalThis.__workspaceFixture.sessions=rows;await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);},retainedRows);
  await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture,shell=document.querySelector('app-shell');
    state.queryError=true; await shell.shadowRoot.querySelector('traffic-workspace').refreshSessions();
    state.lifecycle='running'; await shell.refreshStatus();
  });
  assert.match(await page.locator('.session-status').textContent(),/^Proxy running\. Traffic query failed: Fixture query failure/,'A lifecycle refresh hid a traffic query failure');
  await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture,shell=document.querySelector('app-shell');
    state.queryError=false; state.lifecycle='stopped'; await shell.refreshStatus();
    await shell.shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);
  });

  await page.evaluate(() => { globalThis.__row = document.querySelector('app-shell').shadowRoot.querySelector('tr[data-session-id="first"]'); });
  await page.locator('form.filters button').click();
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.session-status').textContent.includes('Loaded '));
  assert.equal(await page.evaluate(() => globalThis.__row === document.querySelector('app-shell').shadowRoot.querySelector('tr[data-session-id="first"]')), true, 'A refresh replaced a keyed row');
  const coalesced = await page.evaluate(async () => {
    const traffic = document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');
    const before = globalThis.__workspaceFixture.calls.query_sessions;
    globalThis.__workspaceFixture.queryDelay = 25;
    await Promise.all(Array.from({ length: 30 }, () => traffic.refreshSessions()));
    globalThis.__workspaceFixture.queryDelay = 0;
    return globalThis.__workspaceFixture.calls.query_sessions - before;
  });
  assert.ok(coalesced <= 2, 'Burst refreshes were not coalesced');
  await page.evaluate(async () => {
    const traffic = document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');
    globalThis.__workspaceFixture.slowDetail = true;
    await Promise.all([traffic.inspectSession(globalThis.__workspaceFixture.sessions[0]), traffic.inspectSession(globalThis.__workspaceFixture.sessions[1])]);
  });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.selection-bar strong').textContent.endsWith('/second'));
  assert.equal(await page.locator('tr[data-session-id="second"]').getAttribute('aria-selected'), 'true');
  assert.match(await page.locator('.list-footer').textContent(),/Inspection pinned · showing captured traffic/);
  await page.locator('message-inspector[side="response"]').getByText('Auto · JSON',{exact:true}).waitFor({state:'visible'});
  assert.match(await page.locator('message-inspector[side="response"] .body-preview').textContent(), /"fixture": true/);
  await page.getByRole('button',{name:'Replay',exact:true}).click();
  await page.locator('#composer').waitFor({state:'visible'});
  assert.equal(await page.locator('#composer input[name="url"]').inputValue(),'http://example.test/second');
  assert.equal(await page.locator('#composer textarea[name="headers"]').inputValue(),'Accept: */*');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.execute_composer??0),0,'Replay ran before explicit execution');
  await view('traffic');
  await page.locator('tr[data-session-id="cached"]').click();
  await page.locator('message-inspector[side="response"] .body-preview').getByText('304 Not Modified',{exact:false}).waitFor({state:'visible'});
  await page.locator('tr[data-session-id="image"]').click();
  await page.locator('message-inspector[side="response"]').getByText('Auto · Image',{exact:true}).waitFor({state:'visible'});
  await page.locator('message-inspector[side="response"] img').waitFor({state:'visible'});
  const responseInspector=page.locator('message-inspector[side="response"]');
  await page.evaluate(()=>{
    const inspector=document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="response"]');
    const detail=structuredClone(inspector.detail);
    const override=globalThis.__workspaceFixture.bodyOverride={availability:'lost',observedBytes:8,retainedBytes:4,contentCodings:['gzip'],reason:'exchange failed at ResponseBody before completion'};
    Object.assign(detail.storedBodies[0],override);
    inspector.detail=detail;
  });
  await responseInspector.getByText('Preview unavailable:',{exact:false}).waitFor({state:'visible'});
  assert.match(await responseInspector.locator('.body-facts').textContent(),/4 B retained \/ 8 B observed · lost/);
  assert.match(await responseInspector.locator('.body-preview').textContent(),/Reason: exchange failed at ResponseBody/);
  assert.match(await responseInspector.locator('.body-preview').textContent(),/Raw Hex shows the 4 B retained bytes/);
  await responseInspector.getByLabel('Body viewer',{exact:true}).selectOption('bytes');
  await responseInspector.getByText('Hex',{exact:true}).last().waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastBodyRequest.decodeContent),false,'Hex tried to decode an incomplete compressed body');
  assert.equal(await responseInspector.getByLabel('Decode',{exact:true}).isChecked(),false);
  assert.equal(await responseInspector.getByLabel('Decode',{exact:true}).isEnabled(),false);
  await responseInspector.getByLabel('Body viewer',{exact:true}).selectOption('metadata');
  assert.match(await responseInspector.locator('.body-preview').textContent(),/"reason": "exchange failed at ResponseBody/);
  await page.evaluate(()=>{
    const inspector=document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="response"]');
    const detail=structuredClone(inspector.detail);
    Object.assign(detail.storedBodies[0],{availability:'truncated',observedBytes:1851,retainedBytes:1800,reason:'per-body retention limit reached'});
    inspector.detail=detail;
  });
  assert.match(await responseInspector.locator('.body-facts').textContent(),/1,800 B retained \/ 1,851 B observed/,'Rounded counts hid missing body bytes');
  await page.evaluate(()=>{
    const inspector=document.querySelector('app-shell').shadowRoot.querySelector('message-inspector[side="response"]');
    const detail=structuredClone(inspector.detail);
    Object.assign(detail.storedBodies[0],{availability:'evicted',retainedBytes:0,reason:'evicted by circular response-body quota'});
    inspector.detail=detail;
  });
  assert.match(await responseInspector.locator('.body-preview').textContent(),/"availability": "evicted"/,'Metadata cannot inspect a body without retained bytes');
  await responseInspector.getByLabel('Body viewer',{exact:true}).selectOption('auto');
  assert.match(await responseInspector.locator('.body-preview').textContent(),/removed to make room/);
  assert.doesNotMatch(await responseInspector.locator('.body-preview').textContent(),/Hex/,'Unavailable bytes incorrectly recommend Hex');
  await page.evaluate(()=>delete globalThis.__workspaceFixture.bodyOverride);
  await page.locator('tr[data-session-id="second"]').click();
  await page.locator('message-inspector[side="response"]').getByText('Auto · JSON',{exact:true}).waitFor({state:'visible'});
  const heldOrder=await page.locator('.traffic-table tbody tr[data-session-id]').evaluateAll(rows=>rows.map(row=>row.dataset.sessionId));
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;state.sessions.push({...state.sessions[0],id:'new',url:'http://example.test/new',path:'/new',startedAt:9999});await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions();});
  assert.deepEqual(await page.locator('.traffic-table tbody tr[data-session-id]').evaluateAll(rows=>rows.map(row=>row.dataset.sessionId)),heldOrder,'Live arrival moved rows during inspection');
  await page.locator('.new-traffic-button').waitFor({state:'visible'});
  assert.match(await page.locator('.new-traffic-button').textContent(), /1\s+new/);
  await page.locator('.new-traffic-button').click();
  assert.equal(await page.locator('.traffic-table tbody tr').first().getAttribute('data-session-id'),'new');
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;Object.assign(state.sessions[0],{status:null,terminal:'active',responseBytes:0});await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);});
  await page.locator('tr[data-session-id="first"]').click();
  await page.locator('.selection-bar').getByText('Pending',{exact:true}).waitFor({state:'visible'});
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;Object.assign(state.sessions[0],{status:200,terminal:'completed',responseBytes:4});state.channel.onmessage({exchangeId:'first',sequence:2,lagged:false});});
  await page.locator('message-inspector[side="response"] .body-preview').getByText('body for first',{exact:true}).waitFor({state:'visible'});
  await page.locator('tr[data-session-id="second"]').click();
  await page.locator('message-inspector[side="response"]').getByText('Auto · JSON',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button', { name: 'Create auto-response', exact: true }).isEnabled(), true);
  await page.getByRole('button', { name: 'Create auto-response', exact: true }).click();
  await page.locator('#automation').waitFor({ state: 'visible' });
  await page.locator('.auto-response-editor').waitFor({ state: 'visible' });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.auto-response-source p').textContent.includes('Decoded editing will preserve utf-8'));
  await page.locator('.auto-response-editor').getByRole('button', { name: 'Cancel', exact: true }).first().click();
  await page.getByRole('button', { name: 'Create from scratch', exact: true }).click();
  await page.locator('.auto-response-editor input[name="name"]').fill('Preserved draft');
  await page.locator('.auto-response-editor select[name="method"]').selectOption('POST');
  assert.equal(await page.locator('.auto-response-editor textarea[name="requestHeaders"]').isVisible(), true);
  await view('composer');
  await page.locator('input[name="url"]').filter({ visible: true }).fill('http://example.test/replay');
  await view('automation');
  assert.equal(await page.locator('.auto-response-editor input[name="name"]').inputValue(), 'Preserved draft');
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').sourceEditor !== null);
  const sourceSurface=page.locator('script-editor editor-surface').first();
  assert.ok((await page.locator('script-editor .monaco-editor-host').first().boundingBox()).height>=320,'Source editor is too short');
  await sourceSurface.getByRole('button',{name:'Expand editor',exact:true}).click();
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),true);
  assert.ok((await page.locator('script-editor .monaco-editor-host').first().boundingBox()).height>=580,'Expanded source editor is too short');
  assert.equal(await sourceSurface.getByRole('button',{name:'Save draft',exact:true}).isVisible(),true);
  await page.keyboard.press('Escape');
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),false);
  const editorsBefore = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.paused = Array.from({ length: 20 }, (_, index) => ({ decisionId: index + 1, exchangeId: 'paused-' + index, phase: 'request-head', requestHead: { method: 'GET' }, responseHead: null, bodyHex: null, hookId: 'fixture' }));
  });
  await view('breakpoints');
  await page.locator('paused-exchange').first().getByRole('button', { name: 'Continue', exact: true }).waitFor({ state: 'visible' });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange')?.editorReady);
  const editorsVisible = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  assert.ok(editorsVisible > editorsBefore && editorsVisible <= editorsBefore + 2, 'Offscreen breakpoint editors were eagerly created');
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.paused = [];
    await document.querySelector('app-shell').shadowRoot.querySelector('breakpoint-workspace').refreshBreakpoints();
  });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length === 1);
  await view('traffic');
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector('app-shell')).colorScheme), 'dark');
  const layout = await page.evaluate(() => ({ height: document.documentElement.clientHeight, scrollHeight: document.documentElement.scrollHeight }));
  assert.ok(layout.scrollHeight <= layout.height + 1, 'The document viewport can scroll');
  assert.deepEqual(await page.evaluate(() => globalThis.__cspViolations), []);
  assert.deepEqual(errors, []);
  await page.waitForFunction(()=>globalThis.__workspaceFixture.workspace.sidebarCollapsed && globalThis.__workspaceFixture.workspace.listSplit===47 && globalThis.__workspaceFixture.workspace.columns.find(column=>column.id==='host').pinned);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/workspace.png')});
  const beforeReload=requests.length;
  await page.reload();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('.session-status').textContent.includes('Loaded '));
  assert.equal(await page.getByRole('button',{name:'Toggle navigation labels'}).getAttribute('aria-expanded'),'false');
  assert.equal(await page.getByRole('separator',{name:'Resize traffic list and inspector',exact:true}).getAttribute('aria-valuenow'),'47');
  assert.equal(await page.locator('#header-host').getAttribute('data-pinned'),'');
  assert.equal(requests.slice(beforeReload).some(path=>path==='/monaco.js'||path==='/monaco.css'||editorOutputs.includes(path)),false,'Reload eagerly fetched Monaco');
  assert.deepEqual(errors, []);
  await page.setViewportSize({width:760,height:520});
  await page.locator('tr[data-session-id="second"]').click();
  await page.locator('message-inspector[side="response"]').getByText('Auto · JSON',{exact:true}).waitFor({state:'visible'});
  const smallPreview=await page.locator('message-inspector[side="response"] .body-scroll').boundingBox();
  assert.ok(smallPreview.height>=48,'Small-window response preview has no usable space');
  await page.locator('message-inspector[side="response"] .body-preview').scrollIntoViewIfNeeded();
  assert.equal(await page.evaluate(()=>document.documentElement.scrollHeight<=document.documentElement.clientHeight+1),true,'Small-window inspection scroll escaped the pane');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/workspace-small.png')});
  await openColumn('status');
  const smallMenu=await page.locator('#column-actions').boundingBox();
  assert.ok(smallMenu.y+smallMenu.height<=520,'Column menu extends below a small window');
  await page.keyboard.press('Escape');
  process.stdout.write(JSON.stringify({ startupRequests, coalescedQueries: coalesced, editorsBefore, editorsVisible, components: built.stats.componentCount, cspViolations: 0 }) + '\n');
} catch (error) {
  await page.screenshot({path:resolve(root,'../../../target/ui-check/workspace-failure.png')});
  process.stderr.write(JSON.stringify({ errors, requests, state: await page.evaluate(() => ({ calls: globalThis.__workspaceFixture?.calls, output: document.querySelector('app-shell')?.shadowRoot?.querySelector('.session-status')?.textContent, diagnostics: document.querySelector('app-shell')?.shadowRoot?.querySelector('.global-diagnostics')?.textContent, definitions: ['app-shell', 'traffic-workspace', 'settings-workspace'].map((tag) => [tag, Boolean(customElements.get(tag))]) })) }, null, 2) + '\n');
  throw error;
} finally { await browser.close(); }
