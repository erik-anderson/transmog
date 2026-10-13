import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createHash, X509Certificate } from 'node:crypto';
import { join } from 'node:path';
import { chromium } from 'playwright';

const [binary, artifacts, channel, trust = 'spki'] = process.argv.slice(2);
assert.ok(binary && artifacts, 'Expected fixture executable and artifacts directory');
assert.ok(['spki', 'nss'].includes(trust), 'Expected spki or nss trust mode');
assert.ok(trust !== 'nss' || process.platform === 'linux', 'Isolated NSS trust requires Linux');
const results = [];
const cases = [['automatic', 'hybrid'], ['mldsa44', 'hybrid'], ['mldsa65', 'hybrid'], ['mldsa87', 'hybrid'], ['classical', 'hybrid'], ['automatic', 'classical']];
if (trust === 'nss') cases.push(['automatic', 'hybrid', 'wrong-host'], ['automatic', 'hybrid', 'untrusted-root']);
for (const [signature, groups, validation = 'valid'] of cases) {
  const certificatePath = join(artifacts, `${signature}-${groups}-${validation}.pem`);
  const fixture = spawn(binary, [certificatePath, signature, groups, ...(validation === 'wrong-host' ? [validation] : [])], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '', errors = '';
  fixture.stdout.on('data', bytes => output += bytes.toString());
  fixture.stderr.on('data', bytes => errors += bytes.toString());
  const finished = new Promise((resolve, reject) => {
    fixture.once('error', reject);
    fixture.once('exit', code => code === 0 ? resolve() : reject(new Error(`Fixture failed (${code}): ${errors}`)));
  });
  // Handle rejection immediately, including during startup/navigation failures.
  finished.catch(() => {});
  let browser, browserHome;
  try {
    const deadline = Date.now() + 15_000;
    while (!output.includes('LISTEN_ADDR=')) {
      if (fixture.exitCode !== null) await finished;
      assert.ok(Date.now() < deadline, `Fixture startup timed out: ${errors}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
    const port = output.match(/LISTEN_ADDR=127\.0\.0\.1:(\d+)/)[1];
    const root = new X509Certificate(await readFile(certificatePath));
    const spki = createHash('sha256').update(root.publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
    if (trust === 'nss') {
      browserHome = await mkdtemp(join(artifacts, 'browser-home-'));
      // Chromium uses the existing legacy location, including releases before M146.
      const database = join(browserHome, '.pki', 'nssdb');
      await mkdir(database, { recursive: true });
      execFileSync('certutil', ['-d', `sql:${database}`, '-N', '--empty-password']);
      if (validation !== 'untrusted-root') {
        execFileSync('certutil', ['-d', `sql:${database}`, '-A', '-t', 'C,,', '-n', 'PQC fixture root', '-i', certificatePath]);
      }
    }
    browser = await chromium.launch({
      ...(channel && channel !== 'chromium' ? { channel } : {}),
      ...(browserHome ? { env: { ...process.env, HOME: browserHome } } : {}),
      args: ['--disable-background-networking', '--disable-quic', '--disable-http2', ...(trust === 'spki' ? [`--ignore-certificate-errors-spki-list=${spki}`] : [])],
    });
    const context = await browser.newContext({ ignoreHTTPSErrors: false, serviceWorkers: 'block' });
    const page = await context.newPage();
    if (validation !== 'valid') {
      await assert.rejects(page.goto(`https://localhost:${port}/`, { timeout: 15_000 }),
        validation === 'wrong-host' ? /ERR_CERT_COMMON_NAME_INVALID/ : /ERR_CERT_AUTHORITY_INVALID/);
      await finished;
      assert.ok(output.includes('TLS_REJECTED='), 'The browser did not reject the TLS fixture');
      results.push({ browser: browser.version(), channel: channel || 'chromium', trust, signature, groups, validation, rejected: true });
      continue;
    }
    const response = await page.goto(`https://localhost:${port}/`, { timeout: 15_000 });
    assert.equal(response.status(), 200);
    assert.equal(await response.text(), 'ok');
    await finished;
    const facts = JSON.parse(output.match(/^TLS_FACTS=(.+)$/m)[1]);
    assert.equal(facts['Negotiated group'], groups === 'hybrid' ? 'X25519MLKEM768' : 'X25519');
    assert.equal(facts['Local handshake signature'], signature === 'classical' ? 'ecdsa_secp256r1_sha256' : signature === 'automatic' ? 'mldsa44' : signature);
    results.push({ browser: browser.version(), channel: channel || 'chromium', trust, signature, groups, validation, facts });
  } catch (error) {
    throw new Error(`${signature}/${groups}: ${error.message}\n${errors}`, { cause: error });
  } finally {
    await browser?.close();
    if (fixture.exitCode === null) fixture.kill();
    await finished.catch(() => {});
    if (browserHome) await rm(browserHome, { recursive: true, force: true });
  }
}
await writeFile(join(artifacts, 'results.json'), JSON.stringify(results, null, 2));
console.log(JSON.stringify(results.map(({browser, signature, groups, trust, validation, rejected}) => ({browser, signature, groups, trust, validation, rejected}))));
