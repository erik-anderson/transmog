import process from 'node:process';

const portArgument = process.argv.findIndex((value) => value === '--port');
const port = portArgument >= 0 ? process.argv[portArgument + 1] : '9333';
if (port === undefined || !/^\d{1,5}$/.test(port)) {
  throw new Error('usage: npm run smoke:webview -- --port <loopback DevTools port>');
}

const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => {
  if (!response.ok) {
    throw new Error(`DevTools discovery returned ${response.status}`);
  }
  return response.json();
});
const candidates = targets.filter((target) =>
  target.type === 'page' && target.title === 'rustymiddle delivery spike'
);
if (candidates.length !== 1) {
  throw new Error(`expected one rustymiddle page, found ${candidates.length}`);
}

const socket = new WebSocket(candidates[0].webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});

let nextId = 1;
const pending = new Map();
const eventWaiters = new Map();
const browserErrors = [];

socket.addEventListener('message', ({ data }) => {
  const message = JSON.parse(String(data));
  if (message.id !== undefined) {
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (message.error !== undefined) {
      request?.reject(new Error(`${request.method}: ${message.error.message}`));
    } else {
      request?.resolve(message.result);
    }
    return;
  }

  if (message.method === 'Runtime.exceptionThrown') {
    browserErrors.push(message.params.exceptionDetails.text);
  }
  if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
    browserErrors.push({
      text: message.params.entry.text,
      url: message.params.entry.url,
      source: message.params.entry.source
    });
  }
  const waiters = eventWaiters.get(message.method) ?? [];
  eventWaiters.delete(message.method);
  for (const waiter of waiters) {
    waiter.resolve(message.params);
  }
});

try {
  await Promise.all([call('Page.enable'), call('Runtime.enable'), call('Log.enable')]);
  await call('Page.addScriptToEvaluateOnNewDocument', {
    source: `
      globalThis.__rustymiddleCspViolations = [];
      addEventListener('securitypolicyviolation', (event) => {
        globalThis.__rustymiddleCspViolations.push({
          blockedURI: event.blockedURI,
          directive: event.effectiveDirective
        });
      });
    `
  });

  const loaded = waitForEvent('Page.loadEventFired', 10_000);
  await call('Page.reload', { ignoreCache: true });
  await loaded;

  const result = await evaluate(`
    (async () => {
      await customElements.whenDefined('phase-zero-probe');
      const element = document.querySelector('phase-zero-probe');
      const button = element?.shadowRoot?.querySelector('button');
      const output = element?.shadowRoot?.querySelector('output');
      if (!(button instanceof HTMLButtonElement) || !(output instanceof HTMLOutputElement)) {
        throw new Error('hydrated WebUI probe controls were not found');
      }
      button.click();
      const deadline = performance.now() + 10_000;
      while (output.dataset.status !== 'passed' && output.dataset.status !== 'failed') {
        if (performance.now() >= deadline) {
          throw new Error('probe timed out');
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      return {
        status: output.dataset.status,
        text: output.textContent,
        title: document.title,
        url: location.href,
        readyState: document.readyState,
        resources: performance.getEntriesByType('resource').map((entry) => entry.name),
        cspViolations: globalThis.__rustymiddleCspViolations
      };
    })()
  `);

  assert(result.status === 'passed', result.text ?? 'probe failed without a message');
  assert(result.text.includes('fetch=custom-protocol'), 'same-origin custom-protocol fetch did not pass');
  assert(result.text.includes('command=WebView2'), 'typed Tauri command did not pass');
  assert(result.text.includes('hint=1'), 'bounded Tauri channel notification did not pass');
  assert(result.url === 'http://rustymiddle-ui.localhost/', `unexpected application origin: ${result.url}`);
  assert(result.resources.some((url) => url.endsWith('/app.js')), 'module asset was not loaded');
  assert(result.resources.some((url) => url.endsWith('.css')), 'WebUI CSS asset was not loaded');
  assert(result.cspViolations.length === 0, `CSP violations: ${JSON.stringify(result.cspViolations)}`);
  assert(browserErrors.length === 0, `browser errors: ${JSON.stringify(browserErrors)}`);

  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
} finally {
  socket.close();
}

function call(method, params = {}) {
  const id = nextId;
  nextId += 1;
  return new Promise((resolve, reject) => {
    pending.set(id, { method, resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

function waitForEvent(method, timeoutMilliseconds) {
  return new Promise((resolve, reject) => {
    const waiter = { resolve, reject };
    const waiters = eventWaiters.get(method) ?? [];
    waiters.push(waiter);
    eventWaiters.set(method, waiters);
    setTimeout(() => {
      const current = eventWaiters.get(method) ?? [];
      const index = current.indexOf(waiter);
      if (index >= 0) {
        current.splice(index, 1);
        reject(new Error(`timed out waiting for ${method}`));
      }
    }, timeoutMilliseconds);
  });
}

async function evaluate(expression) {
  const response = await call('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true
  });
  if (response.exceptionDetails !== undefined) {
    throw new Error(response.exceptionDetails.exception?.description ?? response.exceptionDetails.text);
  }
  return response.result.value;
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}
