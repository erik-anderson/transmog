import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import readline from 'node:readline';

export type Route = 'h1' | 'h2' | 'h3' | 'auto';
export type Evidence = Record<string, string>;

export type ProxyOptions = {
  binary: string;
  repo: string;
  caCertificate: string;
  caPrivateKey: string;
  upstreamCaCertificate?: string;
  route: Route;
  proofId: string;
};

export type ProxyHandle = {
  child: ChildProcessWithoutNullStreams;
  lines: string[];
  readonly listenAddress: string;
  waitForEvidence(predicate: (item: Evidence) => boolean): Promise<Evidence>;
  waitForWebSocketEvidence(predicate: (item: Evidence) => boolean): Promise<Evidence>;
  stop(): Promise<void>;
};

/** Starts the real CLI proxy and exposes its machine-readable evidence stream. */
export async function startProxy(options: ProxyOptions): Promise<ProxyHandle> {
  const arguments_ = [
    'serve',
    '--ca-cert', options.caCertificate,
    '--ca-key', options.caPrivateKey,
    '--listen', '127.0.0.1:0',
    '--route', options.route,
    '--proof-id', options.proofId,
  ];
  if (options.upstreamCaCertificate) {
    arguments_.push('--upstream-ca-cert', options.upstreamCaCertificate);
  }
  const child = spawn(options.binary, arguments_, {
    cwd: options.repo,
    env: { ...process.env, RUST_LOG: 'rustymiddle=debug' },
    stdio: ['ignore', 'pipe', 'pipe'],
  }) as ChildProcessWithoutNullStreams;
  const lines: string[] = [];
  const evidence: Evidence[] = [];
  const websocketEvidence: Evidence[] = [];
  let listenAddress = '';
  let spawnError: Error | undefined;
  const waiters: Array<{
    predicate: (item: Evidence) => boolean;
    resolve: (item: Evidence) => void;
  }> = [];
  const websocketWaiters: Array<{
    predicate: (item: Evidence) => boolean;
    resolve: (item: Evidence) => void;
  }> = [];
  const consume = (line: string) => {
    lines.push(line);
    if (line.startsWith('LISTEN_ADDR=')) listenAddress = line.slice('LISTEN_ADDR='.length);
    if (line.startsWith('EVIDENCE ')) {
      const item = parseAssignments(line.slice('EVIDENCE '.length));
      evidence.push(item);
      for (const waiter of waiters.splice(0)) {
        if (waiter.predicate(item)) waiter.resolve(item);
        else waiters.push(waiter);
      }
    }
    if (line.startsWith('WS_EVIDENCE ')) {
      const item = parseAssignments(line.slice('WS_EVIDENCE '.length));
      websocketEvidence.push(item);
      for (const waiter of websocketWaiters.splice(0)) {
        if (waiter.predicate(item)) waiter.resolve(item);
        else websocketWaiters.push(waiter);
      }
    }
  };
  readline.createInterface({ input: child.stdout }).on('line', consume);
  readline.createInterface({ input: child.stderr }).on('line', line => lines.push(line));
  child.once('error', error => {
    spawnError = error;
    lines.push(`proxy process error: ${error.message}`);
  });
  try {
    await waitUntil(() => Boolean(listenAddress), 30_000, () => {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null) {
        throw new Error(`proxy exited ${child.exitCode}:\n${lines.join('\n')}`);
      }
    });
  } catch (error) {
    await stopChild(child);
    throw error;
  }
  return {
    child,
    lines,
    get listenAddress() { return listenAddress; },
    waitForEvidence(predicate: (item: Evidence) => boolean): Promise<Evidence> {
      return waitForItem(evidence, waiters, predicate, lines, 'proxy');
    },
    waitForWebSocketEvidence(predicate: (item: Evidence) => boolean): Promise<Evidence> {
      return waitForItem(
        websocketEvidence,
        websocketWaiters,
        predicate,
        lines,
        'WebSocket',
      );
    },
    async stop() {
      await stopChild(child);
    },
  };
}

function waitForItem(
  existing: Evidence[],
  waiters: Array<{
    predicate: (item: Evidence) => boolean;
    resolve: (item: Evidence) => void;
  }>,
  predicate: (item: Evidence) => boolean,
  lines: string[],
  label: string,
): Promise<Evidence> {
  const current = existing.find(predicate);
  if (current) return Promise.resolve(current);
  return new Promise<Evidence>((resolve, reject) => {
    const waiter = {
      predicate,
      resolve: (item: Evidence) => {
        clearTimeout(timer);
        resolve(item);
      },
    };
    const timer = setTimeout(() => {
      const index = waiters.indexOf(waiter);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error(`missing ${label} evidence:\n${lines.join('\n')}`));
    }, 30_000);
    waiters.push(waiter);
  });
}

function parseAssignments(value: string): Evidence {
  return Object.fromEntries(value.split(' ').map(part => {
    const separator = part.indexOf('=');
    return separator < 0
      ? [part, '']
      : [part.slice(0, separator), part.slice(separator + 1)];
  }));
}

async function stopChild(child: ChildProcessWithoutNullStreams) {
  if (child.exitCode !== null) return;
  const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
  child.kill();
  await exited;
}

async function waitUntil(predicate: () => boolean, timeoutMs: number, tick: () => void) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    tick();
    if (Date.now() >= deadline) throw new Error('timed out waiting for proxy startup');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}
