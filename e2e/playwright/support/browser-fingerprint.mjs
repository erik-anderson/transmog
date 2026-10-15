// Isolated local navigation. Fingerprints are decoded from origin wire records,
// never inferred from User-Agent or from this browser driver's response metadata.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { X509Certificate, createHash } from 'node:crypto';
import { readFile, writeFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { chromium } from 'playwright';

const [binary, directory, protocol, leg, channel, proof] = process.argv.slice(2);
assert.ok(binary && directory && ['h1', 'h2', 'h3'].includes(protocol));
assert.ok(['direct', 'proxied'].includes(leg));
const fixture = spawn(binary, [protocol, directory, proof], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
let stdout = '', stderr = '', browser;
fixture.stdout.on('data', bytes => stdout += bytes);
fixture.stderr.on('data', bytes => stderr += bytes);
fixture.stdin.on('error', error => stderr += `\nFixture stdin: ${error.message}\n`);
const exit = new Promise((resolve, reject) => { fixture.once('error', reject); fixture.once('exit', code => resolve(code)); });
exit.catch(() => {});
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
try {
  const deadline = Date.now() + 30000;
  while (!/^READY=.+\n/m.test(stdout)) {
    assert.equal(fixture.exitCode, null, stderr);
    assert.ok(Date.now() < deadline, 'Fixture startup timed out: ' + stderr);
    await pause(25);
  }
  const ready = JSON.parse(stdout.match(/^READY=(.+)$/m)[1]);
  const spki = [];
  for (const name of ['origin-ca.pem', 'proxy-ca.pem']) {
    const root = new X509Certificate(await readFile(join(directory, name)));
    spki.push(createHash('sha256').update(root.publicKey.export({ type: 'spki', format: 'der' })).digest('base64'));
  }
  const port = ready.origin.split(':').at(-1);
  const args = ['--disable-background-networking', '--disable-component-update', '--disable-sync',
    '--host-resolver-rules=MAP localhost 127.0.0.1, EXCLUDE 127.0.0.1, MAP * ~NOTFOUND',
    `--ignore-certificate-errors-spki-list=${spki.join(',')}`];
  if (protocol === 'h3' && leg === 'direct') args.push('--enable-quic', `--origin-to-force-quic-on=localhost:${port}`);
  else args.push('--disable-quic');
  if (protocol === 'h1') args.push('--disable-http2');
  if (leg === 'proxied') args.push('--proxy-bypass-list=<-loopback>');
  browser = await chromium.launch({ channel, headless: true, args,
    ...(leg === 'proxied' ? { proxy: { server: 'http://' + ready.proxy, bypass: '<-loopback>' } } : {}) });
  const context = await browser.newContext({ locale: 'en-US', serviceWorkers: 'block' });
  await context.route('**/*', route => new URL(route.request().url()).hostname === 'localhost' ? route.continue() : route.abort());
  const page = await context.newPage();
  const session = await context.newCDPSession(page);
  await session.send('Network.enable');
  let responseMetadata;
  session.on('Network.responseReceived', ({ response }) => { if (new URL(response.url).pathname === '/probe') responseMetadata = response; });
  const response = await page.goto(`https://localhost:${port}/probe`, { waitUntil: 'load', timeout: 30000 });
  assert.equal(response.status(), 200);
  assert.equal(response.headers()['x-local-probe'], proof);
  assert.ok((await response.text()).includes(proof));
  const expectedBrowserProtocol = leg === 'direct' ? {h1:'http/1.1',h2:'h2',h3:'h3'}[protocol] : protocol === 'h1' ? 'http/1.1' : 'h2';
  assert.equal(responseMetadata?.protocol, expectedBrowserProtocol, 'Browser protocol fallback');
  assert.equal(responseMetadata.securityDetails.issuer, leg === 'proxied' ? 'Local fingerprint proxy CA' : 'Local fingerprint origin CA', 'Proxy bypass or wrong TLS peer');
  const version = browser.version();
  const browserVersion = await session.send('Browser.getVersion');
  await browser.close();
  browser = undefined;
  if (leg === 'proxied') {
    const deadline = Date.now() + 5000;
    while (!(await readdir(directory)).includes('proxy-evidence.json')) {
      assert.ok(Date.now() < deadline, 'Missing completed proxy exchange evidence');
      await pause(25);
    }
    const evidence = JSON.parse(await readFile(join(directory, 'proxy-evidence.json'), 'utf8'));
    assert.equal(evidence.egress, {h1:'Http1',h2:'Http2',h3:'Http3'}[protocol], 'Proxy origin protocol fallback');
  }
  let connections;
  const captureDeadline = Date.now() + 5000;
  do {
    connections = (await readdir(directory)).filter(name => /^connection-\d+\.json$/.test(name));
    if (connections.length) break;
    assert.ok(Date.now() < captureDeadline, 'Missing completed origin capture');
    await pause(25);
  } while (true);
  assert.equal(connections.length, 1, 'Expected exactly one completed origin probe request');
  const origin = JSON.parse(await readFile(join(directory, connections[0]), 'utf8'));
  assert.equal(origin.protocol, protocol, 'Origin protocol fallback');
  assert.equal(origin.tls.alpn, {h1:'http/1.1',h2:'h2',h3:'h3'}[protocol]);
  await writeFile(join(directory, 'browser.json'), JSON.stringify({ browser:version,
    channel, browserVersion,
    requestedProtocol:protocol, leg, observedBrowserProtocol:responseMetadata.protocol,
    certificateIssuer:responseMetadata.securityDetails.issuer, fixture:ready, originRecord:connections[0], proofVerified:true }, null, 2));
} finally {
  try {
    await browser?.close();
  } finally {
    if (fixture.exitCode === null) fixture.stdin.end('stop\n');
    const timer = setTimeout(() => fixture.kill(), 15000);
    try {
      const code = await exit;
      assert.equal(code, 0, 'Fixture shutdown failed: ' + stderr);
    } finally {
      clearTimeout(timer);
      await writeFile(join(directory, 'fixture.log'), stdout + '\n' + stderr);
    }
  }
}
