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
await page.route('https://workspace.test/**', async (route) => {
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
  const summary = (id, index = 0) => ({ id, caller, method: 'GET', host: 'example.test', path: '/' + id, url:'http://example.test/'+id, startedAt:1000+index, contentType:id==='second'?'application/json':id==='image'?'image/webp':'text/plain', protocol: 'HTTP/1.1', status: id==='cached'?304:200, durationMs: index+1, requestBytes: 0, responseBytes: id==='cached'?0:4, terminal: 'completed', loss: false, capturing: false, autoResponse: null });
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
    convertFileSrc(path) { return 'https://workspace.test/' + path; },
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
          if (state.deferFocus && args.query.focusId) { state.deferFocus=false; state.focusPending=true; await new Promise(resolve=>state.releaseFocus=resolve); }
          if (state.deferRefresh && !args.query.focusId) { state.deferRefresh=false; state.refreshPending=true; await new Promise(resolve=>state.releaseRefresh=resolve); }
          await new Promise((resolve) => setTimeout(resolve, state.queryDelay)); state.lastQuery=args.query;
          const key = (row, column) => ({method:row.method,status:row.status,process:row.caller.processName,host:row.host,path:row.path,duration:row.durationMs,'response-bytes':row.responseBytes,'started-at':row.startedAt,pid:row.caller.processId,url:row.url})[column];
          let rows = structuredClone(state.sessions).filter(row => !args.query.search || (row.url+' '+row.caller.processName+' '+row.caller.processId).toLowerCase().includes(args.query.search.toLowerCase()));
          for (const filter of args.query.filters ?? []) rows = rows.filter(row => filter.operator==='minimum'?key(row,filter.column)>=Number(filter.value):filter.operator==='maximum'?key(row,filter.column)<=Number(filter.value):filter.operator==='equals'?String(key(row,filter.column)).toLowerCase()===filter.value.toLowerCase():String(key(row,filter.column)).toLowerCase().includes(filter.value.toLowerCase()));
          const {sort,limit=100} = args.query;
          let offset=args.query.offset??0;
          if (sort) rows.sort((a,b) => {const left=key(a,sort.column),right=key(b,sort.column);const result=typeof left==='number'?left-right:String(left).localeCompare(String(right));return (sort.direction==='descending'?-result:result)||a.id.localeCompare(b.id);});
          if (args.query.focusId) {
            const index=rows.findIndex(row=>row.id===args.query.focusId);
            if (index<0) throw new Error('Source no longer in Traffic.');
            offset=Math.floor(index/limit)*limit;
          }
          return {sessions:rows.slice(offset,offset+limit),focusOffset:args.query.focusId?offset:null,totalMatched:rows.length,retainedCount:state.sessions.length,nextCursor:null,evicted:0,sequenceGaps:0,subscriberLag:0};
        }
        case 'session_detail':
          if (state.deferDetail===args.id) { state.detailPending=true; await new Promise(resolve=>state.releaseDetail=resolve); }
          if (state.slowDetail && args.id === 'first') await new Promise((resolve) => setTimeout(resolve, 100));
          if (state.evictedIds?.includes(args.id)) throw new Error('Session is unavailable or has been evicted');
          return detail(args.id);
        case 'save_response_body': {
          state.lastSave=structuredClone(args);
          await new Promise(resolve=>setTimeout(resolve,80));
          if (state.saveError) throw new Error('Fixture file write failed');
          return state.saveCancelled ? null : {fileName:'original.webp',bytes:4};
        }
        case 'inspect_body': {
          if (state.deferBody===args.request.sessionId && args.request.representation==='original-text') { state.bodyPending=true; await new Promise(resolve=>state.releaseBody=resolve); }
          if (state.slowBody && args.request.sessionId==='first') await new Promise(resolve => setTimeout(resolve,100));
          const metadata=detail(args.request.sessionId).storedBodies[0];
          if (state.bodyOverride) Object.assign(metadata,state.bodyOverride);
          state.lastBodyRequest=structuredClone(args.request);
          if (metadata.contentCodings.length && metadata.availability!=='complete' && args.request.decodeContent) throw new Error('Content decoding needs a complete body');
          const representation=args.request.representation==='auto'?(metadata.mediaType==='application/json'?'formatted-json':metadata.mediaType.startsWith('image/')?'image':'original-text'):args.request.representation;
          return { metadata, representation, decoded:true, textEncoding:'utf-8', display:representation==='formatted-json'?'{\n  "fixture": true\n}':'body for '+args.request.sessionId, displayBytes:4,truncated:false,nextOffset:null,warning:null,previewHandle:representation==='image'?'pixel.png':null,previewMimeType:representation==='image'?'image/png':null };
        }
        case 'automation_status': return structuredClone(state.automation??{ generation: 0, rules: [], autoresponsesEnabled:true,diagnostics:[],usage:[],candidateCount: 0, historyCount: 0 });
        case 'set_autoresponses_enabled': {
          state.automation??={generation:0,rules:[],diagnostics:[],usage:[],candidateCount:0,historyCount:0};
          if(state.automation.generation!==args.generation)throw new Error('Stale generation');
          state.automation={...state.automation,generation:state.automation.generation+1,autoresponsesEnabled:args.enabled};return structuredClone(state.automation);
        }
        case 'test_autoresponse_match': {
          state.lastMatcherTest=structuredClone(args.input);
          const {matcher,url}=args.input;const request=new URL(url);const condition=matcher.url;
          let matched=true;const checks=[];const captures=[];
          if(condition?.kind==='exact')matched=new URL(condition.value).href===request.href;
          else if(condition?.kind==='pattern') {
            let previous=0;let expression='';const labels=[];
            for(const token of condition.value.address.matchAll(/\{([^{}]*)\}/g)) {
              expression+=condition.value.address.slice(previous,token.index).replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
              expression+=token[1].endsWith(':digits')?'([0-9]+)':token[1].endsWith('...')?'([^/?#]+(?:/[^/?#]+)*)':'([^/?#]+)';
              labels.push(token[1].replace(/:digits$|\.\.\.$/g,'')||'Part '+(labels.length+1));previous=token.index+token[0].length;
            }
            expression+=condition.value.address.slice(previous).replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
            const result=new RegExp('^'+expression+'$',condition.value.caseSensitive?'':'i').exec(request.origin+request.pathname);
            matched=Boolean(result);if(result)labels.forEach((label,index)=>captures.push({label,value:result[index+1]}));
            const query=condition.value.query;
            if(query.kind==='exact')matched&&=(query.value===null?!request.search:query.value===request.search.slice(1));
            if(query.kind==='parameters')matched&&=query.value.every(item=>request.searchParams.getAll(item.name).includes(item.value));
          } else if(condition?.kind==='regex') {
            const {pattern,whole,caseSensitive,scope}=condition.value;
            matched=new RegExp(whole?'^(?:'+pattern+')$':pattern,caseSensitive?'':'i').test(scope==='path'?request.pathname:request.href);
          }
          checks.push({label:'Address',matched,detail:'Fixture runtime condition'});
          if(matcher.method){const method=matcher.method===args.input.method;checks.push({label:'Method',matched:method,detail:matcher.method});matched&&=method;}
          return {test:{matched,normalizedUrl:request.href,checks,captures},wouldServe:matched && args.input.enabled && state.automation?.autoresponsesEnabled!==false,explanation:matched?(args.input.enabled?'This rule would serve the saved response.':'The request matches, but this rule is disabled.'):'This request does not match the draft rule.',winningRule:null,examples:matcher.examples?.map(item=>({url:item.url,passed:true}))??[]};
        }
        case 'response_assets': return structuredClone(state.assets??[]);
        case 'inspect_response_asset': {
          const asset=state.assets.find(asset=>asset.id+'@'+asset.revision===args.reference);
          if(!asset)throw new Error('Saved response missing');
          return {asset:structuredClone(asset),display:asset.display??'saved body',textEncoding:'utf-8',contentCodings:[],explanation:'Edits create a new saved response revision.'};
        }
        case 'edit_response_asset': {
          const source=state.assets.find(asset=>asset.id+'@'+asset.revision===args.input.assetReference);
          const revision=Math.max(...state.assets.filter(asset=>asset.id===source.id).map(asset=>asset.revision))+1;
          const asset={...source,revision,status:args.input.status,headers:args.input.headers,mediaType:args.input.mediaType,display:args.input.decodedBody?new TextDecoder().decode(new Uint8Array(args.input.decodedBody)):source.display};
          state.assets.push(asset);state.lastAssetEdit=structuredClone(args.input);return structuredClone(asset);
        }
        case 'pick_response_body': return state.bodyFile??null;
        case 'create_response_asset':
        case 'create_response_asset_from_session': {
          if (state.assetDelay) await new Promise(resolve=>setTimeout(resolve,state.assetDelay));
          if (state.assetError) throw new Error('Fixture asset write failed');
          const input=args.input;
          const asset={id:input.id,revision:input.revision,status:input.status??200,headers:input.headers??[],display:input.body?new TextDecoder().decode(new Uint8Array(input.body)):'body for '+input.exchangeId,bodyBytes:input.body?.length??4,sha256:'0'.repeat(64),mediaType:input.mediaType??'application/json',provenance:command==='create_response_asset'?{kind:'authored'}:{kind:'session',exchange_id:input.exchangeId,boundary:input.boundary}};
          (state.assets??=[]).push(asset); state.lastAsset=structuredClone(input);
          return asset;
        }
        case 'validate_automation': state.candidate=structuredClone(args.document); return {candidateId:'fixture-candidate'};
        case 'activate_automation':
          state.automation={...structuredClone(state.candidate),generation:(state.automation?.generation??0)+1,candidateCount:0,historyCount:0};
          return structuredClone(state.automation);
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
  await page.goto('https://workspace.test/');
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
  const proxyBackground = async lifecycle => {
    await page.waitForFunction(lifecycle => document.querySelector('app-shell').shadowRoot.querySelector('.top-actions proxy-toggle button').dataset.lifecycle === lifecycle, lifecycle);
    await proxy.evaluate(async button => {
      // Reading the style starts any pending CSS transition; labels can update
      // before the 100ms background transition has reached its final color.
      getComputedStyle(button).backgroundColor;
      await Promise.all(button.getAnimations().map(animation => animation.finished.catch(() => {})));
    });
    return proxy.evaluate(button => getComputedStyle(button).backgroundColor);
  };
  await proxy.click();
  await page.getByRole('button',{name:'Starting…',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Starting proxy\./);
  assert.equal(await proxy.isEnabled(),false);
  await page.getByRole('button',{name:'Stop proxy',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Proxy running\./);
  const runningColor=await proxyBackground('running');
  await proxy.click();
  await page.getByRole('button',{name:'Stopping…',exact:true}).first().waitFor({state:'visible'});
  assert.match(await page.locator('.session-status').textContent(),/^Stopping proxy\./);
  await page.getByRole('button',{name:'Start proxy',exact:true}).first().waitFor({state:'visible'});
  assert.notEqual(await proxyBackground('stopped'),runningColor);
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
  assert.equal(await page.getByRole('button',{name:'Copy body',exact:true}).count(),0);
  const saveBody=responseInspector.getByRole('button',{name:'Save as…',exact:true});
  await saveBody.click();
  await responseInspector.getByRole('button',{name:'Saving…',exact:true}).waitFor({state:'visible'});
  assert.equal(await responseInspector.getByRole('button',{name:'Saving…',exact:true}).isEnabled(),false);
  await page.locator('.global-diagnostics').getByText('Saved original.webp (4 B).',{exact:true}).waitFor({state:'visible'});
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.lastSave),{sessionId:'image',boundary:'client-response'},'Save as used preview bytes or a preview URL');
  await page.evaluate(()=>{globalThis.__workspaceFixture.saveCancelled=true;});
  await saveBody.click();
  await responseInspector.getByRole('button',{name:'Saving…',exact:true}).waitFor({state:'visible'});
  await saveBody.waitFor({state:'visible'});
  assert.equal(await saveBody.isEnabled(),true,'Cancelling Save as did not restore the control');
  assert.equal(await page.locator('.global-diagnostics').textContent(),'Saved original.webp (4 B).');
  await page.evaluate(()=>{globalThis.__workspaceFixture.saveCancelled=false;globalThis.__workspaceFixture.saveError=true;});
  await saveBody.click();
  await page.locator('.notice').getByText('Response save failed',{exact:true}).waitFor({state:'visible'});
  assert.match(await page.locator('.notice').textContent(),/Fixture file write failed/);
  await page.getByRole('button',{name:'Dismiss notification',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.saveError=false;});
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
  assert.equal(await saveBody.isEnabled(),false,'Save as accepts an incomplete response');
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
  assert.equal(await saveBody.isEnabled(),false,'Save as accepts evicted response bytes');
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
  await page.locator('#auto-response-body-help').getByText(/text uses utf-8/).waitFor({state:'visible'});
  const autoEditor=page.locator('.auto-response-editor');
  const sourceLink=page.locator('.auto-response-source a');
  const autoField=name=>autoEditor.locator('[name="'+name+'"]');
  assert.equal(await autoField('body').getAttribute('rows'),'12');
  assert.equal(await autoField('body').evaluate(body=>getComputedStyle(body).resize),'vertical');
  assert.match(await autoField('body').evaluate(body=>getComputedStyle(body).fontFamily),/Mono|Consolas/);
  assert.equal(await autoField('name').evaluate(input=>input.getRootNode().activeElement===input),true,'Rule editor did not receive focus');
  await sourceLink.getByText('GET example.test/second',{exact:true}).waitFor({state:'visible'});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.waitFor({state:'hidden'});
  const capturedRuleId=await page.locator('.auto-response-rule').getAttribute('data-rule-id');
  const capturedRule=page.locator('#rule-'+capturedRuleId);
  await capturedRule.getByRole('button',{name:'Disable',exact:true}).click();
  await capturedRule.locator('.rule-state').getByText('Disabled',{exact:true}).waitFor({state:'visible'});
  await capturedRule.getByRole('button',{name:'Edit criteria',exact:true}).click();
  await sourceLink.waitFor({state:'visible'});
  assert.equal(await autoField('status').inputValue(),'200','Saved response metadata was hidden or lost');
  assert.equal(await autoField('body').isVisible(),true,'Saved response body cannot be edited independently of Traffic');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.waitFor({state:'hidden'});
  assert.equal(await capturedRule.locator('.rule-state').textContent(),'Disabled','Editing silently enabled a disabled rule');
  // Both views share one gate, preserving each rule's own enabled state.
  await page.locator('#automation autoresponse-switch button').click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.automation.autoresponsesEnabled===false);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules[0].enabled),false);
  await view('traffic');
  await page.locator('#traffic autoresponse-switch button').getByText(/Paused/).waitFor({state:'visible'});
  await page.locator('#traffic autoresponse-switch button').click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.automation.autoresponsesEnabled===true);
  await view('automation');
  await page.locator('#automation autoresponse-switch button').getByText(/On/).waitFor({state:'visible'});
  await capturedRule.getByRole('button',{name:'Edit criteria',exact:true}).click();
  await sourceLink.waitFor({state:'visible'});

  // Reveal a source on a later page even when current filters exclude it.
  const originalSourceRows=await page.evaluate(async()=>{
    const state=globalThis.__workspaceFixture;
    const rows=structuredClone(state.sessions);
    for(let index=0;index<130;index++) state.sessions.push({...rows[0],id:'source-page-'+index,path:'/filler/'+index,url:'http://example.test/filler/'+index,startedAt:2000+index});
    const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');
    traffic.pageSize='10'; traffic.sort={column:'started-at',direction:'descending'};
    traffic.filters=[{column:'status',operator:'equals',value:'304'}]; traffic.searchText='cached'; traffic.searchInput.value='cached';
    await traffic.refreshSessions(undefined,true);
    state.deferFocus=true;
    return rows;
  });
  await sourceLink.click();
  await page.waitForFunction(()=>globalThis.__workspaceFixture.focusPending);
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;state.deferRefresh=true;
    globalThis.__pendingRefresh=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);
  });
  await page.waitForFunction(()=>globalThis.__workspaceFixture.refreshPending);
  await page.evaluate(()=>globalThis.__workspaceFixture.releaseFocus());
  await page.locator('#traffic').waitFor({state:'visible'});
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').selectedSessionId==='second' && document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').pageIndex>0);
  await page.evaluate(async()=>{globalThis.__workspaceFixture.releaseRefresh();await globalThis.__pendingRefresh;});
  assert.equal(await page.locator('tr[data-session-id="second"]').getAttribute('aria-selected'),'true');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').totalMatched),originalSourceRows.length+130,'A late filtered refresh replaced the source page');
  assert.deepEqual(await page.evaluate(()=>{const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');return {filters:traffic.filters,search:traffic.searchText};}),{filters:[],search:''});
  await view('automation');
  await sourceLink.waitFor({state:'visible'});
  await page.evaluate(async rows=>{
    const state=globalThis.__workspaceFixture;
    state.sessions=rows.filter(row=>row.id!=='second'); state.evictedIds=['second'];
    await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);
  },originalSourceRows);
  await sourceLink.waitFor({state:'detached'});
  await page.locator('.auto-response-source').getByText('Source no longer in Traffic.',{exact:true}).waitFor({state:'visible'});
  assert.match(await page.locator('.auto-response-source').textContent(),/Saved response/,'Eviction removed the durable response metadata');
  await autoEditor.getByRole('button',{name:'Cancel',exact:true}).first().click();
  await capturedRule.getByRole('button',{name:'Edit criteria',exact:true}).click();
  await page.locator('.auto-response-source').getByText('Source no longer in Traffic.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await sourceLink.count(),0,'Reopening an evicted source restored a broken link');
  await autoField('body').fill('Edited after source removal');
  await autoField('status').fill('201');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastAssetEdit.decodedBody.length),'Edited after source removal'.length);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets[0].display),'body for second','Editing rewrote historical saved bytes');
  await capturedRule.getByRole('button',{name:'Edit criteria',exact:true}).click();
  await page.locator('.auto-response-source').getByText('Source no longer in Traffic.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await autoField('status').inputValue(),'201');
  assert.equal(await autoField('body').inputValue(),'Edited after source removal');
  await page.evaluate(async rows=>{
    const state=globalThis.__workspaceFixture;state.sessions=rows;state.evictedIds=[];
    const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');traffic.pageSize='100';
    await traffic.refreshSessions(undefined,true);
  },originalSourceRows);
  await sourceLink.waitFor({state:'visible'});
  await autoEditor.getByRole('button',{name:'Cancel',exact:true}).first().click();

  // Neither a slow text preview nor a slow dropped-session lookup can replace a newer draft.
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;state.deferBody='second';
    globalThis.__pendingAutoResponse=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').beginAutoResponseFromSelected();
  });
  await page.waitForFunction(()=>globalThis.__workspaceFixture.bodyPending);
  await page.getByRole('button',{name:'Create from scratch',exact:true}).click();
  await autoField('body').fill('New scratch draft');
  await page.evaluate(async()=>{globalThis.__workspaceFixture.deferBody=null;globalThis.__workspaceFixture.releaseBody();await globalThis.__pendingAutoResponse;});
  assert.equal(await autoField('body').inputValue(),'New scratch draft','Late captured body overwrote a newer draft');
  assert.equal(await autoField('body').evaluate(body=>body.readOnly),false);
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;state.deferDetail='first';
    globalThis.__pendingAutoResponse=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').beginAutoResponseFromSessionId('first');
  });
  await page.waitForFunction(()=>globalThis.__workspaceFixture.detailPending);
  await page.getByRole('button',{name:'Create from scratch',exact:true}).click();
  await autoField('name').fill('Newer lookup draft');
  await page.evaluate(async()=>{globalThis.__workspaceFixture.deferDetail=null;globalThis.__workspaceFixture.releaseDetail();await globalThis.__pendingAutoResponse;});
  assert.equal(await autoField('name').inputValue(),'Newer lookup draft','Late dropped-session lookup replaced a newer editor');

  await autoField('name').fill('Authored response');
  await autoField('url').fill('http://example.test/authored');
  await autoField('method').selectOption('POST');
  await autoField('requestHeaders').fill('Content-Type: application/json');
  await autoField('method').selectOption('GET');
  await autoField('method').selectOption('POST');
  assert.equal(await autoField('requestHeaders').inputValue(),'Content-Type: application/json','Method switch discarded the header draft');
  const assetsBeforeInvalid=await page.evaluate(()=>globalThis.__workspaceFixture.assets.length);
  await autoField('requestHeaders').fill('Not a header');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.locator('#auto-request-headers-error').waitFor({state:'visible'});
  assert.equal(await autoField('requestHeaders').evaluate(input=>input.validity.customError),true);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets.length),assetsBeforeInvalid,'Invalid matcher created an unused response asset');
  await autoField('requestHeaders').fill('Content-Type: application/json');
  await autoField('responseHeaders').fill('Also not a header');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.locator('#auto-response-headers-error').waitFor({state:'visible'});
  await autoField('responseHeaders').fill('X-Fixture: yes');
  await autoField('body').fill('Authored response body');
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetError=true;});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.getByText('Fixture asset write failed',{exact:true}).waitFor({state:'visible'});
  assert.equal(await autoField('body').inputValue(),'Authored response body','Failed save lost the body draft');
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetError=false;globalThis.__workspaceFixture.assetDelay=400;});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click({clickCount:2});
  await autoEditor.getByRole('button',{name:'Saving…',exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button',{name:'Create from scratch',exact:true}).isEnabled(),false,'Creation remains available during a save');
  assert.equal(await autoField('body').isEnabled(),false,'Body can change during a save');
  await autoEditor.waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.filter(rule=>rule.displayName==='Authored response').length),1,'Double submit created duplicate rules');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets.length),assetsBeforeInvalid+1);
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetDelay=0;});
  const authoredRuleId=await page.locator('.auto-response-rule').first().getAttribute('data-rule-id');
  const authoredRule=page.locator('#rule-'+authoredRuleId);
  await authoredRule.getByRole('button',{name:'Move Authored response down',exact:true}).click();
  await page.waitForFunction(id=>document.querySelector('app-shell').shadowRoot.querySelector('.auto-response-rule').dataset.ruleId!==id,authoredRuleId);
  assert.equal(await authoredRule.locator('.rule-order').textContent(),'2');
  const orderBeforeRemove=await page.locator('.auto-response-rule').evaluateAll(rules=>rules.map(rule=>rule.dataset.ruleId));
  await capturedRule.getByRole('button',{name:'Remove',exact:true}).click();
  await page.getByRole('button',{name:'Undo removal',exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button',{name:'Undo removal',exact:true}).evaluate(button=>button.getRootNode().activeElement===button),true,'Removal left keyboard focus on a deleted control');
  await page.getByRole('button',{name:'Undo removal',exact:true}).click();
  await capturedRule.waitFor({state:'visible'});
  assert.deepEqual(await page.locator('.auto-response-rule').evaluateAll(rules=>rules.map(rule=>rule.dataset.ruleId)),orderBeforeRemove,'Undo changed first-match order');
  assert.equal(await capturedRule.locator('.rule-state').textContent(),'Disabled','Undo changed the rule enabled state');
  await capturedRule.locator('.rule-criteria summary').click();
  assert.equal(await capturedRule.locator('.rule-criteria p').isVisible(),true);

  await page.getByRole('button',{name:'Create from scratch',exact:true}).click();
  await autoField('name').fill('Numeric account pattern');
  await autoEditor.getByLabel('URL matching',{exact:true}).selectOption('pattern');
  await autoField('url').fill('https://api.example.test/users/{:digits}');
  await autoEditor.locator('.matcher-test > summary').click();
  await autoEditor.getByLabel('Test URL',{exact:true}).fill('https://api.example.test/users/42');
  await autoEditor.getByRole('button',{name:'Check match',exact:true}).click();
  await autoEditor.getByText('This rule would serve the saved response.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastMatcherTest.matcher.url.value.address),'https://api.example.test/users/{:digits}');
  await autoEditor.locator('match-editor').evaluate(editor=>{const position=editor.addressInput.value.indexOf('{')+1;editor.addressInput.setSelectionRange(position,position);editor.inspectSelection();});
  await autoEditor.getByRole('button',{name:'Edit placeholder',exact:true}).click();
  await autoEditor.getByLabel('Optional annotation',{exact:true}).fill('account');
  await autoEditor.getByRole('button',{name:'Apply placeholder',exact:true}).click();
  assert.equal(await autoField('url').inputValue(),'https://api.example.test/users/{account:digits}');
  await autoEditor.getByRole('button',{name:'Remember as a match',exact:true}).click();
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.waitFor({state:'hidden'});
  const patternRuleId=await page.locator('.auto-response-rule').first().getAttribute('data-rule-id');
  await page.locator('#rule-'+patternRuleId).getByRole('button',{name:'Edit criteria',exact:true}).click();
  assert.equal(await autoEditor.getByLabel('URL matching',{exact:true}).inputValue(),'pattern');
  assert.equal(await autoField('url').inputValue(),'https://api.example.test/users/{account:digits}');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.find(rule=>rule.displayName==='Numeric account pattern').matcher.examples.length),1);
  await autoEditor.getByRole('button',{name:'Cancel',exact:true}).first().click();

  await page.getByRole('button',{name:'Create from scratch',exact:true}).click();
  await autoField('body').fill(Array.from({length:30},(_,index)=>'line '+(index+1)).join('\n'));
  await page.setViewportSize({width:800,height:600});
  await autoEditor.evaluate(form=>form.scrollIntoView({block:'start'}));
  const savePosition=await autoEditor.getByRole('button',{name:'Save rule',exact:true}).boundingBox();
  assert.ok(savePosition.y>=50 && savePosition.y+savePosition.height<580,'Sticky save actions are outside the small window');
  assert.ok((await autoField('body').boundingBox()).height>=240,'Twelve-line body editor has insufficient height');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-small.png')});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-dark.png')});
  await page.emulateMedia({colorScheme:'light'});
  await page.setViewportSize({width:1280,height:800});
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
