import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash, X509Certificate } from 'node:crypto';
import { join } from 'node:path';
import { chromium } from 'playwright';

const [binary, artifacts, channel] = process.argv.slice(2);
assert.ok(binary && artifacts, 'Expected fixture executable and artifacts directory');
const results = [];
for (const [signature, groups] of [['automatic', 'hybrid'], ['mldsa44', 'hybrid'], ['mldsa65', 'hybrid'], ['mldsa87', 'hybrid'], ['classical', 'hybrid'], ['automatic', 'classical']]) {
  const certificatePath = join(artifacts, `${signature}-${groups}.pem`);
  const fixture = spawn(binary, [certificatePath, signature, groups], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '', errors = '';
  fixture.stdout.on('data', bytes => output += bytes.toString());
  fixture.stderr.on('data', bytes => errors += bytes.toString());
  const finished = new Promise((resolve, reject) => {
    fixture.once('error', reject);
    fixture.once('exit', code => code === 0 ? resolve() : reject(new Error(`Fixture failed (${code}): ${errors}`)));
  });
  // Handle rejection immediately, including during startup/navigation failures.
  finished.catch(() => {});
  let browser;
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
    browser = await chromium.launch({
      ...(channel && channel !== 'chromium' ? { channel } : {}),
      args: ['--disable-background-networking', '--disable-quic', '--disable-http2', `--ignore-certificate-errors-spki-list=${spki}`],
    });
    const context = await browser.newContext({ ignoreHTTPSErrors: false, serviceWorkers: 'block' });
    const page = await context.newPage();
    const response = await page.goto(`https://localhost:${port}/`, { timeout: 15_000 });
    assert.equal(response.status(), 200);
    assert.equal(await response.text(), 'ok');
    await finished;
    const facts = JSON.parse(output.match(/^TLS_FACTS=(.+)$/m)[1]);
    assert.equal(facts['Negotiated group'], groups === 'hybrid' ? 'X25519MLKEM768' : 'X25519');
    assert.equal(facts['Local handshake signature'], signature === 'classical' ? 'ecdsa_secp256r1_sha256' : signature === 'automatic' ? 'mldsa44' : signature);
    results.push({ browser: browser.version(), channel: channel || 'chromium', signature, groups, facts });
  } catch (error) {
    throw new Error(`${signature}/${groups}: ${error.message}\n${errors}`, { cause: error });
  } finally {
    await browser?.close();
    if (fixture.exitCode === null) fixture.kill();
    await finished.catch(() => {});
  }
}
await writeFile(join(artifacts, 'results.json'), JSON.stringify(results, null, 2));
console.log(JSON.stringify(results.map(({browser, signature, groups}) => ({browser, signature, groups}))));
