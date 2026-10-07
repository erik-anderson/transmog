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
    const asset = resources.get(path);
    if (!asset) { await route.fulfill({ status: 404, body: 'Not found' }); return; }
    await route.fulfill({ contentType: asset.contentType, body: asset.body ?? await readFile(asset.file) });
  }
});
await page.addInitScript(() => {
  const caller = { kind: 'local-process', processName: 'Fixture', processId: 42 };
  const summary = (id) => ({ id, caller, method: 'GET', host: 'example.test', path: '/' + id, protocol: 'HTTP/1.1', status: 200, durationMs: 1, requestBytes: 0, responseBytes: 4, terminal: 'completed', loss: false, capturing: false, autoResponse: null });
  const detail = (id) => ({ id, caller, requests: [{ boundary: 'client-request', method: 'GET', target: 'http://example.test/' + id, status: null, protocol: 'HTTP/1.1', headers: [] }], responses: [{ boundary: 'client-response', method: null, target: null, status: 200, protocol: 'HTTP/1.1', headers: [] }], bodies: [], storedBodies: [{ exchangeId: id, boundary: 'client-response', observedBytes: 4, retainedBytes: 4, availability: 'complete', mediaType: 'text/plain', charset: 'utf-8', contentCodings: [], sha256: null, reason: null }], diagnostics: [], hookEffects: [], routeSelection: null, routeAttempts: [], terminal: 'completed', websocket: null, sequenceLoss: 0, autoResponse: null });
  const state = globalThis.__workspaceFixture = { calls: {}, sessions: [summary('first'), summary('second')], paused: [], queryDelay: 0, slowDetail: false };
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
        case 'product_state': return { schemaVersion: 1, preferences: { theme: 'system', sessionPageSize: 100, configureSystemProxy: true }, privacy: { retainResponseBodies: true, retainBodySamples: false, rememberRecentArtifacts: false, includePathsInSupportBundles: false }, window: {}, recentArtifacts: [] };
        case 'app_status': return { lifecycle: 'stopped', listener: null, summary: 'Proxy stopped', hostRestorePending: false };
        case 'watch_sessions': state.channel = args.onEvent; return;
        case 'query_sessions': await new Promise((resolve) => setTimeout(resolve, state.queryDelay)); return { sessions: structuredClone(state.sessions), nextCursor: null, evicted: 0, sequenceGaps: 0, subscriberLag: 0 };
        case 'session_detail': if (state.slowDetail && args.id === 'first') await new Promise((resolve) => setTimeout(resolve, 100)); return detail(args.id);
        case 'inspect_body': return { metadata: detail(args.request.sessionId).storedBodies[0], representation: 'original-text', decoded: true, textEncoding: 'utf-8', display: 'body', displayBytes: 4, truncated: false, nextOffset: null, warning: null, previewHandle: null, previewMimeType: null };
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
});

const view = async (name) => {
  await page.locator('app-shell a[data-view="' + name + '"]').click();
  await page.locator('#' + name).waitFor({ state: 'visible' });
};
try {
  await page.goto('http://workspace.test/');
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.session-status').textContent.startsWith('Watching live traffic.'));
  assert.equal(await page.locator('#traffic').isVisible(), true);
  assert.equal(await page.locator('a[data-view="traffic"]').getAttribute('aria-current'), 'page');
  const editorOutputs = Object.entries(metadata.outputs).filter(([, output]) => Object.keys(output.inputs).some((input) => input.includes('node_modules/monaco-editor'))).map(([file]) => '/' + file.replace(/^dist\//, ''));
  assert.equal(requests.some((path) => path === '/monaco.js' || path === '/monaco.css' || editorOutputs.includes(path)), false, 'Monaco loaded at startup');
  assert.equal(await page.evaluate(() => customElements.get('automation-workspace') !== undefined), false, 'Automation hydrated at startup');
  const startupRequests = requests.length;

  await page.evaluate(() => { globalThis.__row = document.querySelector('app-shell').shadowRoot.querySelector('tr[data-session-id="first"]'); });
  await page.locator('form.filters button').click();
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('.session-status').textContent.startsWith('Loaded '));
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
  const editorsBefore = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  await page.evaluate(() => {
    globalThis.__workspaceFixture.paused = Array.from({ length: 20 }, (_, index) => ({ decisionId: index + 1, exchangeId: 'paused-' + index, phase: 'request-head', requestHead: { method: 'GET' }, responseHead: null, bodyHex: null, hookId: 'fixture' }));
  });
  await view('breakpoints');
  await page.locator('paused-exchange').first().getByRole('button', { name: 'Continue', exact: true }).waitFor({ state: 'visible' });
  await page.waitForFunction(() => document.querySelector('app-shell').shadowRoot.querySelector('paused-exchange')?.editorReady);
  const editorsVisible = await page.evaluate(() => document.querySelector('app-shell').shadowRoot.querySelector('script-editor').monaco.editor.getModels().length);
  assert.ok(editorsVisible > editorsBefore && editorsVisible < editorsBefore + 20, 'Offscreen breakpoint editors were eagerly created');
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
  process.stdout.write(JSON.stringify({ startupRequests, coalescedQueries: coalesced, editorsBefore, editorsVisible, components: built.stats.componentCount, cspViolations: 0 }) + '\n');
} catch (error) {
  process.stderr.write(JSON.stringify({ errors, requests, state: await page.evaluate(() => ({ calls: globalThis.__workspaceFixture?.calls, output: document.querySelector('app-shell')?.shadowRoot?.querySelector('.session-status')?.textContent, diagnostics: document.querySelector('app-shell')?.shadowRoot?.querySelector('.global-diagnostics')?.textContent, definitions: ['app-shell', 'traffic-workspace', 'settings-workspace'].map((tag) => [tag, Boolean(customElements.get(tag))]) })) }, null, 2) + '\n');
  throw error;
} finally { await browser.close(); }
