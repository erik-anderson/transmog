import { test, expect, chromium, type Browser } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { startProxy, type Evidence } from '../support/proxy';

type Origin = {
  name: string;
  url: string;
  marker: string;
  serverHeader: RegExp;
  serverValue: RegExp;
};

const enabled = process.env.RUSTYMIDDLE_INTEROP === '1';
const windows = process.platform === 'win32';
const repo = path.resolve(__dirname, '..', '..', '..');
const binary = requiredEnvironment('RUSTYMIDDLE_BIN');
const caCertificate = requiredEnvironment('RUSTYMIDDLE_TEST_CA_CERT');
const caPrivateKey = requiredEnvironment('RUSTYMIDDLE_TEST_CA_KEY');
const curl = process.env.RUSTYMIDDLE_CURL ?? (windows ? 'curl.exe' : 'curl');
const origins: Origin[] = [
  {
    name: 'Nginx',
    url: requiredEnvironment('RUSTYMIDDLE_NGINX_URL'),
    marker: 'nginx',
    serverHeader: /^server:\s*nginx\/1\.28\.0\s*$/im,
    serverValue: /^nginx\/1\.28\.0$/i,
  },
  {
    name: 'Apache HTTP Server',
    url: requiredEnvironment('RUSTYMIDDLE_APACHE_URL'),
    marker: 'apache',
    serverHeader: /^server:\s*Apache\/2\.4\.65(?:\s+\([^)]*\))?\s*$/im,
    serverValue: /^Apache\/2\.4\.65(?:\s+\([^)]*\))?$/i,
  },
];

test.skip(!enabled, 'Run through scripts/test-interop.ps1 with the container origins enabled.');
test.describe.configure({ mode: 'serial' });

for (const origin of origins) {
  test(`curl interoperates with ${origin.name} through the proxy`, async () => {
    const proofId = `curl-${origin.marker}-${randomUUID()}`;
    const proxy = await startProxy({
      binary,
      repo,
      caCertificate,
      caPrivateKey,
      route: 'h1',
      proofId,
    });
    try {
      const response = runCurl(proxy.listenAddress, origin.url);
      expect(response.headers).toMatch(/^HTTP\/1\.1 200 OK\s*$/im);
      expect(response.headers).toMatch(origin.serverHeader);
      expect(response.headers).toMatch(
        new RegExp(`^x-intercepted-by:\\s*${escapeRegularExpression(proofId)}\\s*$`, 'im'),
      );
      expect(response.body).toContain(`<meta name="intercept-proxy-proof" content="${proofId}">`);
      expect(response.body).toContain(`data-origin="${origin.marker}"`);
      assertEvidence(await proxy.waitForEvidence(matchesOrigin(origin.url)), origin.url);
    } finally {
      await proxy.stop();
    }
  });

  test(`Chromium interoperates with ${origin.name} through the proxy`, async () => {
    const proofId = `chromium-${origin.marker}-${randomUUID()}`;
    const proxy = await startProxy({
      binary,
      repo,
      caCertificate,
      caPrivateKey,
      route: 'h1',
      proofId,
    });
    let browser: Browser | undefined;
    try {
      browser = await chromium.launch({
        proxy: { server: `http://${proxy.listenAddress}` },
        args: [
          '--disable-quic',
          '--disable-features=UseDnsHttpsSvcbAlpn',
          // Chromium otherwise implicitly bypasses proxies for loopback URLs.
          '--proxy-bypass-list=<-loopback>',
        ],
      });
      const context = await browser.newContext({
        ignoreHTTPSErrors: false,
        serviceWorkers: 'block',
      });
      const page = await context.newPage();
      const response = await page.goto(origin.url, { waitUntil: 'domcontentloaded' });
      expect(response).not.toBeNull();
      expect(response!.status()).toBe(200);
      expect(response!.headers()['x-intercepted-by']).toBe(proofId);
      expect(response!.headers()['server']).toMatch(origin.serverValue);
      await expect(page.locator(
        `meta[name="intercept-proxy-proof"][content="${proofId}"]`,
      )).toHaveCount(1);
      await expect(page.locator(`[data-origin="${origin.marker}"]`)).toHaveCount(1);
      assertEvidence(await proxy.waitForEvidence(matchesOrigin(origin.url)), origin.url);
      await context.close();
    } finally {
      await browser?.close();
      await proxy.stop();
    }
  });
}

function runCurl(proxyAddress: string, url: string): { headers: string; body: string } {
  const result = spawnSync(curl, [
    '--silent',
    '--show-error',
    '--fail-with-body',
    '--http1.1',
    '--connect-timeout', '10',
    '--max-time', '30',
    '--noproxy', '',
    '--proxy', `http://${proxyAddress}`,
    '--dump-header', '-',
    url,
  ], {
    cwd: repo,
    encoding: 'utf8',
    timeout: 35_000,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `curl failed (${result.status}): ${result.error ?? ''}\n${result.stdout}\n${result.stderr}`,
    );
  }
  const output = result.stdout;
  const boundary = output.search(/\r?\n\r?\n/);
  if (boundary < 0) throw new Error(`curl response has no header boundary:\n${output}`);
  const match = output.slice(boundary).match(/^\r?\n\r?\n/);
  if (!match) throw new Error(`curl response has an invalid header boundary:\n${output}`);
  return {
    headers: output.slice(0, boundary),
    body: output.slice(boundary + match[0].length),
  };
}

function matchesOrigin(url: string): (item: Evidence) => boolean {
  const parsed = new URL(url);
  return item => item.scheme === 'http'
    && item.host === parsed.hostname
    && item.path === parsed.pathname;
}

function assertEvidence(evidence: Evidence, url: string) {
  expect(evidence.ingress).toBe('Http1');
  expect(evidence.egress).toBe('Http1');
  expect(evidence.adapter).toBe('hyper');
  expect(evidence.request_breakpoint).toBe('true');
  expect(evidence.response_breakpoint).toBe('true');
  expect(evidence.upstream_verification).toBe('not-applicable');
  expect(evidence.host).toBe(new URL(url).hostname);
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  // Module initialization still happens for skipped tests; placeholders keep a
  // normal `npm test` discovery run inert unless the suite was explicitly enabled.
  if (!value && !enabled) return `disabled-${name.toLowerCase()}`;
  if (!value) throw new Error(`Missing required ${name}. Run scripts/test-interop.ps1.`);
  return value;
}

function escapeRegularExpression(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
