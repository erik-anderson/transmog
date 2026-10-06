import { writeFile } from 'node:fs/promises';
import process from 'node:process';

const portArgument = process.argv.findIndex((value) => value === '--port');
const port = portArgument >= 0 ? process.argv[portArgument + 1] : '9333';
const soakArgument = process.argv.findIndex((value) => value === '--soak-minutes');
const soakMinutes = soakArgument >= 0 ? Number(process.argv[soakArgument + 1]) : 0;
const screenshotArgument = process.argv.findIndex((value) => value === '--screenshot');
const screenshotPath = screenshotArgument >= 0 ? process.argv[screenshotArgument + 1] : undefined;
const automationScreenshotArgument = process.argv.findIndex((value) => value === '--automation-screenshot');
const automationScreenshotPath = automationScreenshotArgument >= 0
  ? process.argv[automationScreenshotArgument + 1]
  : undefined;
if (port === undefined || !/^\d{1,5}$/.test(port)) {
  throw new Error('usage: npm run smoke:webview -- --port <loopback DevTools port>');
}
if (!Number.isFinite(soakMinutes) || soakMinutes < 0 || soakMinutes > 240) {
  throw new Error('--soak-minutes must be between 0 and 240');
}

const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => {
  if (!response.ok) {
    throw new Error(`DevTools discovery returned ${response.status}`);
  }
  return response.json();
});
const candidates = targets.filter((target) =>
  target.type === 'page' && target.title === 'Transmog'
);
if (candidates.length !== 1) {
  throw new Error(`expected one Transmog page, found ${candidates.length}`);
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
  await Promise.all([
    call('Page.enable'), call('Runtime.enable'), call('Log.enable'),
    call('Accessibility.enable'), call('Performance.enable')
  ]);
  await call('Page.addScriptToEvaluateOnNewDocument', {
    source: `
      globalThis.__transmogCspViolations = [];
      addEventListener('securitypolicyviolation', (event) => {
        globalThis.__transmogCspViolations.push({
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
      await customElements.whenDefined('transmog-app-shell');
      const element = document.querySelector('transmog-app-shell');
      const button = [...(element?.shadowRoot?.querySelectorAll('button') ?? [])]
        .find((candidate) => candidate.textContent?.trim() === 'Refresh status');
      const output = element?.shadowRoot?.querySelector('.global-diagnostics');
      if (!(button instanceof HTMLButtonElement) || !(output instanceof HTMLOutputElement)) {
        throw new Error('hydrated application controls were not found');
      }
      button.click();
      const deadline = performance.now() + 10_000;
      while (output.textContent === 'Application facade ready.') {
        if (performance.now() >= deadline) {
          throw new Error('status command timed out');
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      const statusText = output.textContent;
      const filter = element.shadowRoot.querySelector('form.filters');
      const sessionOutput = element.shadowRoot.querySelector('.session-status');
      if (!(filter instanceof HTMLFormElement) || !(sessionOutput instanceof HTMLOutputElement)) {
        throw new Error('session submission controls were not found');
      }
      const initializationDeadline = performance.now() + 10_000;
      while (!sessionOutput.textContent?.startsWith('Watching live traffic.')) {
        if (performance.now() >= initializationDeadline) {
          throw new Error('automatic live-watch initialization timed out with: ' + sessionOutput.textContent);
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      sessionOutput.textContent = 'Submitting session query…';
      filter.requestSubmit();
      const submissionDeadline = performance.now() + 10_000;
      while (!sessionOutput.textContent?.startsWith('Loaded ')) {
        if (performance.now() >= submissionDeadline) {
          throw new Error('WebUI form submission timed out with: ' + sessionOutput.textContent);
        }
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      const settingsLink = element.shadowRoot.querySelector('a[data-view="settings"]');
      const trafficLink = element.shadowRoot.querySelector('a[data-view="traffic"]');
      if (!(settingsLink instanceof HTMLAnchorElement) || !(trafficLink instanceof HTMLAnchorElement)) {
        throw new Error('application navigation was not found');
      }
      settingsLink.click();
      const settingsVisible = !element.shadowRoot.querySelector('#settings').hidden
        && element.shadowRoot.querySelector('#traffic').hidden;
      trafficLink.click();
      const rootLayout = {
        documentClientHeight: document.documentElement.clientHeight,
        documentScrollHeight: document.documentElement.scrollHeight,
        bodyClientHeight: document.body.clientHeight,
        bodyScrollHeight: document.body.scrollHeight,
        hostOverflow: getComputedStyle(element).overflow,
        shellHeight: element.shadowRoot.querySelector('.shell').getBoundingClientRect().height,
        activeView: element.shadowRoot.querySelector('.app-view.active')?.id,
        settingsVisible
      };
      const surfaceText = element.shadowRoot.textContent ?? '';
      const automation = element.shadowRoot.querySelector('#automation');
      const scratchButton = [...(automation?.querySelectorAll('button') ?? [])]
        .find((candidate) => candidate.textContent?.trim() === 'Create from scratch');
      scratchButton?.click();
      const autoResponseEditor = automation?.querySelector('.auto-response-editor');
      const autoResponseMethod = autoResponseEditor?.querySelector('select[name="method"]');
      const requestHeaders = autoResponseEditor?.querySelector('textarea[name="requestHeaders"]')?.closest('label');
      const scratchEditorOpened = autoResponseEditor instanceof HTMLFormElement && !autoResponseEditor.hidden;
      if (autoResponseMethod instanceof HTMLSelectElement) {
        autoResponseMethod.value = 'POST';
        autoResponseMethod.dispatchEvent(new Event('change', { bubbles: true }));
      }
      const postHeaderFilterVisible = requestHeaders instanceof HTMLElement && !requestHeaders.hidden;
      const cancelEditor = [...(autoResponseEditor?.querySelectorAll('button') ?? [])]
        .find((candidate) => candidate.textContent?.trim() === 'Cancel');
      cancelEditor?.click();
      trafficLink.click();
      return {
        text: statusText,
        formSubmission: sessionOutput.textContent,
        title: document.title,
        url: location.href,
        readyState: document.readyState,
        resources: performance.getEntriesByType('resource').map((entry) => entry.name),
        cspViolations: globalThis.__transmogCspViolations,
        landmarks: {
          nav: element.shadowRoot.querySelectorAll('nav').length,
          main: element.shadowRoot.querySelectorAll('main').length,
          headings: element.shadowRoot.querySelectorAll('h1, h2, h3').length
        },
        unnamedControls: [...element.shadowRoot.querySelectorAll('button, input, select, textarea, a[href]')]
          .filter((control) => control.getAttribute('aria-hidden') !== 'true')
          .filter((control) => {
            const labelledBy = control.getAttribute('aria-labelledby');
            const labelledText = labelledBy === null ? '' : element.shadowRoot.getElementById(labelledBy)?.textContent;
            const labelText = control.closest('label')?.textContent;
            return ![control.getAttribute('aria-label'), labelledText, labelText, control.textContent, control.getAttribute('title')]
              .some((value) => value?.trim());
          })
          .map((control) => ({ tag: control.tagName, className: control.className, outerHTML: control.outerHTML.slice(0, 300) })),
        rootLayout,
        ux: {
          paginationControls: [...element.shadowRoot.querySelectorAll('button')]
            .filter((candidate) => /next page/i.test(candidate.textContent ?? '')).length,
          watchControls: [...element.shadowRoot.querySelectorAll('button')]
            .filter((candidate) => /watch live/i.test(candidate.textContent ?? '')).length,
          hooksV2Branding: /hooks v2/i.test(surfaceText),
          automationExpanders: automation?.querySelectorAll('details.automation-card').length ?? 0,
          autoResponseWorkspace: automation?.querySelectorAll('.auto-response-workspace').length ?? 0,
          autoResponseDropZone: automation?.querySelectorAll('.auto-response-drop-zone').length ?? 0,
          exactUrlFields: automation?.querySelectorAll('input[name="url"]').length ?? 0,
          firstMatchExplained: /first enabled match wins/i.test(automation?.textContent ?? ''),
          scratchEditorOpened,
          postHeaderFilterVisible,
          scratchEditorClosed: autoResponseEditor instanceof HTMLFormElement && autoResponseEditor.hidden,
          internalAutomationFields: automation?.querySelectorAll('input[name="ruleId"], input[name="revision"], input[name="assetId"], input[name="assetRevision"]').length ?? 0,
          decodeSelected: element.shadowRoot.querySelector('.body-toolbar input[type="checkbox"]')?.checked ?? false,
          noticeAvailable: element.shadowRoot.querySelector('.notice') instanceof HTMLElement,
          sessionScrollerAvailable: element.shadowRoot.querySelector('.table-wrap') instanceof HTMLElement,
          callerColumn: [...element.shadowRoot.querySelectorAll('th')]
            .some((heading) => heading.textContent?.trim() === 'Caller'),
          brandIconLoaded: (() => {
            const icon = element.shadowRoot.querySelector('.brand-mark');
            return icon instanceof HTMLImageElement
              && icon.getAttribute('src') === '/transmog-icon.svg'
              && icon.complete
              && icon.naturalWidth > 0;
          })()
        },
        themePreference: element.dataset.theme,
        startupMs: performance.getEntriesByType('navigation')[0]?.domContentLoadedEventEnd ?? 0
      };
    })()
  `);

  assert(result.text === 'Proxy stopped', `typed status command failed: ${result.text}`);
  assert(result.formSubmission.startsWith('Loaded '), `WebUI event binding failed: ${result.formSubmission}`);
  assert(result.url === 'http://transmog-ui.localhost/', `unexpected application origin: ${result.url}`);
  assert(result.resources.some((url) => url.endsWith('/app.js')), 'module asset was not loaded');
  assert(result.resources.some((url) => url.endsWith('.css')), 'WebUI CSS asset was not loaded');
  assert(result.resources.some((url) => url.endsWith('/transmog-icon.svg')), 'brand icon asset was not loaded');
  assert(result.cspViolations.length === 0, `CSP violations: ${JSON.stringify(result.cspViolations)}`);
  assert(result.landmarks.nav === 1 && result.landmarks.main === 1 && result.landmarks.headings >= 6,
    `semantic landmarks missing: ${JSON.stringify(result.landmarks)}`);
  assert(result.unnamedControls.length === 0, `interactive controls have no accessible name: ${JSON.stringify(result.unnamedControls)}`);
  assert(result.rootLayout.settingsVisible, `navigation did not switch bounded views: ${JSON.stringify(result.rootLayout)}`);
  assert(result.rootLayout.activeView === 'traffic', `traffic view did not restore: ${JSON.stringify(result.rootLayout)}`);
  assert(result.rootLayout.hostOverflow === 'hidden', `application host can scroll: ${JSON.stringify(result.rootLayout)}`);
  assert(result.rootLayout.documentScrollHeight <= result.rootLayout.documentClientHeight + 1,
    `document root can scroll: ${JSON.stringify(result.rootLayout)}`);
  assert(result.rootLayout.bodyScrollHeight <= result.rootLayout.bodyClientHeight + 1,
    `document body can scroll: ${JSON.stringify(result.rootLayout)}`);
  assert(result.rootLayout.shellHeight <= result.rootLayout.documentClientHeight + 1,
    `application shell exceeds the viewport: ${JSON.stringify(result.rootLayout)}`);
  assert(result.ux.paginationControls === 0 && result.ux.watchControls === 0,
    `traffic surface exposes manual paging/watch controls: ${JSON.stringify(result.ux)}`);
  assert(!result.ux.hooksV2Branding, `historical Hooks v2 branding is visible: ${JSON.stringify(result.ux)}`);
  assert(result.ux.automationExpanders >= 1 && result.ux.autoResponseWorkspace === 1
    && result.ux.autoResponseDropZone === 1 && result.ux.exactUrlFields === 1
    && result.ux.firstMatchExplained && result.ux.scratchEditorOpened
    && result.ux.postHeaderFilterVisible && result.ux.scratchEditorClosed
    && result.ux.internalAutomationFields === 0,
    `automation surface lacks the discoverable ordered auto-response flow or exposes internals: ${JSON.stringify(result.ux)}`);
  assert(result.ux.decodeSelected && result.ux.noticeAvailable && result.ux.sessionScrollerAvailable
    && result.ux.callerColumn && result.ux.brandIconLoaded,
    `expected inspection/setup affordances are missing: ${JSON.stringify(result.ux)}`);
  assert(result.startupMs < 10_000, `document startup exceeded 10 seconds: ${result.startupMs}`);

  if (screenshotPath !== undefined) {
    const screenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    await writeFile(screenshotPath, Buffer.from(screenshot.data, 'base64'));
    result.screenshot = screenshotPath;
  }
  if (automationScreenshotPath !== undefined) {
    const automationLayout = await evaluate(`(() => {
      const shell = document.querySelector('transmog-app-shell');
      shell.shadowRoot.querySelector('a[data-view="automation"]').click();
      const notice = shell.shadowRoot.querySelector('.notice');
      return {
        documentScrollTop: document.scrollingElement.scrollTop,
        noticeHidden: notice.hidden,
        noticeText: notice.textContent.trim(),
        topbarTop: shell.shadowRoot.querySelector('.topbar').getBoundingClientRect().top,
        topbarHeight: shell.shadowRoot.querySelector('.topbar').getBoundingClientRect().height
      };
    })()`);
    await new Promise((resolve) => setTimeout(resolve, 250));
    const screenshot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    await writeFile(automationScreenshotPath, Buffer.from(screenshot.data, 'base64'));
    result.automationScreenshot = automationScreenshotPath;
    result.automationLayout = automationLayout;
    await evaluate(`(() => document.querySelector('transmog-app-shell').shadowRoot.querySelector('a[data-view="traffic"]').click())()`);
  }

  const accessibility = await call('Accessibility.getFullAXTree');
  const unnamedAxControls = accessibility.nodes.filter((node) =>
    ['button', 'textbox', 'combobox', 'link', 'checkbox'].includes(node.role?.value)
      && !(node.name?.value ?? '').trim()
  );
  assert(unnamedAxControls.length === 0,
    `accessibility tree contains unnamed controls: ${JSON.stringify(unnamedAxControls)}`);

  await call('Emulation.setEmulatedMedia', {
    media: 'screen',
    features: [
      { name: 'forced-colors', value: 'active' },
      { name: 'prefers-reduced-motion', value: 'reduce' }
    ]
  });
  const contrast = await evaluate(`(() => {
    const root = document.querySelector('transmog-app-shell').shadowRoot;
    const panel = root.querySelector('.rail');
    const button = root.querySelector('button');
    return {
      panelBorder: getComputedStyle(panel).borderRightStyle,
      transition: getComputedStyle(button).transitionDuration
    };
  })()`);
  assert(contrast.panelBorder !== 'none', 'forced-colors removed panel boundaries');
  assert(contrast.transition === '0s', `reduced motion still animates: ${contrast.transition}`);

  await call('Emulation.setEmulatedMedia', {
    media: 'screen',
    features: [{ name: 'prefers-color-scheme', value: 'dark' }]
  });
  const darkCanvas = await evaluate(`(() => {
    const shell = document.querySelector('transmog-app-shell');
    shell.dataset.theme = 'system';
    return getComputedStyle(shell).getPropertyValue('--canvas').trim();
  })()`);
  await call('Emulation.setEmulatedMedia', {
    media: 'screen',
    features: [{ name: 'prefers-color-scheme', value: 'light' }]
  });
  const lightCanvas = await evaluate(`(() => getComputedStyle(document.querySelector('transmog-app-shell')).getPropertyValue('--canvas').trim())()`);
  assert(darkCanvas !== lightCanvas, `system color scheme did not change application palette: ${darkCanvas}`);
  result.systemTheme = { darkCanvas, lightCanvas };

  await call('Emulation.setDeviceMetricsOverride', {
    width: 760, height: 520, deviceScaleFactor: 2, mobile: false
  });
  const scaled = await evaluate(`(() => {
    const root = document.querySelector('transmog-app-shell').shadowRoot;
    const heading = root.querySelector('h1');
    heading.textContent = 'Inspect localized traffic safely — '.repeat(8);
    return {
      viewport: document.documentElement.clientWidth,
      shellWidth: root.querySelector('.shell').getBoundingClientRect().width,
      headingHeight: heading.getBoundingClientRect().height,
      controlsVisible: [...root.querySelectorAll('.app-view.active button, .topbar button, .app-footer button')]
        .filter((button) => !button.hidden)
        .every((button) => button.getBoundingClientRect().height > 0),
      invisibleControls: [...root.querySelectorAll('.app-view.active button, .topbar button, .app-footer button')]
        .filter((button) => !button.hidden && button.getBoundingClientRect().height <= 0)
        .map((button) => button.textContent?.trim()),
      rootScroll: document.documentElement.scrollHeight - document.documentElement.clientHeight
    };
  })()`);
  assert(scaled.shellWidth <= scaled.viewport + 1, `200% DPI shell overflow: ${JSON.stringify(scaled)}`);
  assert(scaled.headingHeight > 0 && scaled.controlsVisible, `long localized text hid interactive UI: ${JSON.stringify(scaled)}`);
  assert(scaled.rootScroll <= 1, `200% DPI introduced root scrolling: ${JSON.stringify(scaled)}`);
  await call('Emulation.clearDeviceMetricsOverride');
  await call('Emulation.setEmulatedMedia', { media: 'screen', features: [] });

  const soak = await evaluate(`(async () => {
    const shell = document.querySelector('transmog-app-shell');
    const deadline = performance.now() + ${Math.round(soakMinutes * 60_000)};
    const minimumIterations = ${soakMinutes === 0 ? 100 : 1};
    let iterations = 0;
    do {
      await shell.refreshStatus();
      if (iterations % 20 === 0) await shell.refreshSessions();
      iterations += 1;
      if (deadline > performance.now()) await new Promise((resolve) => setTimeout(resolve, 100));
    } while (iterations < minimumIterations || performance.now() < deadline);
    return { iterations };
  })()`);
  const metrics = await call('Performance.getMetrics');
  const heap = metrics.metrics.find((metric) => metric.name === 'JSHeapUsedSize')?.value ?? Number.POSITIVE_INFINITY;
  assert(heap < 64 * 1024 * 1024, `bounded status soak exceeded 64 MiB JS heap: ${heap}`);
  result.soak = { ...soak, minutes: soakMinutes, finalHeapBytes: heap };
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
