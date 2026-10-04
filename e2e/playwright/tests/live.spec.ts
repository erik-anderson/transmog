import { test, expect, chromium, type Browser } from '@playwright/test';
import { createHash, randomUUID } from 'node:crypto';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';

type Route = 'h1' | 'h2' | 'h3' | 'auto';
type Evidence = Record<string, string>;
type LiveResult = {
  name: string;
  url: string;
  proxyAddress: string;
  browserVersion: string;
  completedAt: string;
  proofId?: string;
  evidence?: Evidence;
  warmupEvidence?: Evidence;
  upgradeEvidence?: Evidence;
  warmupUrl?: string;
  altSvcStripped?: boolean;
  directFallbackBlocked?: boolean;
};

const enabled = process.env.RUSTYMIDDLE_LIVE === '1';
const windows = process.platform === 'win32';
const repo = path.resolve(__dirname, '..', '..', '..');
const binary = process.env.RUSTYMIDDLE_BIN ?? path.join(
  repo,
  'target',
  'release',
  windows ? 'rustymiddle.exe' : 'rustymiddle',
);
const pwsh = process.env.PWSH ?? 'pwsh';
const caCertificate = process.env.RUSTYMIDDLE_TEST_CA_CERT
  ?? path.join(repo, '.local', 'live-test-ca.pem');
const caPrivateKey = process.env.RUSTYMIDDLE_TEST_CA_KEY
  ?? path.join(repo, '.local', 'live-test-ca.key');
const reportPath = path.join(repo, 'verification', 'live-report.json');
const reportMarkdownPath = path.join(repo, 'verification', 'live-report.md');
let caSha256 = '';
let caTrustVerifiedBeforeRun = false;
let caTrustVerifiedAfterRun = false;
const results: LiveResult[] = [];

test.skip(!enabled, 'Set RUSTYMIDDLE_LIVE=1 in the explicitly network-enabled live job.');
test.skip(!windows, 'The initial live verification target is Windows.');

test.beforeAll(() => {
  if (!existsSync(caCertificate) || !existsSync(caPrivateKey)) {
    throw new Error('Missing durable live-test CA. Run scripts/setup-live-test-ca.ps1 once.');
  }
  const verified = runPowerShell([
    path.join(repo, 'scripts', 'verify-ca-user.ps1'),
    '-CertificatePath', caCertificate,
  ]);
  caSha256 = requiredAssignment(verified, 'CA_SHA256');
  caTrustVerifiedBeforeRun = true;
});

test.afterAll(() => {
  const verified = runPowerShell([
    path.join(repo, 'scripts', 'verify-ca-user.ps1'),
    '-CertificatePath', caCertificate,
  ]);
  if (requiredAssignment(verified, 'CA_SHA256') !== caSha256) {
    throw new Error('The durable CA fingerprint changed during the browser run.');
  }
  caTrustVerifiedAfterRun = true;
  const generatedAt = new Date().toISOString();
  const lockHash = createHash('sha256')
    .update(readFileSync(path.join(repo, 'Cargo.lock')))
    .digest('hex');
  mkdirSync(path.dirname(reportPath), { recursive: true });
  const report = {
    generatedAt,
    os: `${process.platform}-${process.arch}`,
    cargoLockSha256: lockHash,
    caSha256,
    chromiumTrustMode: 'durable-current-user-test-ca',
    certificateBypassEnabled: false,
    playwrightRouteInterceptionUsed: false,
    browserQuicDisabled: true,
    serviceWorkersBlocked: true,
    freshBrowserProfilePerCase: true,
    windowsTrustStoreModifiedByRun: false,
    caTrustVerifiedBeforeRun,
    caTrustVerifiedAfterRun,
    caCleanupPerformed: false,
    caCleanupDisposition: 'durable-test-root-retained-by-explicit-user-choice',
    caRemovalCommand: 'pwsh ./scripts/remove-live-test-ca.ps1',
    results,
  };
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  writeFileSync(reportMarkdownPath, renderMarkdownReport(generatedAt, lockHash));
});

for (const liveCase of [
  { name: 'Wikipedia forced H1', route: 'h1' as Route, url: 'https://www.wikipedia.org/', egress: 'Http1' },
  { name: 'Wikipedia forced H2', route: 'h2' as Route, url: 'https://www.wikipedia.org/', egress: 'Http2' },
  { name: 'Cloudflare forced H3', route: 'h3' as Route, url: 'https://cloudflare-quic.com/', egress: 'Http3' },
]) {
  test(liveCase.name, async () => {
    const proofId = randomUUID();
    const proxy = await startProxy(liveCase.route, proofId);
    let browser: Browser | undefined;
    try {
      browser = await chromium.launch({
        proxy: { server: `http://${proxy.listenAddress}` },
        args: chromiumArguments(),
      });
      const context = await browser.newContext({
        ignoreHTTPSErrors: false,
        serviceWorkers: 'block',
      });
      const page = await context.newPage();
      let response;
      try {
        response = await page.goto(liveCase.url, { waitUntil: 'domcontentloaded' });
      } catch (error) {
        throw new Error(`${String(error)}\nproxy output:\n${proxy.lines.join('\n')}`);
      }
      expect(response).not.toBeNull();
      expect(response!.headers()['x-intercepted-by']).toBe(proofId);
      await expect(page.locator(`meta[name="intercept-proxy-proof"][content="${proofId}"]`))
        .toHaveCount(1);
      const hostname = new URL(response!.url()).hostname;
      const evidence = await proxy.waitForEvidence(
        item => item.host === hostname && item.egress === liveCase.egress,
      );
      expect(evidence.request_breakpoint).toBe('true');
      expect(evidence.response_breakpoint).toBe('true');
      if (liveCase.egress === 'Http3') {
        expect(evidence.h3_alpn.startsWith('h3')).toBeTruthy();
        expect(evidence.adapter).toBe('quiche');
        expect(evidence.fallback_count).toBe('0');
      } else {
        expect(evidence.adapter).toBe('hyper');
      }
      expect(evidence.upstream_verification).toBe('verified');
      expect(evidence.request_event_id).toBe(`${evidence.session_id}:request`);
      expect(evidence.response_event_id).toBe(`${evidence.session_id}:response`);
      results.push({
        name: liveCase.name,
        url: response!.url(),
        proxyAddress: proxy.listenAddress,
        browserVersion: browser.version(),
        completedAt: new Date().toISOString(),
        proofId,
        evidence,
      });
      await context.close();
    } finally {
      await browser?.close();
      await proxy.stop();
    }
  });
}

test('Cloudflare Auto learns Alt-Svc and upgrades a later request to H3', async () => {
  const proofId = randomUUID();
  const proxy = await startProxy('auto', proofId);
  let browser: Browser | undefined;
  try {
    browser = await chromium.launch({
      proxy: { server: `http://${proxy.listenAddress}` },
      args: chromiumArguments(),
    });
    const context = await browser.newContext({
      ignoreHTTPSErrors: false,
      serviceWorkers: 'block',
    });
    const page = await context.newPage();
    const warmupUrl = `https://cloudflare-quic.com/?rustymiddle-warmup=${randomUUID()}`;
    const warmup = await page.goto(
      warmupUrl,
      { waitUntil: 'domcontentloaded' },
    );
    expect(warmup).not.toBeNull();
    expect(warmup!.headers()['x-intercepted-by']).toBe(proofId);
    expect(warmup!.headers()['alt-svc']).toBeUndefined();
    await expect(page.locator(`meta[name="intercept-proxy-proof"][content="${proofId}"]`))
      .toHaveCount(1);
    const warmupEvidence = await proxy.waitForEvidence(
      item => item.host === 'cloudflare-quic.com'
        && item.path === '/'
        && item.egress === 'Http2',
    );
    expect(warmupEvidence.request_breakpoint).toBe('true');
    expect(warmupEvidence.response_breakpoint).toBe('true');
    expect(warmupEvidence.adapter).toBe('hyper');
    expect(warmupEvidence.upstream_verification).toBe('verified');

    const upgraded = await page.goto(
      `https://cloudflare-quic.com/?rustymiddle-upgrade=${randomUUID()}`,
      { waitUntil: 'domcontentloaded' },
    );
    expect(upgraded).not.toBeNull();
    expect(upgraded!.headers()['x-intercepted-by']).toBe(proofId);
    expect(upgraded!.headers()['alt-svc']).toBeUndefined();
    await expect(page.locator(`meta[name="intercept-proxy-proof"][content="${proofId}"]`))
      .toHaveCount(1);
    const upgradeEvidence = await proxy.waitForEvidence(
      item => item.host === 'cloudflare-quic.com'
        && item.path === '/'
        && item.egress === 'Http3',
    );
    expect(upgradeEvidence.h3_alpn.startsWith('h3')).toBeTruthy();
    expect(upgradeEvidence.adapter).toBe('quiche');
    expect(upgradeEvidence.upstream_verification).toBe('verified');
    expect(upgradeEvidence.request_breakpoint).toBe('true');
    expect(upgradeEvidence.response_breakpoint).toBe('true');
    expect(upgradeEvidence.fallback_count).toBe('0');
    results.push({
      name: 'Cloudflare Auto Alt-Svc upgrade',
      url: upgraded!.url(),
      proxyAddress: proxy.listenAddress,
      browserVersion: browser.version(),
      completedAt: new Date().toISOString(),
      proofId,
      warmupEvidence,
      warmupUrl,
      upgradeEvidence,
      altSvcStripped: true,
    });
    await context.close();
  } finally {
    await browser?.close();
    await proxy.stop();
  }
});

test('stopped proxy cannot fall back to DIRECT', async () => {
  const proxy = await startProxy('h1', randomUUID());
  const deadAddress = proxy.listenAddress;
  await proxy.stop();
  const browser = await chromium.launch({
    proxy: { server: `http://${deadAddress}` },
    args: chromiumArguments(),
  });
  try {
    const page = await browser.newPage({ ignoreHTTPSErrors: false, serviceWorkers: 'block' });
    await expect(page.goto('https://www.wikipedia.org/')).rejects.toThrow();
    results.push({
      name: 'Stopped proxy blocks DIRECT fallback',
      url: 'https://www.wikipedia.org/',
      proxyAddress: deadAddress,
      browserVersion: browser.version(),
      completedAt: new Date().toISOString(),
      directFallbackBlocked: true,
    });
  } finally {
    await browser.close();
  }
});

function chromiumArguments(): string[] {
  return [
    '--disable-quic',
    '--disable-features=UseDnsHttpsSvcbAlpn',
  ];
}

function runPowerShell(arguments_: string[]): string {
  const result = spawnSync(pwsh, ['-NoProfile', '-File', ...arguments_], {
    cwd: repo,
    encoding: 'utf8',
  });
  if (result.status !== 0) {
    throw new Error(`PowerShell failed (${result.status}):\n${result.stdout}\n${result.stderr}`);
  }
  return `${result.stdout}\n${result.stderr}`;
}

function requiredAssignment(output: string, name: string): string {
  const line = output.split(/\r?\n/).find(value => value.startsWith(`${name}=`));
  if (!line) throw new Error(`Missing ${name} in output:\n${output}`);
  return line.slice(name.length + 1).trim();
}

function renderMarkdownReport(generatedAt: string, lockHash: string): string {
  const lines = [
    '# Live browser verification',
    '',
    `- Generated: ${generatedAt}`,
    `- OS: ${process.platform}-${process.arch}`,
    `- Cargo.lock SHA-256: \`${lockHash}\``,
    `- Test CA SHA-256: \`${caSha256}\``,
    '- Chromium trust: durable current-user test CA; no certificate bypass',
    `- CA trust verified before run: ${caTrustVerifiedBeforeRun}`,
    `- CA trust verified after run: ${caTrustVerifiedAfterRun}`,
    '- CA lifecycle: durable root retained by explicit user choice; run `pwsh ./scripts/remove-live-test-ca.ps1` for exact-thumbprint teardown',
    '- Chromium QUIC: disabled; service workers: blocked; fresh profile: yes',
    '- Playwright route interception: not used',
    '',
    '| Case | Completed | Browser | Proxy | Target | Ingress/ALPN | Egress/ALPN | Adapter | Trust generation | Breakpoint events | Proof |',
    '|---|---|---|---|---|---|---|---|---:|---|---|',
  ];
  for (const result of results) {
    const evidence = result.upgradeEvidence ?? result.evidence;
    lines.push([
      `| ${result.name}`,
      result.completedAt,
      result.browserVersion,
      result.proxyAddress,
      result.url,
      evidence ? `${evidence.ingress}/${evidence.ingress_alpn}` : '-',
      evidence ? `${evidence.egress}/${evidence.egress_alpn}` : '-',
      evidence?.adapter ?? '-',
      evidence?.trust_generation ?? '-',
      evidence ? `${evidence.request_event_id}, ${evidence.response_event_id}` : '-',
      result.directFallbackBlocked ? 'DIRECT blocked' : 'header + DOM',
    ].join(' | ') + ' |');
    if (result.warmupEvidence) {
      lines.push(
        `| ${result.name} warmup | ${result.completedAt} | ${result.browserVersion} | ${result.proxyAddress} | ${result.warmupUrl} | ${result.warmupEvidence.ingress}/${result.warmupEvidence.ingress_alpn} | ${result.warmupEvidence.egress}/${result.warmupEvidence.egress_alpn} | ${result.warmupEvidence.adapter} | ${result.warmupEvidence.trust_generation} | ${result.warmupEvidence.request_event_id}, ${result.warmupEvidence.response_event_id} | Alt-Svc stripped |`,
      );
    }
  }
  return `${lines.join('\n')}\n`;
}

async function startProxy(route: Route, proofId: string) {
  const child = spawn(binary, [
    'serve',
    '--ca-cert', caCertificate,
    '--ca-key', caPrivateKey,
    '--listen', '127.0.0.1:0',
    '--route', route,
    '--proof-id', proofId,
  ], {
    cwd: repo,
    env: { ...process.env, RUST_LOG: 'rustymiddle=debug' },
    stdio: ['ignore', 'pipe', 'pipe'],
  }) as ChildProcessWithoutNullStreams;
  const lines: string[] = [];
  const evidence: Evidence[] = [];
  let listenAddress = '';
  const waiters: Array<{ predicate: (item: Evidence) => boolean; resolve: (item: Evidence) => void }> = [];
  const consume = (line: string) => {
    lines.push(line);
    if (line.startsWith('LISTEN_ADDR=')) listenAddress = line.slice('LISTEN_ADDR='.length);
    if (line.startsWith('EVIDENCE ')) {
      const item = Object.fromEntries(
        line.slice('EVIDENCE '.length).split(' ').map(part => part.split('=', 2)),
      );
      evidence.push(item);
      for (const waiter of waiters.splice(0)) {
        if (waiter.predicate(item)) waiter.resolve(item);
        else waiters.push(waiter);
      }
    }
  };
  readline.createInterface({ input: child.stdout }).on('line', consume);
  readline.createInterface({ input: child.stderr }).on('line', line => lines.push(line));
  await waitUntil(() => Boolean(listenAddress), 30_000, () => {
    if (child.exitCode !== null) throw new Error(`proxy exited ${child.exitCode}:\n${lines.join('\n')}`);
  });
  return {
    child,
    lines,
    get listenAddress() { return listenAddress; },
    waitForEvidence(predicate: (item: Evidence) => boolean): Promise<Evidence> {
      const current = evidence.find(predicate);
      if (current) return Promise.resolve(current);
      return Promise.race([
        new Promise<Evidence>(resolve => waiters.push({ predicate, resolve })),
        new Promise<Evidence>((_, reject) => setTimeout(
          () => reject(new Error(`missing proxy evidence:\n${lines.join('\n')}`)),
          30_000,
        )),
      ]);
    },
    async stop() {
      if (child.exitCode !== null) return;
      child.kill();
      await new Promise<void>(resolve => child.once('exit', () => resolve()));
    },
  };
}

async function waitUntil(predicate: () => boolean, timeoutMs: number, tick: () => void) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    tick();
    if (Date.now() >= deadline) throw new Error('timed out waiting for proxy startup');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}
