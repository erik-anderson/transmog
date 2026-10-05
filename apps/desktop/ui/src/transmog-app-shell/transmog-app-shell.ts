import { WebUIElement } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';

type Lifecycle = 'stopped' | 'running' | 'stopping' | 'failed';

interface AppStatus {
  lifecycle: Lifecycle;
  listener: string | null;
  summary: string;
  hostRestorePending: boolean;
}
interface CaIdentity { sha256: string; certificatePath: string; }

interface SessionSummary {
  id: string;
  method: string;
  host: string;
  path: string;
  protocol: string;
  status: number | null;
  durationMs: number;
  requestBytes: number;
  responseBytes: number;
  terminal: 'active' | 'completed' | 'failed';
  loss: boolean;
  capturing: boolean;
}

interface SessionPage {
  sessions: SessionSummary[];
  nextCursor: string | null;
  evicted: number;
  sequenceGaps: number;
  subscriberLag: number;
}

interface SessionHint { exchangeId: string | null; sequence: number; lagged: boolean; }
interface SessionDetail {
  id: string;
  requests: unknown[];
  responses: unknown[];
  bodies: unknown[];
  diagnostics: string[];
  hookEffects: string[];
  routeSelection: string | null;
  routeAttempts: string[];
  terminal: string;
  websocket: string | null;
  sequenceLoss: number;
}
type BreakpointPhase = 'request-head' | 'request-body' | 'response-head' | 'response-body';
interface PausedExchange {
  decisionId: number;
  exchangeId: string;
  phase: BreakpointPhase;
  requestHead: Record<string, unknown> | null;
  responseHead: Record<string, unknown> | null;
  bodyHex: string | null;
  hookId: string;
}
interface BreakpointStatus { enabled: boolean; paused: PausedExchange[]; }
interface ComposerResult {
  id: number;
  status: number;
  headers: Array<{name: string; value: string}>;
  body: string;
  bodyIsHex: boolean;
  truncated: boolean;
  attribution: string;
}
type CaptureReadModel = Record<string, unknown>;
interface ProductState {
  schemaVersion: number;
  preferences: { theme: 'system' | 'light' | 'dark'; sessionPageSize: number; configureSystemProxy: boolean };
  privacy: { retainBodySamples: boolean; rememberRecentArtifacts: boolean; includePathsInSupportBundles: boolean };
  window: { width: number; height: number; x: number | null; y: number | null; maximized: boolean };
  recentArtifacts: Array<{path: string; kind: string}>;
}

export class TransmogAppShell extends WebUIElement {
  statusLabel!: HTMLSpanElement;
  listenerValue!: HTMLElement;
  diagnostics!: HTMLOutputElement;
  proxyForm!: HTMLFormElement;
  caThumbprint!: HTMLInputElement;
  filterForm!: HTMLFormElement;
  sessionRows!: HTMLTableSectionElement;
  nextButton!: HTMLButtonElement;
  inspectorOutput!: HTMLPreElement;
  pausedList!: HTMLDivElement;
  breakpointForm!: HTMLFormElement;
  composerForm!: HTMLFormElement;
  composerOutput!: HTMLPreElement;
  captureForm!: HTMLFormElement;
  artifactForm!: HTMLFormElement;
  captureOutput!: HTMLPreElement;
  settingsForm!: HTMLFormElement;
  supportForm!: HTMLFormElement;
  supportOutput!: HTMLPreElement;
  private nextCursor: string | null = null;
  private watching = false;

  async startProxy(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.proxyForm);
    this.diagnostics.textContent = 'Starting the proxy…';
    try {
      await invoke<AppStatus>('start_proxy', {
        request: {
          caCertificatePath: String(data.get('certificate') ?? ''),
          caPrivateKeyPath: String(data.get('privateKey') ?? ''),
          listen: '127.0.0.1:0',
          route: 'auto',
          allowRemoteClients: false,
        },
        configureSystemProxy: data.get('systemProxy') === 'on',
      });
      await this.refreshStatus();
    } catch (error: unknown) {
      this.diagnostics.textContent = `Start failed: ${describeError(error)}`;
    }
  }

  async createCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      const identity = await invoke<CaIdentity>('create_ca', {
        request: {
          certificatePath: String(data.get('certificate') ?? ''),
          privateKeyPath: String(data.get('privateKey') ?? ''),
          commonName: 'Transmog local interception CA',
          validityDays: 3650,
        },
      });
      this.caThumbprint.value = identity.sha256;
      this.diagnostics.textContent = `CA created with a current-user-only private-key ACL. SHA-256 ${identity.sha256}`;
    } catch (error: unknown) {
      this.diagnostics.textContent = `CA creation failed: ${describeError(error)}`;
    }
  }

  async installCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('install_certificate', {
        path: String(data.get('certificate') ?? ''),
        sha256: String(data.get('thumbprint') ?? ''),
      });
      this.diagnostics.textContent = 'The exact public CA is trusted for the current user.';
    } catch (error: unknown) {
      this.diagnostics.textContent = `CA installation failed: ${describeError(error)}`;
    }
  }

  async removeCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('remove_certificate', { sha256: String(data.get('thumbprint') ?? '') });
      this.diagnostics.textContent = 'The exact public CA was removed from current-user trust.';
    } catch (error: unknown) {
      this.diagnostics.textContent = `CA removal failed: ${describeError(error)}`;
    }
  }

  async stopProxy(): Promise<void> {
    this.diagnostics.textContent = 'Stopping and restoring host settings…';
    try {
      await invoke<AppStatus>('stop_application');
      await this.refreshStatus();
    } catch (error: unknown) {
      this.diagnostics.textContent = `Stop failed: ${describeError(error)}`;
    }
  }

  async recoverProxy(): Promise<void> {
    try {
      const restored = await invoke<boolean>('recover_windows_proxy');
      this.diagnostics.textContent = restored
        ? 'The exact journaled Windows proxy settings were restored.'
        : 'No Windows proxy recovery journal was present.';
    } catch (error: unknown) {
      this.diagnostics.textContent = `Recovery failed: ${describeError(error)}`;
    }
  }

  async refreshSessions(event?: Event, cursor: string | null = null): Promise<void> {
    event?.preventDefault();
    const data = new FormData(this.filterForm);
    try {
      const page = await invoke<SessionPage>('query_sessions', {
        query: {
          cursor,
          limit: 100,
          terminal: null,
          method: optionalText(data.get('method')),
          host: optionalText(data.get('host')),
        },
      });
      this.renderSessions(page.sessions);
      this.nextCursor = page.nextCursor;
      this.nextButton.disabled = page.nextCursor === null;
      this.diagnostics.textContent = `Loaded ${page.sessions.length} sessions · evicted ${page.evicted} · gaps ${page.sequenceGaps} · subscriber lag ${page.subscriberLag}`;
    } catch (error: unknown) {
      this.diagnostics.textContent = `Session query failed: ${describeError(error)}`;
    }
  }

  async nextSessions(): Promise<void> {
    if (this.nextCursor !== null) await this.refreshSessions(undefined, this.nextCursor);
  }

  async watchSessions(): Promise<void> {
    if (this.watching) return;
    this.watching = true;
    const onEvent = new Channel<SessionHint>();
    onEvent.onmessage = () => { void this.refreshSessions(); };
    try {
      await invoke<void>('watch_sessions', { onEvent });
      this.diagnostics.textContent = 'Live session refresh enabled.';
    } catch (error: unknown) {
      this.watching = false;
      this.diagnostics.textContent = `Live refresh failed: ${describeError(error)}`;
    }
  }

  private renderSessions(sessions: SessionSummary[]): void {
    this.sessionRows.replaceChildren();
    if (sessions.length === 0) {
      const row = document.createElement('tr');
      const cell = document.createElement('td');
      cell.colSpan = 8;
      cell.textContent = 'No matching sessions.';
      row.append(cell);
      this.sessionRows.append(row);
      return;
    }
    for (const session of sessions.slice(0, 200)) {
      const row = document.createElement('tr');
      const values = [
        session.host, session.path, session.protocol,
        session.status?.toString() ?? '—', `${session.durationMs} ms`,
        `${session.requestBytes} ↑ / ${session.responseBytes} ↓`,
        `${session.terminal}${session.loss ? ' · loss' : ''}${session.capturing ? ' · capture' : ''}`,
      ];
      const methodCell = document.createElement('td');
      const inspect = document.createElement('button');
      inspect.type = 'button';
      inspect.className = 'session-link';
      inspect.textContent = session.method;
      inspect.addEventListener('click', () => { void this.inspectSession(session.id); });
      methodCell.append(inspect);
      row.append(methodCell);
      for (const value of values) {
        const cell = document.createElement('td');
        cell.textContent = value;
        row.append(cell);
      }
      this.sessionRows.append(row);
    }
  }

  private async inspectSession(id: string): Promise<void> {
    this.inspectorOutput.textContent = 'Loading bounded evidence…';
    try {
      const detail = await invoke<SessionDetail>('session_detail', { id });
      this.inspectorOutput.textContent = JSON.stringify(detail, null, 2);
    } catch (error: unknown) {
      this.inspectorOutput.textContent = `Inspector unavailable: ${describeError(error)}`;
    }
  }

  async enableBreakpoints(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.breakpointForm);
    const phases: BreakpointPhase[] = [];
    if (data.get('requestHead') === 'on') phases.push('request-head');
    if (data.get('requestBody') === 'on') phases.push('request-body');
    if (data.get('responseHead') === 'on') phases.push('response-head');
    if (data.get('responseBody') === 'on') phases.push('response-body');
    try {
      const status = await invoke<BreakpointStatus>('enable_breakpoints', {
        settings: {
          phases,
          bodyLimit: 4 * 1024 * 1024,
          maxPending: 64,
          timeoutMs: 30_000,
        },
      });
      this.renderBreakpoints(status);
    } catch (error: unknown) {
      this.diagnostics.textContent = `Breakpoint setup failed: ${describeError(error)}`;
    }
  }

  async disableBreakpoints(): Promise<void> {
    const status = await invoke<BreakpointStatus>('disable_breakpoints');
    this.renderBreakpoints(status);
  }

  async refreshBreakpoints(): Promise<void> {
    try {
      this.renderBreakpoints(await invoke<BreakpointStatus>('breakpoint_status'));
    } catch (error: unknown) {
      this.diagnostics.textContent = `Breakpoint refresh failed: ${describeError(error)}`;
    }
  }

  private renderBreakpoints(status: BreakpointStatus): void {
    this.pausedList.replaceChildren();
    if (status.paused.length === 0) {
      this.pausedList.textContent = status.enabled
        ? 'Controller attached; no exchanges are paused.'
        : 'Breakpoint controller disabled.';
      return;
    }
    for (const paused of status.paused) {
      const card = document.createElement('article');
      card.className = 'paused-card';
      const title = document.createElement('strong');
      title.textContent = `${paused.phase} · ${paused.exchangeId}`;
      const editor = document.createElement('textarea');
      editor.setAttribute('aria-label', `Edit ${paused.phase}`);
      editor.value = paused.bodyHex ?? JSON.stringify(paused.requestHead ?? paused.responseHead, null, 2);
      const actions = document.createElement('div');
      actions.className = 'actions';
      const continueButton = document.createElement('button');
      continueButton.type = 'button';
      continueButton.textContent = 'Continue';
      continueButton.addEventListener('click', () => {
        void this.submitBreakpoint(paused, { action: 'continue' });
      });
      const abortButton = document.createElement('button');
      abortButton.type = 'button';
      abortButton.className = 'secondary';
      abortButton.textContent = 'Abort';
      abortButton.addEventListener('click', () => {
        void this.submitBreakpoint(paused, { action: 'abort', reason: 'aborted by Transmog operator' });
      });
      const replace = document.createElement('button');
      replace.type = 'button';
      replace.className = 'secondary';
      replace.textContent = 'Apply replacement';
      replace.addEventListener('click', () => {
        try {
          const action = paused.phase.endsWith('body')
            ? { action: 'replace-body', body: parseHex(editor.value) }
            : paused.phase === 'request-head'
              ? { action: 'replace-request-head', head: JSON.parse(editor.value) as unknown }
              : { action: 'replace-response-head', head: JSON.parse(editor.value) as unknown };
          void this.submitBreakpoint(paused, action);
        } catch (error: unknown) {
          this.diagnostics.textContent = `Invalid replacement draft: ${describeError(error)}`;
        }
      });
      actions.append(continueButton, abortButton, replace);
      card.append(title, editor, actions);
      this.pausedList.append(card);
    }
  }

  private async submitBreakpoint(paused: PausedExchange, action: object): Promise<void> {
    try {
      const status = await invoke<BreakpointStatus>('decide_breakpoint', {
        decision: { decisionId: paused.decisionId, exchangeId: paused.exchangeId, action },
      });
      this.renderBreakpoints(status);
    } catch (error: unknown) {
      this.diagnostics.textContent = `Breakpoint decision failed: ${describeError(error)}`;
      await this.refreshBreakpoints();
    }
  }

  async executeComposer(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.composerForm);
    try {
      const result = await invoke<ComposerResult>('execute_composer', {
        request: {
          method: String(data.get('method') ?? ''),
          url: String(data.get('url') ?? ''),
          headers: parseHeaderLines(String(data.get('headers') ?? '')),
          body: String(data.get('body') ?? ''),
          bodyIsHex: data.get('bodyHex') === 'on',
          acknowledgeNonIdempotent: data.get('nonIdempotent') === 'on',
          acknowledgeCredentials: data.get('credentials') === 'on',
        },
      });
      this.composerOutput.textContent = JSON.stringify(result, null, 2);
    } catch (error: unknown) {
      this.composerOutput.textContent = `Replay failed: ${describeError(error)}`;
    }
  }

  async startCapture(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.captureForm);
    try {
      const status = await invoke<CaptureReadModel>('start_capture', {
        request: {
          path: String(data.get('capturePath') ?? ''),
          maxFileBytes: Number(data.get('quota')),
          retainBodySamples: data.get('bodies') === 'on',
        },
      });
      this.captureOutput.textContent = JSON.stringify(status, null, 2);
    } catch (error: unknown) {
      this.captureOutput.textContent = `Capture start failed: ${describeError(error)}`;
    }
  }

  async stopCapture(): Promise<void> {
    try {
      const status = await invoke<CaptureReadModel>('stop_capture');
      this.captureOutput.textContent = JSON.stringify(status, null, 2);
    } catch (error: unknown) {
      this.captureOutput.textContent = `Capture finalization failed: ${describeError(error)}`;
    }
  }

  async refreshCapture(): Promise<void> {
    this.captureOutput.textContent = JSON.stringify(await invoke<CaptureReadModel>('capture_status'), null, 2);
  }

  async inspectCapture(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.artifactForm);
    try {
      const summary = await invoke<Record<string, unknown>>('import_capture', {
        request: { path: String(data.get('source') ?? ''), maxFileBytes: 4_294_967_296 },
      });
      this.captureOutput.textContent = JSON.stringify(summary, null, 2);
    } catch (error: unknown) {
      this.captureOutput.textContent = `Import failed: ${describeError(error)}`;
    }
  }

  async exportCapture(): Promise<void> {
    const data = new FormData(this.artifactForm);
    try {
      const report = await invoke<Record<string, unknown>>('export_capture', {
        request: {
          source: String(data.get('source') ?? ''),
          destination: String(data.get('destination') ?? ''),
          format: String(data.get('format') ?? 'json-lines'),
          maxSourceBytes: 4_294_967_296,
        },
      });
      this.captureOutput.textContent = JSON.stringify(report, null, 2);
    } catch (error: unknown) {
      this.captureOutput.textContent = `Export failed: ${describeError(error)}`;
    }
  }

  async loadSettings(): Promise<void> {
    try {
      const state = await invoke<ProductState>('product_state');
      const elements = this.settingsForm.elements;
      (elements.namedItem('theme') as HTMLSelectElement).value = state.preferences.theme;
      (elements.namedItem('pageSize') as HTMLInputElement).value = String(state.preferences.sessionPageSize);
      (elements.namedItem('defaultSystemProxy') as HTMLInputElement).checked = state.preferences.configureSystemProxy;
      (elements.namedItem('defaultBodies') as HTMLInputElement).checked = state.privacy.retainBodySamples;
      (elements.namedItem('rememberArtifacts') as HTMLInputElement).checked = state.privacy.rememberRecentArtifacts;
      (elements.namedItem('supportPaths') as HTMLInputElement).checked = state.privacy.includePathsInSupportBundles;
      this.supportOutput.textContent = `Loaded schema ${state.schemaVersion}; ${state.recentArtifacts.length} recent artifact reference(s).`;
    } catch (error: unknown) {
      this.supportOutput.textContent = `Settings load failed: ${describeError(error)}`;
    }
  }

  async saveSettings(event: Event): Promise<void> {
    event.preventDefault();
    try {
      const state = await invoke<ProductState>('product_state');
      const data = new FormData(this.settingsForm);
      state.preferences.theme = String(data.get('theme')) as ProductState['preferences']['theme'];
      state.preferences.sessionPageSize = Number(data.get('pageSize'));
      state.preferences.configureSystemProxy = data.get('defaultSystemProxy') === 'on';
      state.privacy.retainBodySamples = data.get('defaultBodies') === 'on';
      state.privacy.rememberRecentArtifacts = data.get('rememberArtifacts') === 'on';
      state.privacy.includePathsInSupportBundles = data.get('supportPaths') === 'on';
      const saved = await invoke<ProductState>('save_product_state', { productState: state });
      this.supportOutput.textContent = `Saved product-state schema ${saved.schemaVersion}.`;
    } catch (error: unknown) {
      this.supportOutput.textContent = `Settings save failed: ${describeError(error)}`;
    }
  }

  async refreshDiagnostics(): Promise<void> {
    try {
      const report = await invoke<Record<string, unknown>>('diagnostics_report');
      this.supportOutput.textContent = JSON.stringify(report, null, 2);
    } catch (error: unknown) {
      this.supportOutput.textContent = `Diagnostics unavailable: ${describeError(error)}`;
    }
  }

  async createSupportBundle(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.supportForm);
    try {
      const result = await invoke<Record<string, unknown>>('create_support_bundle', {
        destination: String(data.get('destination') ?? ''),
        includeRecentPaths: data.get('includePaths') === 'on',
      });
      this.supportOutput.textContent = JSON.stringify(result, null, 2);
    } catch (error: unknown) {
      this.supportOutput.textContent = `Support bundle failed: ${describeError(error)}`;
    }
  }

  async prepareUpdate(): Promise<void> {
    try {
      await invoke<AppStatus>('prepare_update_handoff');
      this.supportOutput.textContent = 'Proxy, capture, breakpoints, and Windows host changes are stopped. Close Transmog before running the installer.';
    } catch (error: unknown) {
      this.supportOutput.textContent = `Update handoff failed: ${describeError(error)}`;
    }
  }

  async refreshStatus(): Promise<void> {
    try {
      const status = await invoke<AppStatus>('app_status');
      this.statusLabel.textContent = lifecycleLabel(status.lifecycle);
      this.listenerValue.textContent = status.listener ?? 'Not listening';
      this.diagnostics.textContent = status.hostRestorePending
        ? 'Host restoration is pending and must be retried before restart.'
        : status.summary;
      this.shadowRoot?.querySelector('.status')?.setAttribute('data-lifecycle', status.lifecycle);
    } catch (error: unknown) {
      this.diagnostics.textContent = `Status unavailable: ${describeError(error)}`;
    }
  }
}

TransmogAppShell.define('transmog-app-shell');

function lifecycleLabel(lifecycle: Lifecycle): string {
  switch (lifecycle) {
    case 'running': return 'Running';
    case 'stopping': return 'Stopping';
    case 'failed': return 'Needs attention';
    default: return 'Stopped';
  }
}

function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'unknown failure';
}

function optionalText(value: FormDataEntryValue | null): string | null {
  const text = String(value ?? '').trim();
  return text.length === 0 ? null : text;
}

function parseHex(value: string): number[] {
  const compact = value.replace(/\s+/g, '');
  if (compact.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(compact)) {
    throw new Error('body must be hexadecimal bytes');
  }
  const bytes: number[] = [];
  for (let index = 0; index < compact.length; index += 2) {
    bytes.push(Number.parseInt(compact.slice(index, index + 2), 16));
  }
  return bytes;
}

function parseHeaderLines(value: string): Array<{name: string; value: string}> {
  return value.split(/\r?\n/).filter((line) => line.trim().length > 0).map((line) => {
    const separator = line.indexOf(':');
    if (separator <= 0) throw new Error('each header must use “Name: value”');
    return { name: line.slice(0, separator).trim(), value: line.slice(separator + 1).trim() };
  });
}
