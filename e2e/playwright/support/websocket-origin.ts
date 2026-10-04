import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import path from 'node:path';
import readline from 'node:readline';

export type WebSocketOrigin = {
  readonly url: string;
  lines: string[];
  waitForEcho(value: string): Promise<void>;
  stop(): Promise<void>;
};

export async function startWebSocketOrigin(repo: string): Promise<WebSocketOrigin> {
  const child = spawn(process.execPath, [
    path.join(repo, 'e2e', 'interop', 'websocket-echo.mjs'),
  ], {
    cwd: repo,
    stdio: ['ignore', 'pipe', 'pipe'],
  }) as ChildProcessWithoutNullStreams;
  const lines: string[] = [];
  let listenAddress = '';
  let processError: Error | undefined;
  const echoWaiters = new Map<string, () => void>();
  const consume = (line: string) => {
    lines.push(line);
    if (line.startsWith('LISTEN_ADDR=')) listenAddress = line.slice('LISTEN_ADDR='.length);
    if (line.startsWith('ECHO=')) echoWaiters.get(line.slice('ECHO='.length))?.();
  };
  readline.createInterface({ input: child.stdout }).on('line', consume);
  readline.createInterface({ input: child.stderr }).on('line', line => lines.push(line));
  child.once('error', error => { processError = error; });
  try {
    await waitUntil(() => Boolean(listenAddress), () => {
      if (processError) throw processError;
      if (child.exitCode !== null) throw new Error(`echo origin exited:\n${lines.join('\n')}`);
    });
  } catch (error) {
    await stopChild(child);
    throw error;
  }
  return {
    lines,
    get url() { return `ws://${listenAddress}/socket`; },
    waitForEcho(value: string) {
      if (lines.includes(`ECHO=${value}`)) return Promise.resolve();
      return new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => {
          echoWaiters.delete(value);
          reject(new Error(`echo origin did not receive ${value}:\n${lines.join('\n')}`));
        }, 10_000);
        echoWaiters.set(value, () => {
          clearTimeout(timer);
          echoWaiters.delete(value);
          resolve();
        });
      });
    },
    async stop() { await stopChild(child); },
  };
}

async function waitUntil(predicate: () => boolean, tick: () => void) {
  const deadline = Date.now() + 10_000;
  while (!predicate()) {
    tick();
    if (Date.now() >= deadline) throw new Error('timed out waiting for echo origin');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
}

async function stopChild(child: ChildProcessWithoutNullStreams) {
  if (child.exitCode !== null) return;
  const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
  child.kill();
  await exited;
}
