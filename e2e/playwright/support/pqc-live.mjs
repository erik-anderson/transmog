// Opt-in real proxy/browser validation; no certificate-error or routing bypasses.
import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile, mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { chromium } from "playwright";

const [binary, artifacts, url] = process.argv.slice(2);
assert.equal(process.platform, "linux", "Live validation uses isolated Linux NSS trust");
assert.ok(
  binary && artifacts && url,
  "Expected proxy executable, artifacts directory and HTTPS URL",
);
const destination = new URL(url);
assert.equal(destination.protocol, "https:");
assert.ok(!destination.username && !destination.password, "Use a public URL without credentials");
const run = await mkdtemp(join(artifacts, "live-"));
let browser, proxy, stopped;
let output = "",
  errors = "";
let spawnError;
try {
  const certificate = join(run, "root.pem"),
    key = join(run, "root.key");
  execFileSync(
    binary,
    ["ca", "generate", "--cert", certificate, "--key", key, "--name", "PQC live validation"],
    { timeout: 30_000 },
  );
  const database = join(run, ".pki", "nssdb");
  await mkdir(database, { recursive: true });
  execFileSync("certutil", ["-d", `sql:${database}`, "-N", "--empty-password"]);
  execFileSync("certutil", [
    "-d",
    `sql:${database}`,
    "-A",
    "-t",
    "C,,",
    "-n",
    "PQC live fixture root",
    "-i",
    certificate,
  ]);
  const capture = join(run, "capture.tmcap");
  proxy = spawn(
    binary,
    [
      "serve",
      "--ca-cert",
      certificate,
      "--ca-key",
      key,
      "--listen",
      "127.0.0.1:0",
      "--route",
      "h2",
      "--capture",
      capture,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  stopped = new Promise((resolve, reject) => {
    proxy.once("error", (error) => {
      spawnError = error;
      reject(error);
    });
    proxy.once("exit", (code, signal) => resolve({ code, signal }));
  });
  stopped.catch(() => {});
  proxy.stdout.on("data", (bytes) => (output += bytes.toString()));
  proxy.stderr.on("data", (bytes) => (errors += bytes.toString()));
  const deadline = Date.now() + 30_000;
  while (!output.includes("LISTEN_ADDR=")) {
    if (spawnError) throw spawnError;
    if (proxy.exitCode !== null) throw new Error(`Proxy exited: ${errors}`);
    assert.ok(Date.now() < deadline, `Proxy startup timed out: ${errors}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const address = output.match(/^LISTEN_ADDR=(127\.0\.0\.1:\d+)$/m)?.[1];
  assert.ok(address, "Proxy did not report a loopback listener");
  browser = await chromium.launch({
    channel: "chrome",
    env: { ...process.env, HOME: run },
    proxy: { server: `http://${address}` },
    args: ["--disable-background-networking", "--disable-quic"],
  });
  const context = await browser.newContext({ ignoreHTTPSErrors: false, serviceWorkers: "block" });
  const page = await context.newPage();
  const response = await page.goto(destination.href, {
    timeout: 30_000,
    waitUntil: "domcontentloaded",
  });
  assert.equal(response.status(), 200);
  assert.equal(
    new URL(response.url()).origin,
    destination.origin,
    "The live probe redirected to another origin",
  );
  const browserVersion = browser.version();
  await browser.close();
  browser = undefined;
  proxy.kill("SIGINT");
  const stopDeadline = Date.now() + 15_000;
  while (proxy.exitCode === null && proxy.signalCode === null) {
    assert.ok(Date.now() < stopDeadline, "Proxy failed to seal its capture on shutdown");
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const exit = await stopped;
  assert.equal(exit.code, 0, `Proxy shutdown failed: ${errors}`);
  const exported = execFileSync(
    binary,
    ["capture", "export", "--input", capture, "--format", "jsonl"],
    { timeout: 30_000, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  );
  const transports = exported
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line))
    .filter((record) => record.kind === "performance")
    .flatMap((record) => record.payload.transports);
  const connections = new Map();
  for (const transport of transports) {
    if (
      transport.tls?.serverName === destination.hostname &&
      transport.tls.details["Negotiated group"]
    ) {
      connections.set(`${transport.leg}:${transport.connectionId}`, transport);
    }
  }
  const legs = [...connections.values()];
  const upstream = legs.filter((leg) => leg.leg === "upstream");
  const client = legs.filter((leg) => leg.leg === "client");
  assert.ok(upstream.length && client.length, "Missing recorded TLS facts for either proxy leg");
  for (const leg of [...upstream, ...client]) {
    assert.equal(leg.tls.details["Negotiated group"], "X25519MLKEM768");
    assert.equal(leg.tls.details["Negotiated group ID"], "0x11ec");
  }
  for (const leg of upstream) assert.equal(leg.tls.verification, "Verified");
  const report = {
    url: destination.href,
    browser: browserVersion,
    certificateBypassEnabled: false,
    route: "h2",
    status: response.status(),
    connections: legs.map(({ leg, connectionId, tls }) => ({
      leg,
      connectionId,
      verification: tls.verification,
      facts: tls.details,
    })),
  };
  await writeFile(join(artifacts, "live-results.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report));
} finally {
  try {
    await browser?.close();
  } finally {
    if (proxy && proxy.exitCode === null && proxy.signalCode === null) proxy.kill("SIGKILL");
    await stopped?.catch(() => {});
    await rm(run, { recursive: true, force: true });
  }
}
