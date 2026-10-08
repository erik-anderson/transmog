import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build, Protocol } from '@microsoft/webui';
import { checkHexViewer } from './check-hex-viewer.mjs';

// Use the repository's existing, locked browser test installation.
const { chromium } = createRequire(new URL('../../../../e2e/playwright/package.json', import.meta.url))('playwright');
const root = fileURLToPath(new URL('../', import.meta.url));
const initial = JSON.parse(await readFile(resolve(root, 'src/initial-state.json'), 'utf8'));
const metadata = JSON.parse(await readFile(resolve(root, 'dist/client-metafile.json'), 'utf8'));
const assets = JSON.parse(await readFile(resolve(root, 'dist/client-assets.json'), 'utf8'));
const built = build({ appDir: resolve(root, 'src'), plugin: 'webui', css: 'link', cssPublicBase: '/', cssFileNameTemplate: '[name]-[hash].[ext]', projectionManifests: [resolve(root, 'dist/webui-projection.json')] });
assert.deepEqual(built.warnings, []);
const protocol = new Protocol(built.protocol, { plugin: 'webui' });
const renderHtml = (viewerMode=false) => protocol.render({ ...initial, viewerMode, language: 'en', pageTitle: 'Transmog', heading: 'Inspect traffic without losing the thread', lifecycleLabel: 'Stopped', lifecycleKind: 'stopped', listener: 'Not listening' }).toString().replace(/<script(?=[\s>])/g, '<script nonce="workspace-check"');
// Keep one build's bytes for the whole run, even if another workspace rebuilds dist.
const resources = new Map(await Promise.all(assets.map(async (asset) => [asset.path, { body: await readFile(resolve(root, 'dist', asset.file)), contentType: asset.contentType }])));
resources.set('/document.css', { file: resolve(root, 'src/document.css'), contentType: 'text/css' });
resources.set('/transmog-icon.svg', { file: resolve(root, '../icons/icon.svg'), contentType: 'image/svg+xml' });
for (let index = 0; index < built.cssFiles.length; index += 2) resources.set('/' + built.cssFiles[index], { body: built.cssFiles[index + 1], contentType: 'text/css' });
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.context().grantPermissions(['clipboard-read','clipboard-write'],{origin:'https://workspace.test'});
const errors = [];
const requests = [];
page.on('pageerror', (error) => errors.push(error.message));
page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
page.on('request', (request) => requests.push(new URL(request.url()).pathname));
await page.route('https://workspace.test/**', async (route) => {
  const path = new URL(route.request().url()).pathname;
  if (path === '/') {
    await route.fulfill({ contentType: 'text/html', body: renderHtml(new URL(route.request().url()).searchParams.has('viewer')), headers: { 'Content-Security-Policy': "default-src 'none'; base-uri 'none'; object-src 'none'; script-src 'self' 'nonce-workspace-check'; worker-src 'self'; style-src 'self' 'unsafe-inline'; font-src data:; img-src 'self' data:; connect-src 'self'; require-trusted-types-for 'script'; trusted-types webui monaco" } });
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
    return { id, performance:state.performance, savedEvidence:state.savedTiming, traceId:row.traceId??null, startedAt:row.startedAt, caller, requests: [{ boundary: 'client-request', method: 'GET', target: row.url, status: null, protocol: 'HTTP/1.1', headers: [{name:'Accept',value:'*/*',valueBytes:3,fieldBytes:13,sensitive:false,binary:false},{name:'Authorization',value:'[redacted]',valueBytes:5133,fieldBytes:5150,sensitive:true,binary:false}] }], responses: [{ boundary: 'client-response', method: null, target: null, status: row.status, protocol: 'HTTP/1.1', headers: [{name:'Content-Type',value:row.contentType,sensitive:false,binary:false}] }], bodies: [], storedBodies: [{ exchangeId: id, boundary: 'client-response', observedBytes: row.responseBytes, retainedBytes: row.responseBytes, availability: 'complete', mediaType: row.contentType, charset: 'utf-8', contentCodings: [], sha256: null, reason: null }], diagnostics: [], hookEffects: [], routeSelection: null, routeAttempts: [], terminal: row.terminal, websocket: null, sequenceLoss: 0, autoResponse: null };
  };
  const state = globalThis.__workspaceFixture = { calls: {}, workspace:JSON.parse(localStorage.getItem('workspace')??JSON.stringify(workspace)), lifecycle:'stopped', sessions: ['first','second','cached','image'].map(summary), paused: [], queryDelay: 0, slowDetail: false, slowBody:false };
  const ruleDiagnostics=()=>{
    const rules=state.automation?.rules??[];const earlier=new Map();const diagnostics=[];
    for(const rule of [...rules].filter(rule=>rule.request.responseAsset).sort((a,b)=>a.priority-b.priority)) {
      const matcher=structuredClone(rule.matcher);delete matcher.examples;
      if(matcher.url?.kind==='pattern')matcher.url.value.address=matcher.url.value.address.replace(/\{[a-zA-Z0-9_]*(:digits|\.\.\.)?\}/g,(_,kind)=>'{'+(kind??'')+'}');
      const key=JSON.stringify(matcher);const winner=earlier.get(key);
      if(winner){const asset=reference=>state.assets?.find(asset=>asset.id+'@'+asset.revision===reference);const left=asset(rule.request.responseAsset),right=asset(winner.request.responseAsset);diagnostics.push({ruleId:rule.id,supersededBy:winner.id,duplicateResponse:left?.display===right?.display && left?.status===right?.status});}
      else if(rule.enabled!==false)earlier.set(key,rule);
    }
    if(state.automation)state.automation.diagnostics=diagnostics;
  };
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
        case 'take_opened_traces': {const paths=state.openedTraces??[];state.openedTraces=[];return paths;}
        case 'pick_trace_path': return state.pickedTrace??null;
        case 'open_trace_viewer': state.openedViewer=structuredClone(args.paths);return 'viewer-fixture';
        case 'open_main_window': return;
        case 'save_traffic_trace': state.savedTraceArgs=structuredClone(args);if(state.saveTraceError)throw new Error('Fixture trace write failed');return state.cancelTraceSave?null:{destination:'C:/captures/shared.tmcap.gz',entries:state.sessions.length,bytes:1234,incompleteBodies:0};
        case 'trace_metadata_list': return structuredClone(state.traces??[]);
        case 'cancel_trace_import': state.importCanceled=true;return;
        case 'import_trace': {
          state.lastImport=structuredClone(args.request);state.importCanceled=false;
          args.onProgress.onmessage({operationId:args.request.operationId,completed:1,total:2});
          if(state.deferImport)await new Promise(resolve=>state.releaseImport=resolve);
          if(state.importCanceled)throw new Error('Trace import canceled');
          const trace={id:'trace-'+(state.traces?.length??0),name:args.request.path.split(/[\\/]/).pop(),format:'saz',path:args.request.path,sessions:1,importedAt:Date.now(),context:{networkContext:'Captured machine IP configuration'},notes:['Original trace note']};
          const row={...summary('imported-'+trace.id),traceId:trace.id,durationMs:null,startedAt:Date.now()};
          state.sessions.push(row);(state.traces??=[]).push(trace);return {trace,issues:[]};
        }
        case 'desktop_bootstrap': return { viewerMode:location.search.includes('viewer'), windows:true, caCertificatePath: 'fixture.pem', caPrivateKeyPath: 'fixture.key', caFilesPresent: true, caFilesExist: true, ownedCaSha256: '0'.repeat(64), ownedCaTrusted: true, hostRestorePending: false, diagnosticsPath: 'fixture.jsonl', ...state.caBootstrap };
        case 'reset_ca': {
          state.caEvents??=[];state.caEvents.push('reset');state.resetCaArgs=structuredClone(args);
          if(state.deferCaReset){state.caResetPending=true;await new Promise(resolve=>state.releaseCaReset=resolve);state.deferCaReset=false;}
          if(state.caResetError)throw new Error('Fixture CA file deletion failed');
          state.caBootstrap={caFilesPresent:false,caFilesExist:false,ownedCaSha256:null,ownedCaTrusted:false};return;
        }
        case 'create_ca': {
          state.caEvents??=[];state.caEvents.push('create');state.lastCaCreate=structuredClone(args.request);
          if(state.caBootstrap?.caFilesExist || state.caBootstrap?.ownedCaSha256)throw new Error('Fixture CA destination or identity already exists');
          state.caBootstrap={caFilesPresent:true,caFilesExist:true,ownedCaSha256:'1'.repeat(64),ownedCaTrusted:false};
          return {sha256:'1'.repeat(64),certificatePath:args.request.certificatePath};
        }
        case 'install_certificate': {
          state.caEvents??=[];state.caEvents.push('install');state.lastCaInstall=structuredClone(args);
          if(state.caTrustError)throw new Error('Fixture Windows trust canceled');
          state.caBootstrap={...state.caBootstrap,ownedCaTrusted:true};return;
        }
        case 'product_state': if(state.savedProduct)return {...structuredClone(state.savedProduct),workspace:structuredClone(state.workspace)};return { schemaVersion: 4, workspace:structuredClone(state.workspace), preferences: { theme: 'system', sessionPageSize: 100, configureSystemProxy: true }, privacy: { retainResponseBodies: true, retainBodySamples: false, rememberRecentArtifacts: false, includePathsInSupportBundles: false }, window: {}, recentArtifacts: [] };
        case 'save_workspace_preferences': state.workspace = structuredClone(args.preferences); localStorage.setItem('workspace',JSON.stringify(state.workspace)); return state.workspace;
        case 'app_status': if(state.statusError)throw new Error('Fixture status unavailable');return { lifecycle: state.lifecycle, listener: state.lifecycle==='running'?'127.0.0.1:8888':null, summary: 'Proxy '+state.lifecycle, hostRestorePending: false };
        case 'start_proxy': await new Promise(resolve => setTimeout(resolve,80)); state.lifecycle='running'; return {lifecycle:'running',listener:'127.0.0.1:8888',summary:'Proxy running',hostRestorePending:false};
        case 'stop_application': await new Promise(resolve => setTimeout(resolve,80));if(state.testDrain){state.lifecycle='draining';return {lifecycle:'draining',listener:'127.0.0.1:8888',summary:'Finishing 1 active request/connection',hostRestorePending:false};}state.lifecycle='stopped'; return {lifecycle:'stopped',listener:null,summary:'Proxy stopped',hostRestorePending:false};
        case 'resume_application': state.lifecycle='running';return {lifecycle:'running',listener:'127.0.0.1:8888',summary:'Proxy resumed',hostRestorePending:false};
        case 'search_traffic': {
          const request=args.request;state.lastContentSearch=structuredClone(request);state.searchCanceled=false;
          args.onProgress.onmessage({operationId:request.operationId,completed:0,total:state.sessions.length});
          if(state.deferSearch)await new Promise(resolve=>state.releaseSearch=resolve);
          if(state.searchCanceled)throw new Error('Traffic search canceled');
          const normalize=text=>{text=String(text);if(request.ignoreDiacritics)text=text.normalize('NFD').replace(/\p{M}/gu,'');return request.caseSensitive?text:text.toLowerCase();};
          const regex=request.mode==='regex'?new RegExp(request.pattern,request.caseSensitive?'u':'iu'):null;
          const match=text=>regex?regex.test(String(text)):normalize(text).includes(normalize(request.pattern));
          const ids=state.sessions.filter(row=>!state.dismissedIds?.includes(row.id)).filter(row=>request.metadata&&match(row.url+' '+row.method+' '+row.caller.processName+' '+row.caller.processId+' '+row.status)||request.headers&&match('X-Support: HeaderNeedle '+row.id)||request.responseBodies&&row.contentType!=='image/webp'&&match(row.searchBody??('body for '+row.id))).map(row=>row.id);
          const id='search-'+(state.searchResults?.length??0);(state.searchResults??=[]).push({id,ids});return {id,operationId:request.operationId,ids,examined:state.sessions.length,binaryBodies:1,unavailableBodies:0};
        }
        case 'cancel_traffic_search': state.searchCanceled=true;return;
        case 'matching_traffic_ids': {
          const result=state.searchResults?.find(result=>result.id===args.query.searchResultId);let rows=state.sessions.filter(row=>!state.dismissedIds?.includes(row.id)&&(!result||result.ids.includes(row.id)));
          for(const filter of args.query.filters??[])rows=rows.filter(row=>filter.column==='status'?String(row.status)===filter.value:filter.column==='host'?row.host.includes(filter.value):true);
          return rows.map(row=>row.id);
        }
        case 'remove_unselected_traffic_entries': {
          state.dismissedIds??=[];const changed=state.sessions.map(row=>row.id).filter(id=>!args.ids.includes(id)&&!state.dismissedIds.includes(id));state.dismissedIds.push(...changed);return changed;
        }
        case 'watch_sessions': state.channel = args.onEvent; return;
        case 'query_sessions': {
          if (state.queryError) throw new Error('Fixture query failure');
          if (state.deferFocus && args.query.focusId) { state.deferFocus=false; state.focusPending=true; await new Promise(resolve=>state.releaseFocus=resolve); }
          if (state.deferRefresh && !args.query.focusId) { state.deferRefresh=false; state.refreshPending=true; await new Promise(resolve=>state.releaseRefresh=resolve); }
          await new Promise((resolve) => setTimeout(resolve, state.queryDelay)); state.lastQuery=args.query;
          const key = (row, column) => ({method:row.method,status:row.status,process:row.caller.processName,host:row.host,path:row.path,duration:row.durationMs,'response-bytes':row.responseBytes,'started-at':row.startedAt,pid:row.caller.processId,url:row.url})[column];
          let rows = structuredClone(state.sessions).filter(row => (!args.query.searchResultId||state.searchResults?.find(result=>result.id===args.query.searchResultId)?.ids.includes(row.id)) && !state.dismissedIds?.includes(row.id) && (!args.query.search || (row.url+' '+row.caller.processName+' '+row.caller.processId).toLowerCase().includes(args.query.search.toLowerCase())));
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
        case 'composer_source': {
          if(state.sourceDelay)await new Promise(resolve=>setTimeout(resolve,state.sourceDelay));
          const request=detail(args.id).requests[0];return {method:request.method,url:request.target,headers:request.headers.filter(header=>!header.sensitive),body:'',bodyAvailable:true,notices:[]};
        }
        case 'request_command': state.lastRequestCommand=args;return state.directCommand ? {text:'fixture direct '+args.format,notices:[],bodyFileRequired:false,bodyFileAvailable:true} : {text:'fixture generated '+args.format,notices:['The command needs a request body file.'],bodyFileRequired:true,bodyFileAvailable:true};
        case 'save_request_body': state.savedRequestBody=args;return {text:'fixture saved body command',notices:[],bodyFileRequired:true,bodyFileAvailable:true};
        case 'copy_all_headers': return 'GET http://example.test/second HTTP/1.1\r\nAccept: */*\r\n\r\n\r\nHTTP/1.1 200 OK';
        case 'session_detail':
          if (state.slowDetail && args.id === 'first') await new Promise((resolve) => setTimeout(resolve, 100));
          if (state.evictedIds?.includes(args.id) || state.dismissedIds?.includes(args.id)) throw new Error('Session is unavailable or has been evicted');
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
          const bytes=state.hexBytes??[0,0x41,0xff,0x20];
          const chunks=[];
          if(representation==='bytes')for(let start=0;start<bytes.length;start+=8192)chunks.push(String.fromCharCode(...bytes.slice(start,start+8192)));
          return { metadata, representation, decoded:args.request.decodeContent, textEncoding:'utf-8', display:representation==='formatted-json'?'{\n  "fixture": true\n}':'body for '+args.request.sessionId, displayBytes:representation==='bytes'?bytes.length:4,bytesBase64:representation==='bytes'?btoa(chunks.join('')):null,byteOffset:0,truncated:state.hexTruncated??false,nextOffset:null,warning:null,previewHandle:representation==='image'?'pixel.png':null,previewMimeType:representation==='image'?'image/png':null };
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
          return {test:{matched,normalizedUrl:request.href,checks,captures},wouldServe:matched && args.input.enabled && state.automation?.autoresponsesEnabled!==false,explanation:matched?(args.input.enabled?'This rule would serve the saved response.':'The request matches, but this rule is disabled.'):'This request does not match the draft rule.',winningRule:null,examples:matcher.examples?.map(item=>({method:item.method,url:item.url,passed:true}))??[]};
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
        case 'remove_traffic_entries': {
          const dismissed=new Set(state.dismissedIds??[]);const changed=[];
          for(const id of args.ids)if(state.sessions.some(row=>row.id===id) && (args.restore?dismissed.has(id):!dismissed.has(id))) {if(args.restore)dismissed.delete(id);else dismissed.add(id);changed.push(id);}
          state.dismissedIds=[...dismissed];return changed;
        }
        case 'create_autoresponse_batch': {
          if(state.batchError)throw new Error('Fixture batch storage failed');
          if(state.automation.generation!==args.input.generation)throw new Error('Rules changed during batch review');
          const rules=[],createdIds=[];state.assets??=[];
          for(const [index,id] of args.input.ids.entries()) {
            if(state.evictedIds?.includes(id) || state.dismissedIds?.includes(id))throw new Error('Response was evicted during review');
            const assetId='batch-asset-'+state.assets.length;const asset={id:assetId,revision:1,status:detail(id).responses[0].status,headers:[],display:'body for '+id,bodyBytes:4,sha256:'0'.repeat(64),mediaType:'application/json',provenance:{kind:'session',exchange_id:id,boundary:'client-response'}};
            state.assets.push(asset);const ruleId='batch-rule-'+assetId;createdIds.push(ruleId);
            const matcher={method:'GET',url:{kind:'exact',value:detail(id).requests[0].target},scheme:null,host:null,port:null,pathPrefix:null,query:null,requestHeaders:[],responseHeaders:[],responseStatus:null,responseStatusClass:null};
            rules.push({id:ruleId,displayName:'GET '+detail(id).requests[0].target,enabled:true,revision:1,priority:-1000000+index,matcher,request:{headers:[],replaceBody:null,discardBody:false,abortReason:null,responseAsset:assetId+'@1',allowNonIdempotentBodyReplacement:false},response:{headers:[],replaceBody:null,discardBody:false,abortReason:null}});
          }
          const existing=[...state.automation.rules].sort((a,b)=>a.priority-b.priority).map((rule,index)=>({...rule,priority:-1000000+rules.length+index,revision:rule.revision+1}));
          state.automation={...state.automation,generation:state.automation.generation+1,rules:[...rules,...existing]};ruleDiagnostics();return {status:structuredClone(state.automation),createdIds};
        }
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
          ruleDiagnostics();
          return structuredClone(state.automation);
        case 'save_product_state': state.savedProduct=structuredClone(args.productState);return structuredClone(state.savedProduct);
        case 'pick_support_path': return state.supportPick??null;
        case 'diagnostics_report': return {applicationVersion:'0.1.0',runtime:{operatingSystem:'Windows fixture',architecture:'x64',webviewVersion:'test'},events:[{level:'warning',message:'safe fixture'}],privacyNotice:'Traffic bodies and credentials are omitted'};
        case 'create_support_bundle': state.supportInput=structuredClone(args);return {destination:args.destination,bytes:4096,includedRecentPaths:args.includeRecentPaths};
        case 'prepare_update_handoff': state.lifecycle='stopped';return {lifecycle:'stopped',listener:null,summary:'Proxy stopped',hostRestorePending:false};
        case 'capture_status': return structuredClone(state.capture??{state:'idle'});
        case 'start_capture': state.lastCaptureRequest=structuredClone(args.request);state.capture={state:'active',path:args.request.path,bytesWritten:2048};return structuredClone(state.capture);
        case 'stop_capture': state.capture={...state.capture,state:'sealed'};return structuredClone(state.capture);
        case 'pick_capture_path': state.lastCapturePick=structuredClone(args);return state.capturePick??null;
        case 'import_capture': state.lastCaptureImport=structuredClone(args.request);return {records:12,exchanges:3,lossMarkers:1,retainedBodyBytes:2048,sealed:false,truncatedTail:true,validBytes:4096};
        case 'export_capture': state.lastCaptureExport=structuredClone(args.request);return {destination:args.request.destination,records:12,bytes:4096,sourceSealed:false,sourceTruncatedTail:true,fidelity:'Valid prefix preserved'};
        case 'execute_composer': state.composerInput=structuredClone(args.request);if(state.deferComposer){state.composerPending=true;await new Promise(resolve=>state.finishComposer=resolve);}else await new Promise(resolve=>setTimeout(resolve,80));if(state.composerError)throw new Error('Fixture send failed');return {id:1,status:201,headers:[{name:'X-Duplicate',value:'first'},{name:'X-Duplicate',value:'second'}],body:'<script>globalThis.unsafeComposer=true</script>',bodyIsHex:false,truncated:false,attribution:'composer'};
        case 'script_declarations': return '';
        case 'script_status': return {generation:0,active:state.activeScripts??[],saved:state.savedScript?[state.savedScript]:[],candidateCount:0,historyCount:0};
        case 'save_script': state.savedScript=structuredClone(args.draft);return {generation:1,active:state.activeScripts??[],saved:[state.savedScript],candidateCount:0,historyCount:0};
        case 'validate_script': state.scriptCandidate=structuredClone(args.draft);if(state.deferScriptValidation){state.scriptValidationPending=true;await new Promise(resolve=>state.finishScriptValidation=resolve);}return {candidateId:'fixture-script',scriptId:args.draft.id,revision:args.draft.revision,sourceHash:'0'.repeat(64)};
        case 'test_script': state.scriptTestInput=structuredClone(args.invocation);return {action:'continue'};
        case 'activate_script': state.activeScripts=[{manifest:{id:state.scriptCandidate.id,revision:state.scriptCandidate.revision,sourceHash:'0'.repeat(64)},source:state.scriptCandidate.source}];return {generation:1,active:state.activeScripts,saved:state.savedScript?[state.savedScript]:[],candidateCount:0,historyCount:0};
        case 'disable_script': state.activeScripts=[];return {generation:2,active:[],saved:state.savedScript?[state.savedScript]:[],candidateCount:0,historyCount:0};
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
const discardIfAsked=async()=>{const dialog=page.locator('automation-workspace .unsaved-rule-dialog');if(await dialog.isVisible())await dialog.getByRole('button',{name:'Discard changes',exact:true}).click();};
const newScratch=async()=>{await page.locator('.workspace-tabs').getByRole('button',{name:'Auto-responses',exact:true}).click();await page.getByRole('button',{name:'More autoresponse options',exact:true}).click();await page.getByRole('button',{name:'Create from scratch',exact:true}).click();await discardIfAsked();await page.locator('.auto-response-editor').waitFor({state:'visible'});};
const savedProperties=async()=>{await page.waitForFunction(()=>{const workspace=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');return !workspace.savingAutoResponse && workspace.existingResponse && !workspace.draftDirty;});};
const cancelRule=async()=>{await page.locator('.auto-response-editor').getByRole('button',{name:'Cancel',exact:true}).first().click();await discardIfAsked();};
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
  assert.equal(await page.locator('.traffic-table tr[data-session-id][draggable]').count(),0,'Traffic rows still advertise cross-tab dragging');
  await view('automation');
  assert.equal(requests.some(path=>path==='/monaco.js'),false,'Response management eagerly loaded the script editor');
  await page.getByRole('button',{name:'Choose responses in Traffic',exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button',{name:'Create from scratch',exact:true}).isVisible(),false,'Scratch authoring is prominent');
  assert.match(await page.locator('.auto-response-start-hint').textContent(),/Select one or more captured responses in Traffic/);
  await page.getByRole('button',{name:'Choose responses in Traffic',exact:true}).click();
  await page.locator('#traffic').waitFor({state:'visible'});
  // Populated lists must leave bulk actions reachable by an actual mouse click.
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;state.reviewOriginalRows=state.sessions;state.sessions=Array.from({length:60},(_,index)=>({...state.sessions[1],id:'layout-'+index,path:'/layout/'+index,url:'http://example.test/layout/'+index,startedAt:2000+index}));await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);});
  await page.locator('tr[data-session-id="layout-59"]').click();
  await page.keyboard.press('Control+A');
  const bulkAction=page.locator('.traffic-selection-bar').getByRole('button',{name:/^Create \d+ auto-responses?…$/});
  const selectionBounds=await page.locator('.traffic-selection-bar').boundingBox();
  const tableBounds=await page.locator('.table-wrap').boundingBox();
  assert.ok(selectionBounds.y+selectionBounds.height<=tableBounds.y+1,'Traffic table overlaps bulk actions');
  await bulkAction.click();
  await page.locator('.batch-review').waitFor({state:'visible'});
  await page.getByRole('button',{name:'Cancel batch',exact:true}).click();
  await view('traffic');
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;state.sessions=state.reviewOriginalRows;delete state.reviewOriginalRows;const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');traffic.clearTrafficSelection();await traffic.refreshSessions(undefined,true);});
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
  await page.getByRole('button',{name:'Table settings',exact:true}).click();
  await page.getByLabel('Traffic layout',{exact:true}).selectOption('side-by-side');
  await page.keyboard.press('Escape');
  const panes = await page.locator('.traffic-workspace').evaluate(grid=>({list:grid.querySelector('.session-list-pane').getBoundingClientRect().toJSON(),details:grid.querySelector('.details-pane').getBoundingClientRect().toJSON()}));
  assert.ok(panes.details.x>panes.list.x+panes.list.width,'Side-by-side layout did not arrange panes horizontally');
  await page.getByRole('button',{name:'Table settings',exact:true}).click();
  await page.getByLabel('Traffic layout',{exact:true}).selectOption('stacked');
  await page.keyboard.press('Escape');
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


  await page.evaluate(async()=>{globalThis.__workspaceFixture.testDrain=true;globalThis.__workspaceFixture.lifecycle='running';await document.querySelector('app-shell').refreshStatus();});
  await proxy.click();
  await page.waitForFunction(()=>{const shell=document.querySelector('app-shell');return shell.lifecycleKind==='draining'&&!shell.proxyPending;});
  assert.equal(await page.getByRole('button',{name:'Start proxy',exact:true}).first().isEnabled(),true,'An in-flight drain prevented resume');
  await page.getByRole('button',{name:'Start proxy',exact:true}).first().click();
  await page.waitForFunction(()=>{const shell=document.querySelector('app-shell');return shell.lifecycleKind==='running'&&!shell.proxyPending;});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.resume_application),1);
  await page.evaluate(()=>globalThis.__workspaceFixture.testDrain=false);
  await proxy.click();await page.getByRole('button',{name:'Start proxy',exact:true}).first().waitFor();
  await page.evaluate(async()=>{globalThis.__workspaceFixture.statusError=true;await document.querySelector('app-shell').refreshStatus();});
  await page.locator('.global-diagnostics').getByText('Status unavailable: Fixture status unavailable',{exact:true}).waitFor({state:'visible'});
  await page.evaluate(async()=>{globalThis.__workspaceFixture.statusError=false;await document.querySelector('app-shell').refreshStatus();});

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
  await page.locator('form.filters button[type=submit]').click();
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
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('#inspector-heading').textContent.endsWith('/second'));
  assert.equal(await page.locator('tr[data-session-id="second"]').getAttribute('aria-selected'), 'true');
  assert.match(await page.locator('.list-footer').textContent(),/Inspection pinned · showing captured traffic/);
  await page.locator('message-inspector[side="response"]').getByText('Auto · JSON',{exact:true}).waitFor({state:'visible'});
  assert.match(await page.locator('message-inspector[side="response"] .body-preview').textContent(), /"fixture": true/);
  const requestHeaders=page.locator('message-inspector[side="request"]');
  assert.equal(await requestHeaders.getByText('AUTH',{exact:true}).isVisible(),true);
  assert.match(await requestHeaders.locator('.field-help').first().textContent(),/2 fields.*HTTP\/1 equivalent/);
  await requestHeaders.getByRole('button',{name:'Largest first',exact:true}).click();
  assert.match(await requestHeaders.locator('.headers-table tbody tr').first().textContent(),/Authorization.*5.0 KB/);
  assert.equal(await requestHeaders.getByRole('button',{name:'Largest first',exact:true}).getAttribute('aria-pressed'),'true');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/header-sizes-wide.png')});
  await page.setViewportSize({width:760,height:520});
  await page.getByRole('button',{name:'Request',exact:true}).click();
  assert.equal(await requestHeaders.getByRole('button',{name:'Largest first',exact:true}).isVisible(),true);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/header-sizes-small.png')});
  await page.setViewportSize({width:1280,height:800});
  await page.locator('tr[data-session-id="second"]').click({button:'right'});
  await page.getByRole('button',{name:'Copy as cURL',exact:true}).click();
  const commandDialog=page.locator('.request-command-dialog');
  await commandDialog.getByText('Command copied. Supply the body file before running it.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await commandDialog.getByLabel('Generated command').inputValue(),'fixture generated curl-windows');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.execute_composer??0),0,'Copying a command executed a request');
  await commandDialog.getByRole('button',{name:'Save as…',exact:true}).click();
  await commandDialog.getByText('Request body saved. Updated command copied.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await commandDialog.getByLabel('Generated command').inputValue(),'fixture saved body command');
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.savedRequestBody),{id:'second',format:'curl-windows'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/request-command-wide.png')});
  await page.setViewportSize({width:760,height:520});
  assert.equal(await commandDialog.getByRole('button',{name:'Close',exact:true}).isVisible(),true);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/request-command-small.png')});
  await commandDialog.getByRole('button',{name:'Close',exact:true}).click();
  await page.setViewportSize({width:1280,height:800});
  await page.evaluate(()=>globalThis.__workspaceFixture.directCommand=true);
  await page.locator('tr[data-session-id="second"]').click({button:'right'});
  await page.getByRole('button',{name:'Copy as PowerShell',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').commandBusy);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastRequestCommand.format),'powershell');
  assert.equal(await commandDialog.isVisible(),false,'A direct clipboard command unnecessarily opened the body-save dialog');
  assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),'fixture direct powershell');
  await page.locator('tr[data-session-id="second"]').click({button:'right'});
  await page.getByRole('button',{name:'Copy as cURL',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').commandBusy);
  assert.equal(await commandDialog.isVisible(),false);
  assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),'fixture direct curl-windows');
  await page.getByRole('button',{name:'Edit and replay',exact:true}).click();
  await page.locator('#composer').waitFor({state:'visible'});
  assert.equal(await page.locator('#composer input[name="url"]').inputValue(),'http://example.test/second');
  assert.equal(await page.locator('#composer textarea[name="headers"]').inputValue(),'Accept: */*');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.execute_composer??0),0,'Replay ran before explicit execution');
  await page.locator('#composer input[name="url"]').fill('http://example.test/edited-request');
  await view('traffic');await page.locator('tr[data-session-id="first"]').click();await page.getByRole('button',{name:'Edit and replay',exact:true}).click();
  await page.locator('composer-workspace dialog').getByRole('button',{name:'Keep editing',exact:true}).click();
  assert.equal(await page.locator('#composer input[name="url"]').inputValue(),'http://example.test/edited-request','Loading another request replaced edited content');
  await view('traffic');await page.getByRole('button',{name:'Edit and replay',exact:true}).click();await page.locator('composer-workspace dialog').getByRole('button',{name:'Replace draft',exact:true}).click();
  await page.locator('#composer input[name="url"]').getAttribute('value');
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('composer-workspace').composerSourceId==='first');
  await page.locator('#composer input[name="method"]').fill('POST');
  assert.equal(await page.locator('#composer input[name="nonIdempotent"]').isVisible(),true);
  await page.locator('#composer input[name="method"]').fill('GET');
  assert.equal(await page.locator('#composer input[name="nonIdempotent"]').isVisible(),false);
  await page.locator('#composer textarea[name="headers"]').fill('  Authorization: explicit');
  assert.equal(await page.locator('#composer input[name="credentials"]').isVisible(),true);
  await page.locator('#composer textarea[name="headers"]').fill('Accept: */*');
  await page.evaluate(()=>globalThis.__workspaceFixture.composerError=true);
  await page.getByRole('button',{name:'Send request',exact:true}).click();
  await page.locator('.composer-feedback').getByText('Send failed: Fixture send failed',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.locator('#composer input[name="url"]').inputValue(),'http://example.test/first');
  await page.evaluate(()=>{globalThis.__workspaceFixture.composerError=false;globalThis.__workspaceFixture.deferComposer=true;});
  await page.getByRole('button',{name:'Send request',exact:true}).click();await page.waitForFunction(()=>globalThis.__workspaceFixture.composerPending);
  assert.equal(await page.getByRole('button',{name:'Sending…',exact:true}).isDisabled(),true);
  await page.locator('#composer textarea[name="body"]').fill('new draft while sending');
  await page.evaluate(()=>globalThis.__workspaceFixture.finishComposer());
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('composer-workspace').composerResult?.status===201);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('composer-workspace').composerPane),'request','Late responses interrupted request editing');
  assert.match(await page.locator('.composer-result-body').first().textContent(),/<script>/);
  assert.equal(await page.evaluate(()=>globalThis.unsafeComposer),undefined,'Response markup executed');
  await page.locator('.composer-result-tabs').getByRole('button',{name:'Headers',exact:true}).click();
  assert.equal(await page.locator('.composer-result-headers tbody tr').count(),2,'Duplicate response headers were combined');
  await page.locator('.composer-result-tabs button[data-selected]').getByText('Headers',{exact:true}).waitFor({state:'visible'});
  const bodySpacing=await page.locator('.composer-body').evaluate(label=>{const range=document.createRange();range.selectNodeContents(label.firstElementChild);return label.querySelector('textarea').getBoundingClientRect().top-range.getBoundingClientRect().bottom;});
  assert.ok(bodySpacing<24, 'Body label drifted away from its editor');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/composer-wide.png')});
  await page.setViewportSize({width:760,height:520});
  await page.locator('.composer-pane-tabs').getByRole('button',{name:'Response',exact:true}).click();
  await page.locator('.composer-pane-tabs button[data-selected]').getByText('Response',{exact:true}).waitFor({state:'visible'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/composer-small.png')});
  await page.setViewportSize({width:1280,height:800});

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
  await responseInspector.locator('.body-facts strong').getByText('Hex',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastBodyRequest.decodeContent),false,'Hex tried to decode an incomplete compressed body');
  assert.equal(await responseInspector.getByLabel('Decode',{exact:true}).isChecked(),false);
  assert.equal(await responseInspector.getByLabel('Decode',{exact:true}).isEnabled(),false);
  await checkHexViewer(page,responseInspector,resolve(root,'../../../target/ui-check/hex-selection.png'));
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
  assert.equal(await page.getByRole('button',{name:'Use selected response',exact:true}).isVisible(),true);
  assert.equal(await page.getByRole('button',{name:'Create from scratch',exact:true}).isVisible(),false);
  assert.equal(await autoField('body').getAttribute('rows'),'12');
  assert.equal(await autoField('body').evaluate(body=>getComputedStyle(body).resize),'vertical');
  assert.match(await autoField('body').evaluate(body=>getComputedStyle(body).fontFamily),/Mono|Consolas/);
  assert.equal(await autoField('name').evaluate(input=>input.getRootNode().activeElement===input),true,'Rule editor did not receive focus');
  await sourceLink.getByText('GET example.test/second',{exact:true}).waitFor({state:'visible'});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await savedProperties();
  const capturedRuleId=await page.locator('.auto-response-rule').getAttribute('data-rule-id');
  const capturedRule=page.locator('#rule-'+capturedRuleId);
  await autoField('enabled').uncheck();
  await capturedRule.locator('.rule-state').getByText('Disabled',{exact:true}).waitFor({state:'visible'});
  await capturedRule.click();
  await sourceLink.waitFor({state:'visible'});
  assert.equal(await autoField('status').inputValue(),'200','Saved response metadata was hidden or lost');
  assert.equal(await autoField('body').isVisible(),true,'Saved response body cannot be edited independently of Traffic');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await savedProperties();
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
  await capturedRule.click();
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
  await cancelRule();
  await capturedRule.click();
  await page.locator('.auto-response-source').getByText('Source no longer in Traffic.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await sourceLink.count(),0,'Reopening an evicted source restored a broken link');
  await autoField('body').fill('Edited after source removal');
  await autoField('status').fill('201');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await savedProperties();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastAssetEdit.decodedBody.length),'Edited after source removal'.length);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets[0].display),'body for second','Editing rewrote historical saved bytes');
  await capturedRule.click();
  await page.locator('.auto-response-source').getByText('Source no longer in Traffic.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await autoField('status').inputValue(),'201');
  assert.equal(await autoField('body').inputValue(),'Edited after source removal');
  await page.evaluate(async rows=>{
    const state=globalThis.__workspaceFixture;state.sessions=rows;state.evictedIds=[];
    const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');traffic.pageSize='100';
    await traffic.refreshSessions(undefined,true);
  },originalSourceRows);
  await sourceLink.waitFor({state:'visible'});
  await cancelRule();

  // A slow captured-response preview cannot replace a newer draft.
  await page.evaluate(()=>{
    const state=globalThis.__workspaceFixture;state.deferBody='second';
    globalThis.__pendingAutoResponse=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').beginAutoResponseFromSelected();
  });
  await page.waitForFunction(()=>globalThis.__workspaceFixture.bodyPending);
  await newScratch();
  await autoField('body').fill('New scratch draft');
  await page.evaluate(async()=>{globalThis.__workspaceFixture.deferBody=null;globalThis.__workspaceFixture.releaseBody();await globalThis.__pendingAutoResponse;});
  assert.equal(await autoField('body').inputValue(),'New scratch draft','Late captured body overwrote a newer draft');
  assert.equal(await autoField('body').evaluate(body=>body.readOnly),false);
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
  if(!await autoEditor.locator('details:has([name="responseHeaders"])').evaluate(details=>details.open))await autoEditor.locator('details:has([name="responseHeaders"])').locator('summary').first().click();
  await autoField('responseHeaders').fill('Also not a header');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await page.locator('#auto-response-headers-error').waitFor({state:'visible'});
  if(!await autoEditor.locator('details:has([name="responseHeaders"])').evaluate(details=>details.open))await autoEditor.locator('details:has([name="responseHeaders"])').locator('summary').first().click();
  await autoField('responseHeaders').fill('X-Fixture: yes\nContent-Length: 999\nContent-Encoding: gzip\nContent-Type: incorrect/type');
  await autoField('body').fill('Authored response body');
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetError=true;});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();
  await autoEditor.getByText('Fixture asset write failed',{exact:true}).waitFor({state:'visible'});
  assert.equal(await autoField('body').inputValue(),'Authored response body','Failed save lost the body draft');
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetError=false;globalThis.__workspaceFixture.assetDelay=400;});
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click({clickCount:2});
  await autoEditor.getByRole('button',{name:'Saving…',exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button',{name:'More autoresponse options',exact:true}).isEnabled(),false,'Creation remains available during a save');
  assert.equal(await autoField('body').isEnabled(),false,'Body can change during a save');
  await savedProperties();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.filter(rule=>rule.displayName==='Authored response').length),1,'Double submit created duplicate rules');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.assets.length),assetsBeforeInvalid+1);
  assert.deepEqual(await page.evaluate(()=>{const asset=globalThis.__workspaceFixture.assets.find(asset=>asset.id+'@'+asset.revision===globalThis.__workspaceFixture.automation.rules.find(rule=>rule.displayName==='Authored response').request.responseAsset);return asset.headers.map(header=>new TextDecoder().decode(new Uint8Array(header.name)));}),['X-Fixture']);
  await page.evaluate(()=>{globalThis.__workspaceFixture.assetDelay=0;});
  const authoredRuleId=await page.locator('.auto-response-rule').first().getAttribute('data-rule-id');
  const authoredRule=page.locator('#rule-'+authoredRuleId);
  await authoredRule.click();await page.locator('.rule-selection-toolbar').getByRole('button',{name:'More',exact:true}).click();await page.getByRole('button',{name:'Move later',exact:true}).click();
  await page.waitForFunction(id=>document.querySelector('app-shell').shadowRoot.querySelector('.auto-response-rule').dataset.ruleId!==id,authoredRuleId);
  assert.equal(await authoredRule.locator('.rule-order').textContent(),'2');
  const orderBeforeRemove=await page.locator('.auto-response-rule').evaluateAll(rules=>rules.map(rule=>rule.dataset.ruleId));
  await capturedRule.click();await capturedRule.press('Delete');
  await page.getByRole('button',{name:'Undo',exact:true}).waitFor({state:'visible'});
  assert.equal(await page.getByRole('button',{name:'Undo',exact:true}).evaluate(button=>button.getRootNode().activeElement===button),true,'Removal left keyboard focus on a deleted control');
  await page.getByRole('button',{name:'Undo',exact:true}).click();
  await capturedRule.waitFor({state:'visible'});
  assert.deepEqual(await page.locator('.auto-response-rule').evaluateAll(rules=>rules.map(rule=>rule.dataset.ruleId)),orderBeforeRemove,'Undo changed first-match order');
  assert.equal(await capturedRule.locator('.rule-state').textContent(),'Disabled','Undo changed the rule enabled state');
  await capturedRule.click();
  assert.match(await capturedRule.locator('.rule-criteria').getAttribute('title'),/example.test/);
  assert.equal(await autoField('url').inputValue(),'http://example.test/second');

  await newScratch();
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
  await savedProperties();
  const patternRuleId=await page.locator('.auto-response-rule').first().getAttribute('data-rule-id');
  await page.locator('#rule-'+patternRuleId).click();
  assert.equal(await autoEditor.getByLabel('URL matching',{exact:true}).inputValue(),'pattern');
  assert.equal(await autoField('url').inputValue(),'https://api.example.test/users/{account:digits}');
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.find(rule=>rule.displayName==='Numeric account pattern').matcher.examples.length),1);
  await page.evaluate(id=>{const state=globalThis.__workspaceFixture;state.automation.usage=[{ruleId:id,matches:2,lastMatchedAt:1000}];document.querySelector('app-shell').onTrafficRefreshed();},patternRuleId);
  await autoEditor.getByText(/2 retained matches/).waitFor({state:'visible'});
  await page.evaluate(()=>{globalThis.__workspaceFixture.automation.usage=[];document.querySelector('app-shell').onTrafficRefreshed();});
  await autoEditor.getByText('No matches in retained Traffic.',{exact:true}).waitFor({state:'visible'});
  await cancelRule();

  // The dense list shares keyboard range selection, bulk state changes and Undo.
  const firstRule=page.locator('.auto-response-rule').first();
  await firstRule.click();await savedProperties();
  await firstRule.press('Control+Shift+ArrowDown');
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleSelectionCount===2);
  assert.equal(await page.locator('.auto-response-rule').nth(1).getAttribute('aria-selected'),'true');
  assert.equal(await page.locator('.auto-response-rule').nth(1).locator('input[type="checkbox"]').isChecked(),true);
  const enabledBeforeBulk=await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.map(rule=>({id:rule.id,enabled:rule.enabled})));
  await page.locator('.rule-selection-toolbar').getByRole('button',{name:'More',exact:true}).click();
  await page.locator('#rule-actions-menu').getByRole('button',{name:'Enable selected',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').changingAutoResponse);
  await page.locator('.auto-response-rule').nth(1).press('Control+z');
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').changingAutoResponse);
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.map(rule=>({id:rule.id,enabled:rule.enabled}))),enabledBeforeBulk);
  await page.locator('#rule-'+patternRuleId).click();await savedProperties();
  await page.locator('.rule-selection-toolbar').getByRole('button',{name:'More',exact:true}).click();
  await page.getByRole('button',{name:'Duplicate as disabled copies',exact:true}).click();
  const copyRule=page.locator('.auto-response-rule').filter({has:page.locator('strong').getByText('Copy of Numeric account pattern',{exact:true})});
  await copyRule.waitFor({state:'visible'});
  await copyRule.locator('.rule-warning').getByText('Duplicate',{exact:true}).waitFor({state:'visible'});
  await copyRule.click();await savedProperties();
  await autoField('url').fill('https://api.example.test/users/{:digits}');
  await autoEditor.getByRole('button',{name:'Save rule',exact:true}).click();await savedProperties();
  assert.equal(await copyRule.locator('.rule-warning').textContent(),'Duplicate','Anonymous annotations changed duplicate matching identity');
  await autoField('enabled').check();
  await copyRule.locator('.rule-state').getByText('Enabled',{exact:true}).waitFor({state:'visible'});
  assert.match(await page.locator('.rule-diagnostic').textContent(),/always superseded/);
  await autoField('enabled').uncheck();
  await copyRule.locator('.rule-state').getByText('Disabled',{exact:true}).waitFor({state:'visible'});
  await copyRule.press('Delete');
  await copyRule.waitFor({state:'detached'});
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleSelectionCount),0);
  await page.getByRole('button',{name:'Undo',exact:true}).press('Control+z');
  await copyRule.waitFor({state:'visible'});
  assert.equal(await copyRule.locator('.rule-state').textContent(),'Disabled');

  // Switching selection cannot silently discard property edits; Del in a field edits text.
  await authoredRule.click();await savedProperties();
  await autoField('name').fill('Unsaved name');
  await capturedRule.click();
  await page.locator('automation-workspace .unsaved-rule-dialog').getByRole('button',{name:'Keep editing',exact:true}).click();
  assert.equal(await autoField('name').inputValue(),'Unsaved name');
  await capturedRule.click();await discardIfAsked();await savedProperties();
  await autoField('name').fill('Saved while switching');await authoredRule.click();
  await page.locator('automation-workspace .unsaved-rule-dialog').getByRole('button',{name:'Save and continue',exact:true}).click();await savedProperties();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.find(rule=>rule.displayName==='Saved while switching')!==undefined),true);
  await capturedRule.click();await savedProperties();
  const countBeforeTextDelete=await page.locator('.auto-response-rule').count();
  await autoField('name').fill('Keep');await autoField('name').press('Home');await autoField('name').press('Delete');
  assert.equal(await autoField('name').inputValue(),'eep');
  assert.equal(await page.locator('.auto-response-rule').count(),countBeforeTextDelete);
  await cancelRule();

  await page.getByLabel('Search rules',{exact:true}).fill('Numeric account');
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').visibleRules.length===2);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleSelectionCount),0);
  await page.getByLabel('Search rules',{exact:true}).fill('');
  const ruleDivider=page.getByRole('separator',{name:'Resize rule list and properties',exact:true});
  const ruleSplitBefore=await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleListSplit);
  await ruleDivider.press('ArrowRight');
  await page.waitForFunction(before=>globalThis.__workspaceFixture.workspace.autoresponseSplit>before,ruleSplitBefore);
  await ruleDivider.press('ArrowLeft');

  // Traffic supports multiselection, eligibility review, atomic failure and batch creation.
  await view('traffic');
  await page.evaluate(async()=>{const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');const state=globalThis.__workspaceFixture;state.pagingRows=structuredClone(state.sessions);for(let index=0;index<10;index++)state.sessions.push({...state.sessions[0],id:'paging-'+index,path:'/paging/'+index,url:'http://example.test/paging/'+index,startedAt:-index});traffic.clearTrafficSelection();traffic.pageSize='10';await traffic.refreshSessions(undefined,true);});
  await page.locator('tr[data-session-id]').first().click();
  await page.locator('tr[data-session-id]').first().press('Control+Shift+ArrowDown');
  await page.getByRole('button',{name:'Next',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').pageIndex===1 && document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').sessions[0].id.startsWith('paging-'));
  assert.match(await page.locator('.traffic-selection-bar').textContent(),/2 selected · 2 on other pages/);
  await page.locator('tr[data-session-id]').first().click({modifiers:['Control']});
  await page.evaluate(async()=>await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true));
  assert.match(await page.locator('.traffic-selection-bar').textContent(),/3 selected · 2 on other pages/);
  await page.getByRole('button',{name:'Previous',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').sessions.some(row=>row.id==='new'));
  assert.equal(await page.locator('tr[aria-selected="true"][data-session-id]').count(),2);
  await page.getByLabel('Search traffic',{exact:true}).fill('first');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').totalMatched===1);
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').selectedTrafficCount),0);
  await page.getByRole('button',{name:'Clear search',exact:true}).click();
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;state.sessions=state.pagingRows;delete state.pagingRows;const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');traffic.pageSize='100';await traffic.refreshSessions(undefined,true);});
  await page.evaluate(async()=>{const state=globalThis.__workspaceFixture;state.sessions.find(row=>row.id==='cached').terminal='active';await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').refreshSessions(undefined,true);});
  await page.locator('tr[data-session-id="first"]').click();
  await page.locator('tr[data-session-id="second"]').click({modifiers:['Control']});
  await page.locator('tr[data-session-id="cached"]').click({modifiers:['Control']});
  assert.equal(await page.locator('tr[aria-selected="true"][data-session-id]').count(),3);
  await page.locator('.traffic-selection-bar').getByRole('button',{name:/^Create \d+ auto-responses?…$/}).click();
  await page.waitForFunction(()=>{const auto=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');return !auto.batchReviewHidden && !auto.batchLoading && auto.batchRows.length===3;});
  assert.match(await page.locator('.batch-review').textContent(),/3 selected · 2 ready · 1 unavailable/);
  assert.match(await page.locator('.batch-review').textContent(),/Wait for this request to complete/);
  const includedResponse=page.locator('.batch-table tbody input[type="checkbox"]').nth(1);
  await includedResponse.uncheck();
  await page.getByRole('button',{name:'Create 1 rule',exact:true}).waitFor({state:'visible'});
  await page.getByRole('button',{name:'Refresh review',exact:true}).click();
  await page.waitForFunction(()=>{const auto=document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace');return !auto.batchLoading && auto.batchEligibleCount===1;});
  assert.equal(await includedResponse.isChecked(),false,'Refreshing the batch lost excluded choices');
  await includedResponse.check();
  const rulesBeforeBatch=await page.evaluate(()=>structuredClone(globalThis.__workspaceFixture.automation.rules));
  await page.evaluate(()=>globalThis.__workspaceFixture.batchError=true);
  await page.getByRole('button',{name:'Create 2 rules',exact:true}).click();
  await page.locator('.batch-review').getByText('Fixture batch storage failed',{exact:true}).waitFor({state:'visible'});
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules),rulesBeforeBatch);
  await page.evaluate(()=>globalThis.__workspaceFixture.batchError=false);
  await page.getByRole('button',{name:'Create 2 rules',exact:true}).click();
  await page.locator('.batch-review').waitFor({state:'hidden'});
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('automation-workspace').ruleSelectionCount),2);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.automation.rules.length),rulesBeforeBatch.length+2);
  await capturedRule.locator('.rule-warning').getByText('Shadowed',{exact:true}).waitFor({state:'visible'});

  await view('traffic');
  await page.locator('tr[data-session-id="first"]').click();
  await page.locator('tr[data-session-id="second"]').click({modifiers:['Control']});
  await page.locator('tr[data-session-id="second"]').press('Delete');
  await page.locator('tr[data-session-id="first"]').waitFor({state:'detached'});
  await page.locator('tr[data-session-id="second"]').waitFor({state:'detached'});
  await page.getByRole('button',{name:'Undo traffic removal',exact:true}).press('Control+z');
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').removingTraffic);
  await page.locator('tr[data-session-id="first"]').waitFor({state:'visible'});
  await page.locator('tr[data-session-id="second"]').waitFor({state:'visible'});
  assert.equal(await page.locator('tr[aria-selected="true"][data-session-id]').count(),2);
  await view('automation');

  const rowBaselines=await page.locator('.auto-response-rule').first().evaluate(row=>{const bottom=selector=>{const element=row.querySelector(selector);const range=document.createRange();range.selectNodeContents(element);return range.getBoundingClientRect().bottom;};return [bottom('.rule-description strong'),bottom('.rule-state')];});
  assert.ok(Math.abs(rowBaselines[0]-rowBaselines[1])<=2,'Rule name and state text baselines do not align');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-dense.png')});
  await newScratch();
  await autoField('body').fill(Array.from({length:30},(_,index)=>'line '+(index+1)).join('\n'));
  await page.setViewportSize({width:800,height:600});
  await autoEditor.evaluate(form=>form.scrollIntoView({block:'start'}));
  const savePosition=await autoEditor.getByRole('button',{name:'Save rule',exact:true}).boundingBox();
  assert.ok(savePosition.y>=50 && savePosition.y+savePosition.height<592,'Sticky save actions are outside the small window');
  assert.ok((await autoField('body').boundingBox()).height>=240,'Twelve-line body editor has insufficient height');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-small.png')});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/auto-response-dark.png')});
  await page.emulateMedia({colorScheme:'light'});
  await page.setViewportSize({width:1280,height:800});
  await cancelRule();
  await newScratch();
  await page.locator('.auto-response-editor input[name="name"]').fill('Preserved draft');
  await page.locator('.auto-response-editor select[name="method"]').selectOption('POST');
  assert.equal(await page.locator('.auto-response-editor textarea[name="requestHeaders"]').isVisible(), true);
  await view('composer');
  await page.locator('input[name="url"]').filter({ visible: true }).fill('http://example.test/replay');
  await view('automation');
  assert.equal(await page.locator('.auto-response-editor input[name="name"]').inputValue(), 'Preserved draft');
  await page.locator('.workspace-tabs').getByRole('button',{name:'Scripts',exact:true}).click();
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').sourceEditor !== null);
  const sourceSurface=page.locator('script-editor editor-surface').first();
  assert.ok((await page.locator('script-editor .monaco-editor-host').first().boundingBox()).height>=320,'Source editor is too short');
  await sourceSurface.getByRole('button',{name:'Expand editor',exact:true}).click();
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),true);
  assert.ok((await page.locator('script-editor .monaco-editor-host').first().boundingBox()).height>=580,'Expanded source editor is too short');
  assert.equal(await sourceSurface.getByRole('button',{name:'Save draft',exact:true}).isVisible(),true);
  await page.keyboard.press('Escape');
  assert.equal(await sourceSurface.locator('dialog').evaluate(dialog=>dialog.matches(':modal')),false);
  const scripts=page.locator('script-editor');
  await scripts.getByRole('button',{name:'Validate',exact:true}).click();
  await scripts.locator('.script-state').getByText('Validated · Unsaved',{exact:true}).waitFor({state:'visible'});
  await scripts.locator('.script-test summary').click();
  await scripts.getByLabel('Test URL',{exact:true}).fill('https://sandbox.example.test:8443/users/42?ready=yes');
  await scripts.getByLabel('Method',{exact:true}).fill('PATCH');
  await scripts.getByLabel('Headers',{exact:true}).fill('X-Test: yes');
  await scripts.getByRole('button',{name:'Test in sandbox',exact:true}).click();
  await scripts.locator('.script-state').getByText('Tested · Unsaved',{exact:true}).waitFor({state:'visible'});
  assert.deepEqual(await page.evaluate(()=>{const input=globalThis.__workspaceFixture.scriptTestInput.request;return {method:input.method,host:input.host,port:input.port,path:input.path,query:input.query};}),{method:'PATCH',host:'sandbox.example.test',port:8443,path:'/users/42',query:'ready=yes'});
  await scripts.getByRole('button',{name:'Enable',exact:true}).click();
  await scripts.locator('.script-state').getByText('Active · Unsaved',{exact:true}).waitFor({state:'visible'});
  await scripts.getByRole('button',{name:'More',exact:true}).click();
  await scripts.getByRole('button',{name:'Pause script',exact:true}).click();
  await page.keyboard.press('Escape');
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferScriptValidation=true;globalThis.__scriptValidation=document.querySelector('app-shell').shadowRoot.querySelector('script-editor').validateScript();});
  await page.waitForFunction(()=>globalThis.__workspaceFixture.scriptValidationPending);
  await page.evaluate(()=>{const scripts=document.querySelector('app-shell').shadowRoot.querySelector('script-editor');scripts.sourceEditor.setValue(scripts.sourceEditor.getValue()+'\n// changed while validating');globalThis.__workspaceFixture.finishScriptValidation();globalThis.__workspaceFixture.deferScriptValidation=false;});
  await page.evaluate(()=>globalThis.__scriptValidation);
  assert.equal(await scripts.getByRole('button',{name:'Enable',exact:true}).isDisabled(),true,'An older validation enabled changed source');
  await scripts.getByRole('button',{name:'Save draft',exact:true}).click();
  await scripts.locator('.script-state').getByText('Saved',{exact:true}).waitFor({state:'visible'});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/scripts-wide.png')});
  await page.setViewportSize({width:760,height:520});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/scripts-small.png')});
  await page.setViewportSize({width:1280,height:800});
  const editorsBefore = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.paused = Array.from({ length: 20 }, (_, index) => ({ decisionId: index + 1, exchangeId: 'paused-' + index, phase: index%2?'response-head':'request-head', requestHead: { method: 'GET',target:'https://example.test/paused/'+index }, responseHead: index%2?{status:200,headers:[]}:null, bodyHex: null, hookId: 'fixture', expiresAtUnixMs:Date.now()+60000 }));
  });
  await view('breakpoints');
  await page.locator('paused-exchange').first().getByRole('button', { name: 'Continue', exact: true }).waitFor({ state: 'visible' });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange')?.editorReady);
  await page.setViewportSize({width:760,height:520});
  await page.locator('tr[data-decision-id="1"]').click();
  await page.screenshot({path:resolve(root,'../../../target/ui-check/breakpoints-small.png')});
  const pausedBounds=await page.locator('paused-exchange').first().boundingBox();
  const expandBounds=await page.locator('paused-exchange').first().getByRole('button',{name:'Expand editor',exact:true}).boundingBox();
  assert.ok(pausedBounds.x+pausedBounds.width<=760,'Paused editor exceeds the narrow viewport');
  assert.ok(expandBounds.x+expandBounds.width<=760,'Expand editor control is clipped');
  await page.setViewportSize({width:1280,height:800});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/breakpoints-wide.png')});
  assert.equal(await page.locator('paused-exchange').count(),1,'The queue allocated an editor for every request');
  await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange').editor.setValue('{\"method\":\"PATCH\"}'));
  await page.locator('tr[data-decision-id="2"]').click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange').draftText.includes('status'));
  assert.equal(await page.locator('.nav-count').textContent(),'20');
  await page.locator('tr[data-decision-id="1"]').click();
  assert.match(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange').editor.getValue()),/PATCH/,'Queue navigation lost a replacement draft');
  const editorsVisible = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  assert.ok(editorsVisible > editorsBefore && editorsVisible <= editorsBefore + 2, 'Offscreen breakpoint editors were eagerly created');
  await page.evaluate(async () => {
    globalThis.__workspaceFixture.paused = [];
    await document.querySelector('app-shell').shadowRoot.querySelector('breakpoint-workspace').refreshBreakpoints();
  });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length === 1);
  await view('captures');
  const captures=page.locator('capture-workspace');
  const captureIdle=()=>page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('capture-workspace').captureBusy);
  await captures.getByLabel('New capture file',{exact:true}).fill('kept.tmcap');
  await captures.getByRole('button',{name:'Choose…',exact:true}).filter({visible:true}).click();await captureIdle();
  assert.equal(await captures.getByLabel('New capture file',{exact:true}).inputValue(),'kept.tmcap','Canceling the picker changed a path');
  await captures.getByRole('button',{name:'Start recording',exact:true}).click();
  await captures.locator('.capture-state').getByText('Recording · 2 KiB written',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastCaptureRequest.maxFileBytes),1073741824);
  assert.equal(await captures.getByRole('button',{name:'Start recording',exact:true}).isDisabled(),true);
  await captures.getByRole('button',{name:'Stop recording',exact:true}).click();await captureIdle();
  await captures.locator('.capture-tabs').getByRole('button',{name:'Inspect / recover',exact:true}).click();
  await captures.getByRole('button',{name:'Inspect file',exact:true}).click();await captureIdle();
  assert.match(await captures.locator('.capture-receipt').textContent(),/Interrupted tail; valid prefix recovered/);
  await captures.getByRole('button',{name:'Export a recovered copy…',exact:true}).click();
  assert.equal(await captures.getByLabel('New export file',{exact:true}).inputValue(),'session.recovered.tmcap');
  await captures.getByLabel('Format',{exact:true}).selectOption('json-lines');
  assert.equal(await captures.getByLabel('New export file',{exact:true}).inputValue(),'session.recovered.jsonl');
  await captures.getByRole('button',{name:'Export file',exact:true}).click();await captureIdle();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastCaptureExport.format),'json-lines');
  assert.match(await captures.locator('.capture-receipt').textContent(),/Valid prefix preserved/);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/capture-wide.png')});
  await page.setViewportSize({width:760,height:520});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/capture-small.png')});
  await page.setViewportSize({width:1280,height:800});
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
  await page.setViewportSize({width:1280,height:800});
  await view('settings');
  const setupCertificate=page.locator('#settings').getByRole('button',{name:'Check certificate setup',exact:true});
  const resetCertificate=page.locator('#settings').getByRole('button',{name:'Reset certificate and set up again',exact:true});
  const certificateNotice=page.locator('.notice');
  const caIdle=()=>page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('settings-workspace').proxyPending);
  await page.locator('.settings-tabs').getByRole('button',{name:'Preferences',exact:true}).click();
  const preferences=page.locator('settings-workspace');
  await preferences.getByLabel('Theme',{exact:true}).selectOption('light');
  assert.equal(await preferences.getByRole('button',{name:'Save settings',exact:true}).isEnabled(),true);
  await preferences.getByRole('button',{name:'Revert changes',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('settings-workspace').settingsBusy);
  assert.equal(await preferences.getByLabel('Theme',{exact:true}).inputValue(),'system');
  await preferences.getByLabel('Redact Authorization, Proxy-Authorization, Cookie and Set-Cookie values',{exact:true}).check();
  await preferences.getByLabel('Entries per page',{exact:true}).fill('75');
  await preferences.getByRole('button',{name:'Save settings',exact:true}).click();
  await preferences.getByText('Settings saved.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.savedProduct.privacy.redactSensitiveHeaders),true);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.savedProduct.privacy.retainRequestBodies),true);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.savedProduct.preferences.sessionPageSize),75,'Disabled form controls were omitted from the saved preferences');
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').pageSize),'75','Saving failed to apply the page size to Traffic');
  assert.equal(await preferences.getByRole('button',{name:'Save settings',exact:true}).isDisabled(),true);
  await page.setViewportSize({width:1280,height:800});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/settings-wide.png')});
  await page.setViewportSize({width:760,height:520});
  const preferenceSave=await preferences.getByRole('button',{name:'Save settings',exact:true}).boundingBox();
  assert.ok(preferenceSave.y+preferenceSave.height<=512,'Preference save actions leave the narrow window');
  await page.screenshot({path:resolve(root,'../../../target/ui-check/settings-small.png')});
  await page.setViewportSize({width:1280,height:800});
  await page.locator('.settings-tabs').getByRole('button',{name:'Support',exact:true}).click();
  assert.equal(await preferences.getByLabel('Include recent artifact paths',{exact:true}).isDisabled(),true);
  await preferences.getByRole('button',{name:'Refresh diagnostics',exact:true}).click();
  await preferences.getByText('Diagnostics refreshed.',{exact:true}).waitFor({state:'visible'});
  assert.match(await preferences.locator('.support-facts').textContent(),/Windows fixture/);
  await preferences.getByRole('button',{name:'Create support bundle',exact:true}).click();
  await preferences.getByText('Support bundle saved.',{exact:true}).waitFor({state:'visible'});
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.supportInput.includeRecentPaths),false);
  await page.locator('.settings-tabs').getByRole('button',{name:'Connection',exact:true}).click();
  await preferences.locator('.connection-advanced summary').click();
  // Legacy, partial, and missing material all offer recovery without attempting
  // to overwrite files or trust a certificate with an unknown identity.
  for(const bootstrap of [
    {caFilesPresent:true,caFilesExist:true,ownedCaSha256:null,ownedCaTrusted:false},
    {caFilesPresent:false,caFilesExist:true,ownedCaSha256:null,ownedCaTrusted:false},
    {caFilesPresent:false,caFilesExist:false,ownedCaSha256:'0'.repeat(64),ownedCaTrusted:true},
  ]) {
    await page.evaluate(bootstrap=>{const state=globalThis.__workspaceFixture;state.caBootstrap=bootstrap;state.caEvents=[];},bootstrap);
    await setupCertificate.click();await caIdle();
    assert.match(await certificateNotice.textContent(),/Interception certificate needs a reset/);
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caEvents),[]);
    page.once('dialog',dialog=>dialog.dismiss());
    await certificateNotice.getByRole('button',{name:'Reset certificate and set up again',exact:true}).click();await caIdle();
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caEvents),[],'Canceling reset changed certificate state');
    // Reset always targets the backend-owned paths even when the form contains
    // a user-selected CA, and uses the default paths for the fresh certificate.
    await page.getByLabel('CA certificate',{exact:true}).fill('custom.pem');
    await page.getByLabel('CA private key',{exact:true}).fill('custom.key');
    await page.evaluate(()=>{globalThis.__workspaceFixture.deferCaReset=true;});
    page.once('dialog',async dialog=>{assert.match(dialog.message(),/fixture.pem\nfixture.key/);await dialog.accept();});
    await resetCertificate.click();
    await page.waitForFunction(()=>globalThis.__workspaceFixture.caResetPending);
    assert.equal(await setupCertificate.isDisabled(),true);
    assert.equal(await resetCertificate.isDisabled(),true);
    await page.evaluate(async()=>{const settings=document.querySelector('app-shell').shadowRoot.querySelector('settings-workspace');await settings.resetCa();globalThis.__workspaceFixture.releaseCaReset();delete globalThis.__workspaceFixture.caResetPending;});
    await caIdle();
    assert.match(await certificateNotice.textContent(),/HTTPS interception is ready/);
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caEvents),['reset','create','install']);
    assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.resetCaArgs),{},'Reset accepted arbitrary file paths');
    assert.deepEqual(await page.evaluate(()=>[globalThis.__workspaceFixture.lastCaCreate.certificatePath,globalThis.__workspaceFixture.lastCaCreate.privateKeyPath,globalThis.__workspaceFixture.lastCaInstall.path]),['fixture.pem','fixture.key','fixture.pem']);
    assert.equal(await page.getByLabel('CA SHA-256',{exact:true}).inputValue(),'1'.repeat(64));
  }
  // A cleanup failure must not advance to certificate creation or trust.
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.caResetError=true;state.caEvents=[];});
  page.once('dialog',dialog=>dialog.accept());await resetCertificate.click();await caIdle();
  assert.match(await certificateNotice.textContent(),/Certificate reset failed: Fixture CA file deletion failed/);
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caEvents),['reset']);
  await page.evaluate(()=>{globalThis.__workspaceFixture.caResetError=false;globalThis.__workspaceFixture.caTrustError=true;globalThis.__workspaceFixture.caEvents=[];});
  page.once('dialog',dialog=>dialog.accept());await resetCertificate.click();await caIdle();
  assert.match(await certificateNotice.textContent(),/Certificate setup failed: Fixture Windows trust canceled/);
  await page.evaluate(()=>{globalThis.__workspaceFixture.caTrustError=false;globalThis.__workspaceFixture.caEvents=[];});
  page.once('dialog',dialog=>dialog.accept());
  await certificateNotice.getByRole('button',{name:'Try again',exact:true}).click();await caIdle();
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.caEvents),['install'],'Retrying trust recreated the certificate');
  await certificateNotice.getByRole('button',{name:'Start proxy',exact:true}).click();await caIdle();
  assert.equal(await resetCertificate.isDisabled(),true,'Reset remained available while the proxy was running');
  await page.locator('#settings').getByRole('button',{name:'Stop proxy',exact:true}).click();await caIdle();
  assert.equal(await resetCertificate.isDisabled(),false);
  assert.deepEqual(await page.evaluate(()=>globalThis.__cspViolations),[]);
  assert.deepEqual(errors,[]);

  await view('traffic');

  await page.evaluate(async()=>{const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');await traffic.clearFilters();const state=globalThis.__workspaceFixture;state.sessions.find(row=>row.id==='first').searchBody='Café marker';state.sessions.find(row=>row.id==='second').searchBody='CAFE other';});
  const searchOptions=page.locator('#traffic-search-options');
  await page.getByRole('button',{name:'Search options',exact:true}).click();
  await searchOptions.getByLabel('URL, method, process, PID and status',{exact:true}).uncheck();
  await searchOptions.getByLabel('Request and response headers',{exact:true}).uncheck();
  await searchOptions.getByLabel('Ignore accents (text mode)',{exact:true}).check();
  await searchOptions.getByLabel('Select all matches after searching',{exact:true}).check();
  await page.getByLabel('Search traffic',{exact:true}).fill('cafe');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.getByRole('button',{name:'Select all 2 matches',exact:true}).waitFor();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').selectedTrafficCount===2);
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.lastContentSearch.ignoreDiacritics),true);
  assert.doesNotMatch(await page.locator('.content-search-status').textContent(),/binary|image/i);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/content-search-wide.png')});
  await page.getByLabel('Search traffic',{exact:true}).fill('marker');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.waitForFunction(()=>{const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');return !traffic.searchingTraffic&&traffic.selectedTrafficCount===1&&traffic.trafficSelection.ids.has('first');});
  await page.locator('.traffic-selection-bar').getByRole('button',{name:'More',exact:true}).click();
  await page.locator('#traffic-entry-menu').getByRole('button',{name:'Remove unselected entries',exact:true}).click();
  await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').removingTraffic);
  await page.getByRole('button',{name:'Clear search',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').totalMatched===1);
  await page.locator('.traffic-undo').getByRole('button',{name:'Undo traffic removal',exact:true}).click();
  await page.waitForFunction(()=>{const traffic=document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace');return !traffic.removingTraffic&&traffic.totalMatched>=4;});
  await page.getByRole('button',{name:'Search options',exact:true}).click();
  await searchOptions.getByLabel('Match mode',{exact:true}).selectOption('regex');
  assert.equal(await searchOptions.getByLabel('Ignore accents (text mode)',{exact:true}).isDisabled(),true);
  await page.getByLabel('Search traffic',{exact:true}).fill('caf[ée]');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.getByRole('button',{name:'Select all 2 matches',exact:true}).waitFor();
  await page.getByLabel('Search traffic',{exact:true}).fill('(');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.locator('.content-search-status').getByText(/Search failed:/).waitFor();
  assert.equal(await page.evaluate(()=>document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').selectedTrafficCount),0);
  await page.evaluate(()=>globalThis.__workspaceFixture.deferSearch=true);
  await page.getByLabel('Search traffic',{exact:true}).fill('body');
  await page.locator('#traffic .filters').getByRole('button',{name:'Search',exact:true}).click();
  await page.getByRole('button',{name:'Cancel search',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.releaseSearch();globalThis.__workspaceFixture.deferSearch=false;});
  await page.locator('.content-search-status').getByText('Search canceled. Previous results are unchanged.',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Clear search',exact:true}).click();
  await page.getByRole('button',{name:'Search options',exact:true}).click();
  await searchOptions.getByLabel('Match mode',{exact:true}).selectOption('text');
  await searchOptions.getByLabel('URL, method, process, PID and status',{exact:true}).check();
  await searchOptions.getByLabel('Request and response headers',{exact:true}).check();
  await searchOptions.getByLabel('Select all matches after searching',{exact:true}).uncheck();
  await page.getByRole('button',{name:'Search options',exact:true}).click();
  await page.evaluate(()=>globalThis.__workspaceFixture.pickedTrace='C:/captures/support.saz');
  await page.locator('#traffic').getByRole('button',{name:'Import…',exact:true}).click();
  await page.locator('.trace-import-status').getByText(/Imported 1 entry from support.saz/).waitFor();
  await page.getByRole('button',{name:'Files',exact:true}).click();
  await page.locator('#trace-files-menu').getByRole('button',{name:'Trace metadata…',exact:true}).click();
  await page.locator('.trace-metadata-dialog[open] h3').getByText('support.saz',{exact:true}).waitFor();
  assert.match(await page.locator('.trace-context').textContent(),/Captured machine IP configuration/);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/trace-metadata-wide.png')});
  await page.locator('.trace-metadata-dialog').getByRole('button',{name:'Close',exact:true}).click();
  await page.evaluate(async()=>{globalThis.__workspaceFixture.openedTraces=['C:/captures/second.saz'];await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').takeOpenedTraces();});
  await page.locator('.trace-open-dialog[open]').waitFor();
  await page.locator('.trace-open-dialog').getByRole('button',{name:'Open separate viewer',exact:true}).click();
  assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.openedViewer),['C:/captures/second.saz']);
  await page.evaluate(()=>{globalThis.__workspaceFixture.deferImport=true;});
  await page.locator('#traffic').getByRole('button',{name:'Import…',exact:true}).click();
  await page.getByRole('button',{name:'Cancel import',exact:true}).click();
  await page.evaluate(()=>{globalThis.__workspaceFixture.releaseImport();globalThis.__workspaceFixture.deferImport=false;});
  await page.locator('.trace-import-status').getByText('Import canceled. Traffic is unchanged.',{exact:true}).waitFor();
  await page.goto('https://workspace.test/?viewer');
  await page.getByText('Capture viewer',{exact:true}).waitFor();
  assert.equal(await page.getByRole('button',{name:'Start proxy',exact:true}).count(),0);
  assert.equal(await page.locator('settings-workspace').count(),0);
  assert.equal(await page.getByRole('button',{name:'Save trace…',exact:true}).count(),1);
  await page.locator('#traffic').getByRole('button',{name:'Import…',exact:true}).waitFor();
  await page.evaluate(async()=>{globalThis.__workspaceFixture.openedTraces=['C:/captures/viewer.saz'];await document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').takeOpenedTraces();});
  await page.locator('.trace-import-status').getByText(/Imported 1 entry from viewer.saz/).waitFor();
  assert.equal(await page.locator('.trace-open-dialog[open]').count(),0);
  await page.getByRole('button',{name:'Open main window',exact:true}).click();
  assert.equal(await page.evaluate(()=>globalThis.__workspaceFixture.calls.open_main_window),1);
  await page.setViewportSize({width:800,height:600});
  await page.screenshot({path:resolve(root,'../../../target/ui-check/capture-viewer-compact.png')});
  await page.getByRole('button',{name:'Save trace…',exact:true}).click();
  const saveTrace=page.locator('.trace-save-dialog[open]');
  assert.equal(await saveTrace.getByLabel('Compress for sharing (.tmcap.gz)',{exact:true}).isChecked(),true);
  assert.equal(await saveTrace.getByLabel('Include this computer’s network configuration',{exact:true}).isChecked(),false);
  await saveTrace.getByLabel('Include this computer’s network configuration',{exact:true}).check();
  await page.evaluate(()=>globalThis.__workspaceFixture.saveTraceError=true);
  await saveTrace.getByRole('button',{name:'Save as…',exact:true}).click();
  await saveTrace.getByText(/Trace could not be saved:/).waitFor();assert.equal(await saveTrace.getByLabel('Include this computer’s network configuration',{exact:true}).isChecked(),true);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/save-trace-compact.png')});
  await page.evaluate(()=>{globalThis.__workspaceFixture.saveTraceError=false;globalThis.__workspaceFixture.cancelTraceSave=true;});
  await saveTrace.getByRole('button',{name:'Save as…',exact:true}).click();await saveTrace.getByText('Save canceled.',{exact:true}).waitFor();
  await page.evaluate(()=>globalThis.__workspaceFixture.cancelTraceSave=false);
  await saveTrace.getByRole('button',{name:'Save as…',exact:true}).click();await page.waitForFunction(()=>!document.querySelector('app-shell').shadowRoot.querySelector('traffic-workspace').savingTrace);
  assert.equal(await page.locator('.trace-save-dialog[open]').count(),0);assert.deepEqual(await page.evaluate(()=>globalThis.__workspaceFixture.savedTraceArgs),{options:{includeNetworkContext:true},compressed:true});
  await page.evaluate(()=>{const state=globalThis.__workspaceFixture;state.performance={points:[{milestone:'client-connected',unixMillis:1800000000000,offsetMicros:-2500},{milestone:'request-headers',unixMillis:1800000000002,offsetMicros:0},{milestone:'exchange-done',unixMillis:1800000000005,offsetMicros:3500}],protocols:[],transports:[{leg:'upstream',connectionId:'shared-h2-fixture',shared:true,outcome:'connected',sampledOffsetMicros:3000,peer:'192.0.2.1:443',local:'192.0.2.2:54321',dnsMicros:null,tcpMicros:1200,tlsMicros:500,tlsVersion:'TLSv1.3',tlsResumed:false,cipher:'TLS_AES_128_GCM_SHA256',alpn:'h2',bytesRead:10240,bytesWritten:1024}]};state.savedTiming={ClientBeginRequest:'00:00:00.000'};});
  await page.locator('.session-link').first().click();
  await page.locator('.selection-actions').getByRole('button',{name:'Timings',exact:true}).click();
  const timing=page.locator('.timing-dialog[open]');
  await timing.getByText('3.5 ms',{exact:true}).waitFor();
  assert.match(await timing.textContent(),/Negative offsets/);
  await timing.getByText('Proxy ↔ upstream',{exact:true}).click();
  assert.ok((await timing.textContent()).includes('Reused / shared connection'));
  assert.match(await timing.textContent(),/TLSv1.3/);
  await timing.getByText('Original imported timing and session evidence',{exact:true}).click();
  await timing.getByText('00:00:00.000',{exact:true}).waitFor();
  await page.setViewportSize({width:1280,height:800});await page.screenshot({path:resolve(root,'../../../target/ui-check/timings-wide.png')});
  await page.setViewportSize({width:800,height:600});
  const timingBounds=await timing.boundingBox();assert.ok(timingBounds.width<=800&&timingBounds.height<=600);
  await page.screenshot({path:resolve(root,'../../../target/ui-check/timings-compact.png')});
  await timing.getByRole('button',{name:'Close',exact:true}).click();
  assert.equal(await page.locator('.selection-actions').getByRole('button',{name:'Timings',exact:true}).evaluate(node=>node===node.getRootNode().activeElement),true);
  await page.evaluate(()=>{globalThis.__workspaceFixture.performance=undefined;});
  await page.locator('.selection-actions').getByRole('button',{name:'Timings',exact:true}).click();
  await timing.getByText(/no measured proxy timeline/).waitFor();await page.keyboard.press('Escape');assert.equal(await page.locator('.timing-dialog[open]').count(),0);
  assert.deepEqual(errors,[]);
  process.stdout.write(JSON.stringify({ startupRequests, coalescedQueries: coalesced, editorsBefore, editorsVisible, components: built.stats.componentCount, cspViolations: 0 }) + '\n');
} catch (error) {
  await page.screenshot({path:resolve(root,'../../../target/ui-check/workspace-failure.png')});
  process.stderr.write(JSON.stringify({ errors, requests, state: await page.evaluate(() => ({ calls: globalThis.__workspaceFixture?.calls, output: document.querySelector('app-shell')?.shadowRoot?.querySelector('.session-status')?.textContent, diagnostics: document.querySelector('app-shell')?.shadowRoot?.querySelector('.global-diagnostics')?.textContent, definitions: ['app-shell', 'traffic-workspace', 'settings-workspace'].map((tag) => [tag, Boolean(customElements.get(tag))]) })) }, null, 2) + '\n');
  throw error;
} finally { await browser.close(); }
