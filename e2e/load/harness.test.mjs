import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer, request } from 'node:http';
import { createConnection } from 'node:net';
import { closeServer, listen, startSite } from './site.mjs';
import { options, workloadSize, percentile } from './run.mjs';

test('scale qualification cannot silently become a smoke test', () => {
  const config = options([]);
  const expected = workloadSize(config);
  assert.equal(expected.exchanges, 10000);
  assert.ok(expected.responseBytes > 2 * 1024 ** 3);
  assert.throws(() => options(['--navigations', '1']), /10,000/);
  assert.throws(() => options(['--navigations', '10000', '--requests-per-navigation', '1']), /2 GiB/);
  assert.throws(() => options(['--concurrency', 'Infinity']), /Invalid concurrency/);
  assert.throws(() => options(['--target', 'unknown']), /Invalid --target/);
  assert.equal(options(['--smoke']).navigations, 3);
  assert.equal(options(['--smoke', '--navigations', '4']).navigations, 4);
  assert.equal(percentile([4, 1, 2, 3], .95), 4);
});

function getThrough(proxy, url) {
  return new Promise((resolve, reject) => {
    const target = new URL(url);
    const req = request(proxy, { path: url, headers: { Host: target.host } }, response => {
      let bytes = 0;
      response.on('data', chunk => { bytes += chunk.length; });
      response.on('end', () => resolve({ status: response.statusCode, bytes }));
      response.on('error', reject);
    });
    req.on('error', reject); req.end();
  });
}

test('the egress gate requires the target proxy and rejects other destinations before forwarding', async () => {
  let proxyCalls = 0, forbiddenCalls = 0;
  const fakeProxy = createServer((req, res) => {
    proxyCalls++;
    const upstream = request(req.url, { method: req.method, headers: req.headers }, response => {
      res.writeHead(response.statusCode, response.headers); response.pipe(res);
    });
    upstream.on('error', error => res.destroy(error)); req.pipe(upstream);
  });
  const forbidden = createServer((_req, res) => { forbiddenCalls++; res.end('unexpected'); });
  const address = await listen(fakeProxy);
  const forbiddenUrl = await listen(forbidden);
  const site = await startSite(new URL(address).host);
  try {
    const valid = await getThrough(site.gateway, site.origin + '/exchange/0/2');
    assert.deepEqual(valid, { status: 200, bytes: 256 * 1024 });
    assert.equal(proxyCalls, 1);
    assert.equal(site.stats.exchanges, 1);
    assert.deepEqual(await getThrough(site.gateway, forbiddenUrl + '/'), { status: 403, bytes: 0 });
    assert.equal(proxyCalls, 1); assert.equal(forbiddenCalls, 0);
    assert.equal(site.stats.blocked, 1);
    const connectReply = await new Promise((resolve, reject) => {
      const address = new URL(site.gateway);
      const socket = createConnection({ host: address.hostname, port: Number(address.port) });
      let reply = '';
      socket.on('connect', () => socket.write(`CONNECT ${new URL(forbiddenUrl).host} HTTP/1.1\r\nHost: ${new URL(forbiddenUrl).host}\r\n\r\n`));
      socket.on('data', data => { reply += data; });
      socket.on('end', () => resolve(reply)); socket.on('error', reject);
    });
    assert.match(connectReply, /^HTTP\/1\.1 403 Forbidden/);
    assert.equal(proxyCalls, 1); assert.equal(forbiddenCalls, 0);
    await assert.rejects(getThrough(address, site.origin + '/exchange/0/2'), /socket hang up/);
    assert.equal(site.stats.exchanges, 1, 'A request without gate proof was counted');
    assert.ok(site.stats.errors.some(error => error.includes('bypassed')));
  } finally { await site.close(); await closeServer(fakeProxy); await closeServer(forbidden); }
});
