import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createServer, request, Agent } from 'node:http';
import { once } from 'node:events';

export const RESPONSE_SIZES = [64, 128, 256, 512].map(kib => kib * 1024);
export const requestSize = index => index % 3 === 2 ? 0 : (index % 3 + 1) * 128 * 1024;

// Shared by the server and its browser script. Every byte is checked in both
// directions; repeated deterministic blocks keep the fixture bounded in RAM.
function pattern() {
  const bytes = new Uint8Array(65536);
  let state = 0x12345678;
  for (let i = 0; i < bytes.length; i++) {
    state ^= state << 13; state ^= state >>> 17; state ^= state << 5;
    bytes[i] = state & 255;
  }
  return bytes;
}

function browserWorkload() {
  const bytes = pattern();
  globalThis.runLoadBatch = async (navigation, count, concurrency) => {
    let next = 0;
    let requestBytes = 0, responseBytes = 0, completed = 0;
    const durations = [];
    const controller = new AbortController();
    const workers = Array.from({ length: concurrency }, async () => {
      while (next < count && !controller.signal.aborted) {
        const index = next++;
        const start = performance.now();
        const size = requestSize(index);
        const body = size ? new Uint8Array(size) : undefined;
        if (body) for (let offset = 0; offset < size; offset += bytes.length) body.set(bytes, offset);
        const response = await fetch(`/exchange/${navigation}/${index}`, {
          method: body ? 'POST' : 'GET', body,
          signal: controller.signal,
          headers: { 'Content-Type': 'application/octet-stream' }, cache: 'no-store',
        });
        if (response.status !== 200) throw new Error(`Exchange ${navigation}/${index}: ${response.status}`);
        const result = new Uint8Array(await response.arrayBuffer());
        if (result.length !== RESPONSE_SIZES[index % RESPONSE_SIZES.length]) throw new Error('Response length mismatch');
        for (let i = 0; i < result.length; i++) {
          if (result[i] !== bytes[i % bytes.length]) throw new Error(`Response corruption at ${i}`);
        }
        requestBytes += size; responseBytes += result.length; completed++;
        durations.push(performance.now() - start);
      }
    });
    try { await Promise.all(workers); }
    catch (error) { controller.abort(); await Promise.allSettled(workers); throw error; }
    document.querySelector('output').textContent = `${completed} verified exchanges`;
    return { completed, requestBytes, responseBytes, durations };
  };
}

export async function listen(server) {
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return `http://127.0.0.1:${server.address().port}`;
}

export async function closeServer(server) {
  const closed = new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  server.closeAllConnections?.();
  await closed;
}

/** Local site plus a strict egress gate in front of the target proxy. */
export async function startSite(proxyAddress) {
  assert.match(proxyAddress, /^127\.0\.0\.1:\d+$/);
  const token = randomUUID();
  const block = Buffer.from(pattern());
  const stats = { completed: 0, requestBytes: 0, responseBytes: 0, exchanges: 0,
    documents: 0, redirects: 0, chunks: 0, errors: [], blocked: 0 };
  let origin;
  const script = `const RESPONSE_SIZES=${JSON.stringify(RESPONSE_SIZES)};const requestSize=${requestSize};\n${pattern}\n(${browserWorkload})();`;
  const server = createServer(async (req, res) => {
    try {
      assert.equal(req.headers['x-load-gate'], token, 'Chromium bypassed the scoped proxy');
      const url = new URL(req.url, origin);
      assert.equal(url.origin, origin);
      assert.equal(req.headers.host, new URL(origin).host);
      let received = 0;
      for await (const chunk of req) {
        for (let i = 0; i < chunk.length; i++) assert.equal(chunk[i], block[(received + i) % block.length]);
        received += chunk.length;
      }
      let body, size, chunked = false;
      let status = 200, type = 'text/html; charset=utf-8';
      const exchange = /^\/exchange\/(\d+)\/(\d+)$/.exec(url.pathname);
      if (exchange) {
        const index = Number(exchange[2]);
        assert.equal(received, requestSize(index));
        assert.equal(req.method, received ? 'POST' : 'GET');
        size = RESPONSE_SIZES[index % RESPONSE_SIZES.length];
        type = 'application/octet-stream';
        chunked = index % 2 === 0;
      } else if (/^\/redirect\/\d+$/.test(url.pathname)) {
        status = 302; body = Buffer.alloc(0);
        res.setHeader('Location', url.pathname.replace('/redirect/', '/page/'));
      } else if (/^\/page\/\d+$/.test(url.pathname)) {
        body = Buffer.from(`<!doctype html><html><head><meta charset="utf-8"><title>Synthetic load</title><link rel="icon" href="/pixel.svg"><link rel="stylesheet" href="/site.css"><script src="/workload.js" defer></script></head><body><h1>Synthetic navigation ${url.pathname}</h1><output>Ready</output><img src="/pixel.svg"><iframe src="/frame" title="Local frame"></iframe><a href="/page/0">Home</a></body></html>`);
      } else if (url.pathname === '/workload.js') {
        type = 'text/javascript'; body = Buffer.from(script);
      } else if (url.pathname === '/site.css') {
        type = 'text/css'; body = Buffer.from('body { font: 16px system-ui; margin: 2rem; } output { display: block; }');
      } else if (url.pathname === '/pixel.svg') {
        type = 'image/svg+xml'; body = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="green"/></svg>');
      } else if (url.pathname === '/frame') {
        body = Buffer.from('<!doctype html><title>Local frame</title><p>Local iframe resource</p>');
      } else {
        throw new Error(`Unexpected fixture URL: ${req.url}`);
      }
      size ??= body.length;
      res.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store',
        'Content-Security-Policy': "default-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'",
        ...(chunked ? {} : { 'Content-Length': size }) });
      res.once('finish', () => {
        stats.completed++; stats.requestBytes += received; stats.responseBytes += size;
        if (exchange) stats.exchanges++;
        if (url.pathname.startsWith('/page/')) stats.documents++;
        if (status === 302) stats.redirects++;
        if (chunked) stats.chunks++;
      });
      if (body) res.end(body);
      else {
        for (let offset = 0; offset < size; offset += block.length) {
          if (!res.write(block.subarray(0, Math.min(block.length, size - offset)))) await once(res, 'drain');
        }
        res.end();
      }
    } catch (error) {
      stats.errors.push(String(error));
      res.destroy(error);
    }
  });
  // This fixture has a lifetime owned by the runner. Do not introduce Node's
  // five-second idle-close race into a capture/retention scale measurement.
  server.keepAliveTimeout = 0;
  origin = await listen(server);
  const [host, port] = proxyAddress.split(':');
  const agent = new Agent({ keepAlive: true, maxSockets: 32 });
  const gate = createServer((req, res) => {
    let target;
    try { target = new URL(req.url); } catch { /* Reject non-proxy requests. */ }
    if (!target || target.origin !== origin || req.headers.host !== new URL(origin).host) {
      stats.blocked++; res.writeHead(403); res.end(); return;
    }
    const upstream = request({ host, port, path: req.url, method: req.method, agent,
      headers: { ...req.headers, 'x-load-gate': token } }, response => {
      res.writeHead(response.statusCode, response.headers); response.pipe(res);
    });
    upstream.on('error', error => { stats.errors.push(String(error)); res.destroy(error); });
    req.on('aborted', () => upstream.destroy());
    res.on('close', () => { if (!res.writableFinished) upstream.destroy(); });
    req.pipe(upstream);
  });
  gate.keepAliveTimeout = 0;
  gate.on('connect', (_req, socket) => { stats.blocked++; socket.end('HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n'); });
  const gateway = await listen(gate);
  return { origin, gateway, stats, async close() {
    agent.destroy(); await closeServer(gate); await closeServer(server);
  } };
}
