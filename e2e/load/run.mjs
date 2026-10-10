import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, readFile, writeFile, stat, rm, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { promisify } from 'node:util';
import os from 'node:os';
import { chromium } from '../playwright/node_modules/playwright/index.mjs';
import { closeServer, listen, RESPONSE_SIZES, requestSize, startSite } from './site.mjs';
import { createServer } from 'node:net';

const command = promisify(execFile);
const directory = dirname(fileURLToPath(import.meta.url));
const repo = resolve(directory, '../..');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const GiB = 1024 ** 3;

export function options(args) {
  const result = { target: 'both', smoke: false, headed: false, navigations: 100,
    requestsPerNavigation: 100, concurrency: 8, storage: 'disk', timeoutMinutes: 30,
    output: join(repo, 'artifacts/load'), python: 'python',
    cli: join(repo, 'target/release', process.platform === 'win32' ? 'transmog-cli.exe' : 'transmog-cli'),
    desktop: join(repo, 'target/release/transmog.exe') };
  const values = new Map([['--target', 'target'], ['--navigations', 'navigations'],
    ['--requests-per-navigation', 'requestsPerNavigation'], ['--concurrency', 'concurrency'],
    ['--storage', 'storage'], ['--timeout-minutes', 'timeoutMinutes'], ['--output', 'output'],
    ['--cli', 'cli'], ['--desktop', 'desktop'], ['--python', 'python']]);
  for (let index = 0; index < args.length; index++) {
    const flag = args[index];
    if (flag === '--smoke') result.smoke = true;
    else if (flag === '--headed') result.headed = true;
    else if (values.has(flag)) {
      const value = args[++index];
      assert.ok(value && !value.startsWith('--'), `Missing value for ${flag}`);
      const key = values.get(flag);
      result[key] = typeof result[key] === 'number' ? Number(value) : value;
    } else throw new Error(`Unknown option: ${flag}`);
  }
  if (result.smoke) {
    if (!args.includes('--navigations')) result.navigations = 3;
    if (!args.includes('--requests-per-navigation')) result.requestsPerNavigation = 12;
  }
  for (const [key, max] of [['navigations', 10000], ['requestsPerNavigation', 10000],
    ['concurrency', 32], ['timeoutMinutes', 240]]) {
    assert.ok(Number.isSafeInteger(result[key]) && result[key] > 0 && result[key] <= max, `Invalid ${key}`);
  }
  assert.ok(['both', 'cli', 'desktop'].includes(result.target), 'Invalid --target');
  assert.ok(['disk', 'memory'].includes(result.storage), 'Invalid --storage');
  const expected = workloadSize(result);
  if (!result.smoke) {
    assert.ok(expected.exchanges >= 10000, 'Full qualification requires at least 10,000 exchanges; use --smoke for a smaller check');
    assert.ok(expected.requestBytes + expected.responseBytes >= 2 * GiB, 'Full qualification requires at least 2 GiB of payloads');
  }
  for (const path of ['output', 'cli', 'desktop']) result[path] = resolve(result[path]);
  return result;
}

export function workloadSize(config) {
  let requestBytes = 0, responseBytes = 0;
  for (let index = 0; index < config.requestsPerNavigation; index++) {
    requestBytes += requestSize(index);
    responseBytes += RESPONSE_SIZES[index % RESPONSE_SIZES.length];
  }
  return { exchanges: config.navigations * config.requestsPerNavigation,
    requestBytes: requestBytes * config.navigations, responseBytes: responseBytes * config.navigations };
}

export function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)] ?? 0;
}

async function bounded(promise, ms, label) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms} ms`)), ms);
    })]);
  } finally { clearTimeout(timer); }
}

async function waitFor(check, label, timeout = 60000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await delay(100);
  }
  throw new Error(`${label} timed out`);
}

async function fileHash(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

function assignments(output) {
  return Object.fromEntries(output.split(/\r?\n/).filter(line => line.includes('=')).map(line => {
    const index = line.indexOf('='); return [line.slice(0, index), line.slice(index + 1)];
  }));
}

async function startTarget(kind, config, root, ca, samples) {
  const profile = join(root, 'profile');
  const managed = join(profile, 'Transmog');
  await mkdir(managed, { recursive: true });
  let port;
  if (kind === 'desktop') {
    assert.equal(process.platform, 'win32', 'Desktop qualification requires Windows');
    const listener = createServer();
    await listen(listener); port = listener.address().port; await closeServer(listener);
    await writeFile(join(managed, 'interception-ca.pem'), await readFile(ca.cert));
    await writeFile(join(managed, 'interception-ca.key'), await readFile(ca.key));
    await writeFile(join(managed, 'certificate-ownership-v1.json'), JSON.stringify({ schema: 1, sha256: ca.sha256 }));
    // Suppress the native startup updater before launching; no live-site check.
    await writeFile(join(managed, 'update-preferences.1.json'), JSON.stringify({ remindAfterUnixMs: Date.now() + 86400000 }));
  }
  const capture = join(root, 'traffic.tmcap');
  const hostConfig = { binary: kind === 'cli' ? config.cli : config.desktop,
    arguments: kind === 'cli' ? ['serve', '--ca-cert', ca.cert, '--ca-key', ca.key,
      '--listen', '127.0.0.1:0', '--route', 'h1', '--capture', capture, '--capture-bodies'] : [],
    log: join(root, 'target.log'), closeHelper: join(repo, 'scripts/close-desktop-probe-window.ps1'),
    environment: { LOCALAPPDATA: profile, RUST_LOG: 'warn',
      ...(kind === 'desktop' ? { WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1 --disable-background-networking --host-resolver-rules="MAP * ~NOTFOUND, EXCLUDE transmog-ui.localhost"` } : {}) } };
  const configPath = join(root, 'process.json');
  await writeFile(configPath, JSON.stringify(hostConfig, null, 2));
  const child = spawn(config.python, [join(directory, 'process-host.py'), '--kind', kind, '--config', configPath],
    { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  let pid, failure, stopping = false, exited;
  const stderr = [];
  const completion = new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => {
      exited = code;
      if (code !== 0) reject(new Error(`Process host exited ${code}: ${stderr.join('')}`));
      else resolve();
    });
  });
  completion.catch(error => { failure = error; });
  child.stderr.on('data', data => stderr.push(String(data)));
  createInterface({ input: child.stdout }).on('line', line => {
    try {
      const event = JSON.parse(line);
      if (event.type === 'started') pid = event.pid;
      if (event.type === 'sample') samples.push(event);
      if (event.type === 'sample-error') failure = new Error(event.message);
      if (event.type === 'exited' && !stopping) failure = new Error(`Target exited unexpectedly: ${event.code}`);
      if (event.type === 'target-exited' && !stopping) failure = new Error(`Target exited unexpectedly: ${event.code}`);
    } catch (error) { failure = error; }
  });
  const health = () => { if (failure) throw failure; if (exited !== undefined && !stopping) throw new Error('Target process host exited'); };
  const stop = async () => {
    if (!stopping) { stopping = true; child.stdin.end('stop\n'); }
    await bounded(completion, 75000, 'Target shutdown');
  };
  try { await waitFor(() => { health(); return pid; }, 'Target startup'); }
  catch (error) { await stop().catch(() => {}); throw error; }
  return { pid, port, profile, capture, log: hostConfig.log, health, stop };
}

async function connectDesktop(target) {
  const page = await waitFor(async () => {
    target.health();
    try {
      const pages = await fetch(`http://127.0.0.1:${target.port}/json/list`, { signal: AbortSignal.timeout(1000) }).then(r => r.json());
      return pages.find(page => page.type === 'page' && page.url.includes('transmog-ui'));
    } catch { return null; }
  }, 'WebView2 discovery');
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await bounded(new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true });
  }), 10000, 'WebView2 connection');
  let next = 0;
  const pending = new Map(), errors = [];
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(String(data));
    if (message.method === 'Runtime.exceptionThrown') errors.push(message.params.exceptionDetails);
    const waiter = pending.get(message.id);
    if (waiter) { pending.delete(message.id); clearTimeout(waiter.timer);
      if (message.error) waiter.reject(new Error(message.error.message)); else waiter.resolve(message.result); }
  });
  socket.addEventListener('close', () => {
    for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error('WebView2 disconnected')); }
    pending.clear();
  });
  const call = (method, params = {}, timeout = 60000) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, timeout);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expression, timeout) => {
    const result = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, timeout);
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result?.value;
  };
  await call('Runtime.enable');
  await waitFor(() => evaluate('Boolean(document.querySelector("app-shell")?.traffic && window.__TAURI_INTERNALS__)'), 'Desktop hydration');
  return { call, evaluate, errors, close: () => socket.close(),
    invoke: (name, args = {}, timeout) => evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(name)},${JSON.stringify(args)})`, timeout) };
}

async function configureDesktop(client, config, ca) {
  await client.evaluate('document.querySelector("app-shell").settings.ready');
  const state = await client.invoke('product_state');
  state.preferences.configureSystemProxy = false;
  state.privacy.maxLiveEntries = null;
  state.privacy.retainRequestBodies = true;
  state.privacy.retainResponseBodies = true;
  state.privacy.bufferLimit = { mode: config.storage === 'disk' ? 'unlimited' : 'automatic' };
  await client.invoke('save_product_state', { productState: state });
  const buffer = await client.invoke('buffer_status');
  if (config.storage === 'memory') {
    const expected = workloadSize(config);
    assert.ok(buffer.maxBytes >= 2 * (expected.requestBytes + expected.responseBytes) + 64 * 1024 ** 2,
      'Automatic memory budget cannot retain this workload; choose --storage disk or a larger host');
    assert.equal(buffer.storage, 'memory');
  } else assert.equal(buffer.storage, 'disk');
  const status = await client.invoke('start_proxy', { request: { caCertificatePath: ca.cert,
    caPrivateKeyPath: ca.key, listen: '127.0.0.1:0', route: 'http1', allowRemoteClients: false } });
  assert.equal(status.lifecycle, 'running');
  assert.equal(status.hostRestorePending, false);
  await client.evaluate('document.querySelector("app-shell").refreshStatus()');
  // The plain-HTTP fixture intentionally needs no HTTPS setup. Use the normal
  // dismissal action for its startup setup notice, keeping Traffic unobstructed.
  await client.evaluate('document.querySelector("app-shell").settings.dismissNotice()');
  return { address: status.listener, buffer };
}

async function workload(page, site, config, target, progress) {
  const durations = [], navigations = [];
  let completed = 0, requestBytes = 0, responseBytes = 0;
  for (let index = 0; index < config.navigations; index++) {
    target.health();
    const start = performance.now();
    const path = index % 5 === 0 ? `/redirect/${index}` : `/page/${index}`;
    await page.goto(site.origin + path, { waitUntil: 'load' });
    await page.waitForFunction(() => typeof globalThis.runLoadBatch === 'function');
    const batch = await bounded(page.evaluate(({ index, config }) =>
      globalThis.runLoadBatch(index, config.requestsPerNavigation, config.concurrency), { index, config }), 90000, 'Navigation batch');
    assert.equal(batch.completed, config.requestsPerNavigation);
    completed += batch.completed; requestBytes += batch.requestBytes; responseBytes += batch.responseBytes;
    durations.push(...batch.durations);
    if (index % 10 === 0) await page.reload({ waitUntil: 'load' });
    if (index > 0 && index % 10 === 1) {
      await page.goBack({ waitUntil: 'load' }); await page.goForward({ waitUntil: 'load' });
    }
    navigations.push({ index, milliseconds: performance.now() - start, completed, requestBytes, responseBytes });
    progress.push({ timestamp: Date.now(), ...navigations.at(-1) });
    if (index % 5 === 0 || index === config.navigations - 1) {
      console.log(`  navigation ${index + 1}/${config.navigations}: ${completed} verified exchanges, ${((requestBytes + responseBytes) / GiB).toFixed(2)} GiB`);
    }
  }
  assert.deepEqual({ exchanges: completed, requestBytes, responseBytes }, workloadSize(config));
  assert.deepEqual(site.stats.errors, []);
  return { completed, requestBytes, responseBytes, navigations,
    latencyMs: { p50: percentile(durations, .50), p95: percentile(durations, .95), p99: percentile(durations, .99), max: Math.max(...durations) } };
}

async function verifyDesktop(client, site, root) {
  const view = await waitFor(async () => {
    const view = await client.invoke('query_traffic_view', { query: {} });
    const active = await client.invoke('query_sessions', { query: { terminal: false, limit: 1 } });
    if (view.page.sequenceGaps > 0 || view.page.evicted > 0) {
      await client.invoke('release_traffic_view', { viewId: view.id });
      throw new Error(`Retained traffic lost evidence: ${view.page.sequenceGaps} observer sequence gaps, ${view.page.evicted} evictions`);
    }
    if (view.page.retainedCount === site.stats.completed && active.totalMatched === 0) return view;
    await client.invoke('release_traffic_view', { viewId: view.id }); return null;
  }, 'Retained traffic completion');
  try {
    assert.equal(view.ids.length, site.stats.completed);
    assert.equal(view.page.evicted, 0); assert.equal(view.page.sequenceGaps, 0);
    let requestBytes = 0, responseBytes = 0;
    const samples = [];
    for (let offset = 0; offset < view.ids.length; offset += 100) {
      const rows = await client.invoke('traffic_view_rows', { viewId: view.id, offset, limit: 100 });
      assert.ok(rows.length > 0);
      for (const row of rows) {
        assert.equal(row.terminal, 'completed'); assert.equal(row.loss, false);
        assert.ok([200, 302].includes(row.status)); assert.equal(row.host, '127.0.0.1');
        requestBytes += row.requestBytes; responseBytes += row.responseBytes;
        if (samples.length < 3 && row.path.startsWith('/exchange/')) samples.push(row.id);
      }
    }
    // Traffic sizes choose one message boundary. The cache and native trace
    // retain both original and effective boundaries independently.
    assert.equal(requestBytes, site.stats.requestBytes);
    assert.equal(responseBytes, site.stats.responseBytes);
    for (const id of samples) {
      const detail = await client.invoke('session_detail', { id });
      assert.ok(detail.storedBodies.length > 0);
      for (const body of detail.storedBodies) assert.equal(body.availability, 'complete');
    }
    const buffer = await client.invoke('buffer_status');
    assert.equal(buffer.retainedBytes, 2 * (requestBytes + responseBytes));
    await client.evaluate('(async()=>{const s=document.querySelector("app-shell");await s.activateView("traffic");await s.traffic.refreshSessions(undefined,true);})()');
    const last = await client.evaluate('document.querySelector("app-shell").traffic.virtualList.ids.at(-1)');
    const key = async (value, code, virtualKey, modifiers = 0) => {
      await client.call('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: value, code, windowsVirtualKeyCode: virtualKey, modifiers });
      await client.call('Input.dispatchKeyEvent', { type: 'keyUp', key: value, code, windowsVirtualKeyCode: virtualKey, modifiers });
    };
    await client.evaluate('document.querySelector("app-shell").traffic.trafficTable.focus()');
    await key('End', 'End', 35);
    await waitFor(() => client.evaluate(`document.querySelector('app-shell').traffic.selectedSessionId===${JSON.stringify(last)}`), 'Offscreen keyboard selection');
    await key('a', 'KeyA', 65, 2);
    await waitFor(() => client.evaluate(`document.querySelector('app-shell').traffic.selectedTrafficCount===${view.ids.length}`), 'Select all retained entries');
    await key('Escape', 'Escape', 27); await key('End', 'End', 35);
    const rendered = await client.evaluate(`(()=>{const w=document.querySelector('app-shell').traffic;return {total:w.totalMatched,rows:w.sessions.length,selected:w.selectedSessionId,selectedCount:w.selectedTrafficCount,scrollTop:w.sessionScroller.scrollTop};})()`);
    assert.equal(rendered.total, view.ids.length);
    assert.ok(rendered.rows < 150, 'Traffic DOM grew with the full catalog');
    if (view.ids.length >= 10000) assert.ok(rendered.scrollTop > 100000, 'End did not scroll across the full retained collection');
    await delay(500);
    const image = await client.call('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(root, 'desktop-populated.png'), Buffer.from(image.data, 'base64'));
    assert.deepEqual(client.errors, []);
    return { entries: view.ids.length, requestBytes, responseBytes, buffer, rendered,
      evicted: view.page.evicted, sequenceGaps: view.page.sequenceGaps };
  } finally { await client.invoke('release_traffic_view', { viewId: view.id }); }
}

async function inspectCapture(config, path) {
  const { stdout } = await command(config.cli, ['capture', 'inspect', '--input', path],
    { timeout: 300000, windowsHide: true });
  const summary = assignments(stdout);
  await writeFile(join(dirname(path), 'capture-inspection.json'), JSON.stringify(summary, null, 2));
  return { ...summary, fileBytes: (await stat(path)).size, path };
}

async function captureEvidence(config, path, expected) {
  const summary = await inspectCapture(config, path);
  assert.equal(summary.SEALED, 'true'); assert.equal(summary.TRUNCATED_TAIL, 'false');
  assert.equal(Number(summary.EXCHANGES), expected.completed);
  assert.equal(Number(summary.LOSS_MARKERS), 0);
  assert.equal(Number(summary.RETAINED_BODY_BYTES), 2 * (expected.requestBytes + expected.responseBytes),
    'The native trace did not retain both complete observation boundaries');
  return summary;
}

async function runTarget(kind, config, runRoot, ca) {
  const root = join(runRoot, kind);
  await mkdir(root);
  const samples = [], progress = [];
  const result = { target: kind, passed: false, qualified: !config.smoke, samples, progress };
  let target, desktop, site, browser;
  console.log(`Starting ${kind} (${config.smoke ? 'smoke' : 'full qualification'})…`);
  try {
    target = await startTarget(kind, config, root, ca, samples);
    result.pid = target.pid;
    let address;
    if (kind === 'desktop') {
      desktop = await connectDesktop(target);
      const started = await configureDesktop(desktop, config, ca);
      await assert.rejects(stat(join(target.profile, 'Transmog/proxy-recovery-v1.json')), { code: 'ENOENT' },
        'Manual client routing created a Windows proxy recovery journal');
      address = started.address; result.initialBuffer = started.buffer;
    } else {
      address = await waitFor(async () => { target.health();
        const log = await readFile(target.log, 'utf8'); return /^LISTEN_ADDR=(.+)$/m.exec(log)?.[1].trim();
      }, 'CLI listener');
    }
    result.proxyAddress = address;
    await delay(1100); // An idle sample after proxy startup, before the load client exists.
    result.idleSample = samples.at(-1);
    site = await startSite(address);
    result.origin = site.origin;
    browser = await chromium.launch({ headless: !config.headed,
      proxy: { server: site.gateway, bypass: '<-loopback>' },
      env: { ...process.env, CHROME_LOG_FILE: join(root, 'chromium.log') },
      args: ['--disable-quic', '--disable-background-networking', '--disable-features=BackForwardCache',
        '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1'] });
    result.browserVersion = browser.version();
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const page = await context.newPage();
    page.setDefaultTimeout(30000); page.setDefaultNavigationTimeout(60000);
    const browserErrors = [];
    page.on('pageerror', error => browserErrors.push(String(error)));
    page.on('requestfailed', request => browserErrors.push(`${request.url()}: ${request.failure()?.errorText}`));
    const start = performance.now();
    result.workload = await bounded(workload(page, site, config, target, progress), config.timeoutMinutes * 60000, 'Load workload');
    result.loadedSample = samples.at(-1);
    result.elapsedMs = performance.now() - start;
    result.exchangesPerSecond = result.workload.completed / (result.elapsedMs / 1000);
    result.payloadBytesPerSecond = (result.workload.requestBytes + result.workload.responseBytes) / (result.elapsedMs / 1000);
    assert.deepEqual(browserErrors, []);
    await browser.close(); browser = null;
    result.originStats = structuredClone(site.stats);
    assert.equal(site.stats.exchanges, result.workload.completed);
    if (desktop) {
      result.traffic = await verifyDesktop(desktop, site, root);
      const stop = await desktop.invoke('stop_application');
      assert.equal(stop.hostRestorePending, false);
      await waitFor(async () => (await desktop.invoke('app_status')).lifecycle === 'stopped', 'Proxy drain');
      const exportStart = performance.now();
      const saved = await desktop.invoke('export_live_capture', {}, 600000);
      result.saveMs = performance.now() - exportStart;
      assert.equal(saved.records, site.stats.completed);
      assert.ok(saved.fidelity.includes('0 incomplete body boundaries'), saved.fidelity);
      result.capture = await captureEvidence(config, saved.destination, site.stats);
      result.clear = await desktop.invoke('clear_traffic', {}, 120000);
      assert.equal((await desktop.invoke('query_sessions', { query: {} })).retainedCount, 0);
      if (result.clear.undoable) {
        assert.ok(config.smoke, 'A scale capture unexpectedly retained Clear all Undo');
        assert.equal((await desktop.invoke('buffer_status')).retainedBytes, result.traffic.buffer.retainedBytes);
      } else assert.equal((await desktop.invoke('buffer_status')).retainedBytes, 0);
      await delay(2000);
      result.clearedSample = samples.at(-1);
    }
    target.health();
    await target.stop();
    if (desktop) {
      const remaining = await readdir(join(target.profile, 'Transmog/body-cache-v1')).catch(error => {
        if (error.code === 'ENOENT') return []; throw error;
      });
      assert.deepEqual(remaining, [], 'Desktop shutdown left live body-cache files');
    }
    if (!desktop) {
      const log = await readFile(target.log, 'utf8');
      result.proxyCompletions = log.split(/\r?\n/).filter(line => line.startsWith('EVIDENCE ')).length;
      assert.equal(result.proxyCompletions, site.stats.completed);
      result.capture = await captureEvidence(config, target.capture, site.stats);
    }
    assert.ok(samples.length, 'Memory sampling produced no evidence');
    result.passed = true;
  } catch (error) {
    result.error = String(error.stack ?? error);
    if (browser) await browser.contexts()[0]?.pages()[0]?.screenshot({ path: join(root, 'failure.png') }).catch(() => {});
    if (desktop) {
      await desktop.call('Page.captureScreenshot', { format: 'png' }).then(image =>
        writeFile(join(root, 'failure-native.png'), Buffer.from(image.data, 'base64'))).catch(() => {});
      try {
        const view = await desktop.invoke('query_traffic_view', { query: {} });
        const active = await desktop.invoke('query_traffic_view', { query: { terminal: false } });
        result.diagnostics = { expectedCompleted: site?.stats.completed, retainedCount: view.ids.length,
          sequenceGaps: view.page.sequenceGaps, evicted: view.page.evicted,
          pendingCount: active.ids.length, pendingEntries: active.page.sessions };
        await desktop.invoke('release_traffic_view', { viewId: view.id });
        await desktop.invoke('release_traffic_view', { viewId: active.id });
      } catch (error) { result.diagnosticsError = String(error); }
      await writeFile(join(root, 'failure-diagnostics.json'), JSON.stringify({ error: result.error,
        diagnostics: result.diagnostics, diagnosticsError: result.diagnosticsError,
        workload: result.workload, originStats: site?.stats }, null, 2));
      // Preserve a completed workload's failing retained snapshot before native
      // shutdown releases it. Saving diagnostic evidence never changes passed.
      if (result.workload && !result.capture) {
        try {
          await desktop.invoke('stop_application');
          await waitFor(async () => (await desktop.invoke('app_status')).lifecycle === 'stopped', 'Diagnostic proxy drain');
          const saved = await desktop.invoke('export_live_capture', {}, 600000);
          result.failureCapture = { save: saved, inspection: await inspectCapture(config, saved.destination) };
        } catch (error) { result.failureCaptureError = String(error); }
      }
    }
  } finally {
    if (site) result.originStats = structuredClone(site.stats);
    if (!result.workload && progress.length) result.partialWorkload = progress.at(-1);
    await browser?.close().catch(error => { result.cleanupError = String(error); result.passed = false; });
    if (desktop) {
      await desktop.invoke('stop_application').catch(() => {});
      desktop.close();
    }
    await site?.close().catch(error => { result.cleanupError = String(error); result.passed = false; });
    await target?.stop().catch(error => { result.cleanupError = String(error); result.passed = false; });
    if (samples.length) {
      const rss = sample => sample?.processes.reduce((sum, process) => sum + process.rss, 0) ?? null;
      const privateBytes = sample => process.platform === 'win32'
        ? sample?.processes.reduce((sum, process) => sum + process.private, 0) ?? null : null;
      result.memory = { baselineTreeRss: rss(result.idleSample), peakTreeRss: Math.max(...samples.map(rss)),
        loadedTreeRss: rss(result.loadedSample), clearedTreeRss: rss(result.clearedSample),
        finalTreeRss: rss(samples.at(-1)), baselineTreePrivate: privateBytes(result.idleSample),
        peakTreePrivate: process.platform === 'win32' ? Math.max(...samples.map(privateBytes)) : null,
        loadedTreePrivate: privateBytes(result.loadedSample), clearedTreePrivate: privateBytes(result.clearedSample),
        finalTreePrivate: privateBytes(samples.at(-1)), sampleIntervalMs: 1000 };
    }
    await writeFile(join(root, 'result.json'), JSON.stringify(result, null, 2));
  }
  console.log(`${kind}: ${result.passed ? 'PASS' : 'FAIL'}; evidence: ${root}`);
  return result;
}

async function main() {
  if (process.argv.includes('--help')) {
    console.log('Usage: node e2e/load/run.mjs [--target both|cli|desktop] [--smoke] [--headed]\n' +
      '  [--navigations 100] [--requests-per-navigation 100] [--concurrency 8]\n' +
      '  [--storage disk|memory] [--timeout-minutes 30] [--output artifacts/load]\n' +
      '  [--cli PATH] [--desktop PATH] [--python python]\nSee docs/load-testing.md.'); return;
  }
  const config = options(process.argv.slice(2));
  assert.ok(['win32', 'linux'].includes(process.platform), 'Process memory qualification supports Windows and Linux');
  await command(config.python, ['--version'], { timeout: 10000, windowsHide: true });
  if (config.target !== 'cli') assert.equal(process.platform, 'win32', 'Use --target cli on non-Windows hosts');
  await stat(config.cli);
  if (config.target !== 'cli') {
    await stat(config.desktop);
    const { stdout } = await command('pwsh.exe', ['-NoProfile', '-NonInteractive', '-Command',
      "@(Get-Process -Name transmog -ErrorAction SilentlyContinue).Count"], { windowsHide: true });
    assert.equal(Number(stdout.trim()), 0, 'Close Transmog before starting an isolated desktop load run');
  }
  await mkdir(config.output, { recursive: true });
  const runRoot = join(config.output, new Date().toISOString().replace(/[:.]/g, '-') + '-' + process.pid);
  await mkdir(runRoot);
  const ca = { cert: join(runRoot, 'ca.pem'), key: join(runRoot, 'ca.key') };
  const results = [];
  const report = { schema: 1, startedAt: new Date().toISOString(), config, expected: workloadSize(config),
    node: process.version, platform: os.platform(), release: os.release(), architecture: os.arch(), cpus: os.cpus().length };
  try {
    report.gitRevision = (await command('git', ['rev-parse', 'HEAD'], { cwd: repo })).stdout.trim();
    report.gitDirty = Boolean((await command('git', ['status', '--porcelain'], { cwd: repo })).stdout.trim());
    report.cliSha256 = await fileHash(config.cli);
    if (config.target !== 'cli') report.desktopSha256 = await fileHash(config.desktop);
    const generated = await command(config.cli, ['ca', 'generate', '--cert', ca.cert, '--key', ca.key,
      '--name', 'Transmog local load fixture'], { timeout: 60000, windowsHide: true });
    ca.sha256 = assignments(generated.stdout).CA_SHA256;
    for (const kind of config.target === 'both' ? ['cli', 'desktop'] : [config.target]) {
      results.push(await runTarget(kind, config, runRoot, ca));
    }
  } catch (error) {
    report.setupError = String(error.stack ?? error);
    throw error;
  } finally {
    const expectedTargets = config.target === 'both' ? 2 : 1;
    report.results = results; report.passed = !report.setupError && results.length === expectedTargets && results.every(result => result.passed);
    report.finishedAt = new Date().toISOString();
    await writeFile(join(runRoot, 'report.json'), JSON.stringify(report, null, 2));
    const lines = ['# Local Chromium load test', '', `Outcome: ${report.passed ? 'PASS' : 'FAIL'} (${config.smoke ? 'smoke, not scale qualification' : 'scale qualification'})`, '',
      '| Target | Verified exchanges | Request + response GiB | Seconds | p95 ms | Peak process tree MiB |',
      '| --- | ---: | ---: | ---: | ---: | ---: |'];
    for (const result of results) {
      const measured = result.workload ?? result.partialWorkload;
      lines.push(`| ${result.target}: ${result.passed ? 'PASS' : 'FAIL'} | ${measured?.completed ?? 0} | ${((measured?.requestBytes + measured?.responseBytes || 0) / GiB).toFixed(2)} | ${result.elapsedMs ? (result.elapsedMs / 1000).toFixed(1) : '—'} | ${result.workload ? result.workload.latencyMs.p95.toFixed(1) : '—'} | ${result.memory ? (result.memory.peakTreeRss / 1024 ** 2).toFixed(1) : '—'} |`);
    }
    for (const result of results) if (result.error) lines.push('', '```text', result.error, '```');
    if (report.setupError) lines.push('', '```text', report.setupError, '```');
    await writeFile(join(runRoot, 'report.md'), lines.join('\n') + '\n');
    // Remove only the fresh run's generated CA material. Profiles contain an
    // OS-protected copy, retained for diagnosing failed native startups.
    await rm(ca.key, { force: true }); await rm(ca.cert, { force: true });
    console.log(`Report: ${join(runRoot, 'report.md')}`);
  }
  if (!report.passed) process.exitCode = 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error); process.exitCode = 1; });
}
