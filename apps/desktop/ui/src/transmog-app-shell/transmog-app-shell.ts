import { WebUIElement } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import { ModuleKind, ModuleResolutionKind, ScriptTarget, monaco, typescriptDefaults } from '../monaco.js';

function reportUnhandledFrontendIssue(code: string, value: unknown): void {
  void invoke<void>('record_frontend_diagnostic', {
    code,
    message: describeError(value),
  }).catch(() => undefined);
}

window.addEventListener('error', (event) => {
  reportUnhandledFrontendIssue('unhandled-error', event.error ?? event.message);
});
window.addEventListener('unhandledrejection', (event) => {
  reportUnhandledFrontendIssue('unhandled-rejection', event.reason);
});

type Lifecycle = 'stopped' | 'running' | 'stopping' | 'failed';

interface AppStatus {
  lifecycle: Lifecycle;
  listener: string | null;
  summary: string;
  hostRestorePending: boolean;
}
interface CaIdentity { sha256: string; certificatePath: string; }
interface DesktopBootstrap {
  caCertificatePath: string;
  caPrivateKeyPath: string;
  caFilesPresent: boolean;
  ownedCaSha256: string | null;
  ownedCaTrusted: boolean;
  hostRestorePending: boolean;
  diagnosticsPath: string;
}

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
  storedBodies: StoredBodyMetadata[];
  diagnostics: string[];
  hookEffects: string[];
  routeSelection: string | null;
  routeAttempts: string[];
  terminal: string;
  websocket: string | null;
  sequenceLoss: number;
}
interface StoredBodyMetadata {
  exchangeId: string;
  boundary: string;
  observedBytes: number;
  retainedBytes: number;
  availability: string;
  mediaType: string | null;
  charset: string | null;
  contentCodings: string[];
  sha256: string | null;
  reason: string | null;
}
interface BodyInspection {
  metadata: StoredBodyMetadata;
  representation: string;
  decoded: boolean;
  display: string;
  displayBytes: number;
  truncated: boolean;
  nextOffset: number | null;
  warning: string | null;
  previewHandle: string | null;
  previewMimeType: string | null;
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
  privacy: { retainResponseBodies: boolean; retainBodySamples: boolean; rememberRecentArtifacts: boolean; includePathsInSupportBundles: boolean };
  window: { width: number; height: number; x: number | null; y: number | null; maximized: boolean };
  recentArtifacts: Array<{path: string; kind: string}>;
}
interface AutomationCandidate { candidateId: string; ruleCount: number; registrationCount: number; }
interface AutomationStatus {
  generation: number;
  rules: Array<Record<string, unknown> & {id: string; revision: number; priority: number}>;
  candidateCount: number;
  historyCount: number;
}
interface ResponseAsset { id: string; revision: number; status: number; bodyBytes: number; sha256: string; mediaType: string | null; }
type ScriptHandler = 'onRequestHead' | 'onRequestBody' | 'onResponseHead' | 'onResponseBody';
interface ScriptDraft {
  id: string;
  revision: number;
  source: string;
  handlers: ScriptHandler[];
  prefilter: { method: string | null; host: string | null; pathPrefix: string | null };
  capabilities: {
    readSensitiveHeaders: boolean;
    readBodies: boolean;
    writeHeaders: string[];
    writeBody: boolean;
    respond: boolean;
    abort: boolean;
  };
  limits: {
    maxInputBodyBytes: number;
    maxOutputBytes: number;
    maxLogBytes: number;
    maxHeapBytes: number;
    maxDurationMs: number;
  };
  priority: number;
}
interface ScriptRevision { manifest: { id: string; revision: number; sourceHash: string }; source: string; }
interface ScriptStatus {
  generation: number;
  active: ScriptRevision[];
  saved: ScriptDraft[];
  candidateCount: number;
  historyCount: number;
}
interface ScriptCandidate { candidateId: string; scriptId: string; revision: number; sourceHash: string; }

const SCRIPT_TEMPLATE = `import type { Action, Context, Request } from "transmog:api/v1";

export function onRequestHead(_context: Context, request: Request): Action {
  if (request.host === "example.test") {
    return {
      action: "headers",
      operations: [{ operation: "set", name: "User-Agent", value: "Transmog/1.0" }],
    };
  }
  return { action: "continue" };
}
`;

export class TransmogAppShell extends WebUIElement {
  statusLabel!: HTMLSpanElement;
  listenerValue!: HTMLElement;
  diagnostics!: HTMLOutputElement;
  notice!: HTMLElement;
  noticeTitle!: HTMLElement;
  noticeMessage!: HTMLParagraphElement;
  noticeActionButton!: HTMLButtonElement;
  proxyForm!: HTMLFormElement;
  proxyControls!: HTMLFieldSetElement;
  proxyOutput!: HTMLOutputElement;
  sessionOutput!: HTMLOutputElement;
  caCertificate!: HTMLInputElement;
  caPrivateKey!: HTMLInputElement;
  caThumbprint!: HTMLInputElement;
  filterForm!: HTMLFormElement;
  sessionRows!: HTMLTableSectionElement;
  sessionScroller!: HTMLElement;
  followStatus!: HTMLElement;
  followButton!: HTMLButtonElement;
  selectedMethod!: HTMLElement;
  selectedUrl!: HTMLElement;
  selectedStatus!: HTMLElement;
  requestInspectorOutput!: HTMLPreElement;
  responseInspectorOutput!: HTMLPreElement;
  bodyBoundary!: HTMLSelectElement;
  bodyRepresentation!: HTMLSelectElement;
  bodyDecoded!: HTMLInputElement;
  bodyMaxBytes!: HTMLInputElement;
  bodyPreviewOutput!: HTMLPreElement;
  bodyImagePreview!: HTMLImageElement;
  pausedList!: HTMLDivElement;
  breakpointForm!: HTMLFormElement;
  automationForm!: HTMLFormElement;
  autoResponseForm!: HTMLFormElement;
  responseAssetSelect!: HTMLSelectElement;
  automationOutput!: HTMLPreElement;
  scriptForm!: HTMLFormElement;
  scriptEditor!: HTMLDivElement;
  scriptDiffEditor!: HTMLDivElement;
  scriptOutput!: HTMLPreElement;
  composerForm!: HTMLFormElement;
  composerOutput!: HTMLPreElement;
  captureForm!: HTMLFormElement;
  artifactForm!: HTMLFormElement;
  captureOutput!: HTMLPreElement;
  settingsForm!: HTMLFormElement;
  supportForm!: HTMLFormElement;
  supportOutput!: HTMLPreElement;
  diagnosticsPath!: HTMLElement;
  private watching = false;
  private sessionUpdates: Channel<SessionHint> | null = null;
  private followLatest = true;
  private programmaticSessionScroll = false;
  private noticeAction: 'setup-ca' | 'start-proxy' | 'settings' | 'recover-proxy' | null = null;
  private scriptRevision = Date.now();
  private sessionLimit = 100;
  private selectedSessionId: string | null = null;
  private sourceEditor: monaco.editor.IStandaloneCodeEditor | null = null;
  private revisionDiff: monaco.editor.IStandaloneDiffEditor | null = null;
  private diffModels: monaco.editor.ITextModel[] = [];
  private breakpointEditors: monaco.editor.IStandaloneCodeEditor[] = [];
  private candidate: ScriptCandidate | null = null;
  private readonly systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
  private readonly systemThemeChanged = (): void => {
    if (this.dataset.theme === 'system') this.applyMonacoTheme('system');
  };
  private readonly sessionScrolled = (): void => {
    if (this.programmaticSessionScroll || this.selectedSessionId !== null) return;
    const distance = this.sessionScroller.scrollHeight
      - this.sessionScroller.scrollTop
      - this.sessionScroller.clientHeight;
    this.followLatest = distance <= 4;
    this.renderFollowState();
  };

  connectedCallback(): void {
    super.connectedCallback();
    this.lockRootViewport();
    this.systemTheme.addEventListener('change', this.systemThemeChanged);
    queueMicrotask(() => {
      this.sessionScroller.addEventListener('scroll', this.sessionScrolled, { passive: true });
      void this.initializeShell();
      void this.initializeScriptEditor().catch((error: unknown) => {
        this.scriptOutput.textContent = `Script editor initialization failed: ${describeError(error)}`;
        void this.reportFrontendIssue('script-editor-initialization-failed', error);
      });
    });
  }

  private lockRootViewport(): void {
    for (const root of [document.documentElement, document.body]) {
      root.style.width = '100%';
      root.style.height = '100%';
      root.style.margin = '0';
      root.style.overflow = 'hidden';
    }
  }

  disconnectedCallback(): void {
    this.systemTheme.removeEventListener('change', this.systemThemeChanged);
    this.sessionScroller?.removeEventListener('scroll', this.sessionScrolled);
    for (const editor of this.breakpointEditors) editor.dispose();
    this.breakpointEditors = [];
    this.revisionDiff?.dispose();
    this.sourceEditor?.dispose();
    for (const model of this.diffModels) model.dispose();
    this.diffModels = [];
    super.disconnectedCallback();
  }

  private async initializeScriptEditor(): Promise<void> {
    if (this.sourceEditor !== null || this.scriptEditor === undefined) return;
    const declarations = await invoke<string>('script_declarations');
    typescriptDefaults.setCompilerOptions({
      allowNonTsExtensions: true,
      module: ModuleKind.ESNext,
      moduleResolution: ModuleResolutionKind.NodeJs,
      noEmit: true,
      strict: true,
      target: ScriptTarget.ESNext,
    });
    typescriptDefaults.setDiagnosticsOptions({
      noSemanticValidation: false,
      noSyntaxValidation: false,
    });
    typescriptDefaults.addExtraLib(
      declarations,
      'file:///transmog-script-api/v1.d.ts',
    );
    this.ensureMonacoStyles();
    const model = monaco.editor.createModel(SCRIPT_TEMPLATE, 'typescript', monaco.Uri.parse('file:///transmog-scripts/draft/main.ts'));
    this.sourceEditor = monaco.editor.create(this.scriptEditor, {
      model,
      automaticLayout: true,
      accessibilitySupport: 'on',
      ariaLabel: 'Traffic script TypeScript source',
      minimap: { enabled: false },
      tabFocusMode: true,
      theme: this.monacoTheme(),
    });
    this.sourceEditor.onDidChangeModelContent(() => { this.candidate = null; });
    this.scriptOutput.textContent = 'Ready. Monaco diagnostics are advisory; Rust validation is authoritative.';
    await this.refreshScripts();
  }

  private async initializeShell(): Promise<void> {
    this.setProxyOutput('Loading durable proxy state…', 'progress');
    try {
      const [bootstrap, productState, status] = await Promise.all([
        invoke<DesktopBootstrap>('desktop_bootstrap'),
        invoke<ProductState>('product_state'),
        invoke<AppStatus>('app_status'),
      ]);
      this.caCertificate.value = bootstrap.caCertificatePath;
      this.caPrivateKey.value = bootstrap.caPrivateKeyPath;
      this.caThumbprint.value = bootstrap.ownedCaSha256 ?? '';
      this.diagnosticsPath.textContent = bootstrap.diagnosticsPath;
      this.applyTheme(productState.preferences.theme);
      this.populateSettings(productState);
      this.renderAppStatus(status);
      const certificate = bootstrap.ownedCaSha256 !== null && !bootstrap.caFilesPresent
        ? `Owned CA ${bootstrap.ownedCaSha256} is remembered, but its app-managed files are missing. Remove exact CA, then create and trust a new durable CA.`
        : bootstrap.ownedCaSha256 === null
          ? 'No app-owned trusted CA is recorded. Create and trust one before intercepting HTTPS.'
          : bootstrap.ownedCaTrusted
            ? `Owned CA ${bootstrap.ownedCaSha256} is present in current-user trust.`
            : `Owned CA ${bootstrap.ownedCaSha256} is not present in current-user trust.`;
      const ready = bootstrap.caFilesPresent && bootstrap.ownedCaTrusted;
      this.setProxyOutput(`${certificate} Diagnostic log: ${bootstrap.diagnosticsPath}`, ready ? 'success' : bootstrap.ownedCaSha256 !== null && !bootstrap.caFilesPresent ? 'error' : 'progress');
      if (bootstrap.hostRestorePending) {
        this.showNotice(
          'Windows proxy recovery required',
          'Transmog could not restore the settings journaled by a previous run. Restore them before starting again.',
          'Restore settings',
          'recover-proxy',
        );
      } else if (!ready) {
        this.showNotice(
          'HTTPS interception needs setup',
          'Before the proxy can start, create and trust Transmog’s local interception certificate. Windows will ask you to approve adding it to your current-user trusted roots.',
          'Set up now',
          'setup-ca',
        );
      }
      await this.watchSessions();
      await Promise.all([this.refreshAutomation(), this.refreshResponseAssets()]);
    } catch (error: unknown) {
      const message = `Desktop initialization failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
      await this.reportFrontendIssue('desktop-initialization-failed', error);
    } finally {
      this.proxyControls.disabled = false;
    }
  }

  showView(event: Event): void {
    event.preventDefault();
    const link = event.currentTarget as HTMLAnchorElement;
    const view = link.dataset.view;
    if (view === undefined) return;
    this.activateView(view);
  }

  private activateView(view: string): void {
    for (const candidate of this.shadowRoot?.querySelectorAll<HTMLAnchorElement>('.rail a[data-view]') ?? []) {
      const active = candidate.dataset.view === view;
      candidate.classList.toggle('active', active);
      if (active) candidate.setAttribute('aria-current', 'page');
      else candidate.removeAttribute('aria-current');
    }
    for (const section of this.shadowRoot?.querySelectorAll<HTMLElement>('.app-view') ?? []) {
      const active = section.id === view;
      section.hidden = !active;
      section.classList.toggle('active', active);
    }
    if (view === 'automation') {
      requestAnimationFrame(() => {
        this.sourceEditor?.layout();
        this.revisionDiff?.layout();
      });
    }
  }

  private ensureMonacoStyles(): void {
    if (this.shadowRoot?.querySelector('link[data-monaco]') !== null) return;
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = '/app.css';
    link.dataset.monaco = 'true';
    this.shadowRoot?.append(link);
  }

  async startProxy(event?: Event): Promise<void> {
    event?.preventDefault();
    const data = new FormData(this.proxyForm);
    try {
      const bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      if (bootstrap.hostRestorePending) {
        this.showNotice(
          'Windows proxy recovery required',
          'Restore the exact settings journaled by the previous run before starting the proxy again.',
          'Restore settings',
          'recover-proxy',
        );
        this.setProxyOutput('Start blocked: Windows proxy recovery is pending.', 'error');
        return;
      }
      if (!bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null || !bootstrap.ownedCaTrusted) {
        const missing = !bootstrap.caFilesPresent || bootstrap.ownedCaSha256 === null
          ? 'Create and trust Transmog’s interception certificate before starting the proxy.'
          : 'Transmog’s interception certificate is not trusted for the current user. Trust it before starting the proxy.';
        this.showNotice('Proxy setup required', missing, 'Set up HTTPS interception', 'setup-ca');
        this.setProxyOutput(`Start blocked: ${missing}`, 'error');
        this.activateView('settings');
        return;
      }
      this.setProxyOutput('Starting capture, binding the proxy, and applying current-user Windows proxy settings…', 'progress');
      const status = await invoke<AppStatus>('start_proxy', {
        request: {
          caCertificatePath: String(data.get('certificate') ?? ''),
          caPrivateKeyPath: String(data.get('privateKey') ?? ''),
          listen: '127.0.0.1:0',
          route: 'auto',
          allowRemoteClients: false,
        },
      });
      this.renderAppStatus(status);
      const listener = status.listener ?? 'an unknown listener';
      this.dismissNotice();
      this.setProxyOutput(`Proxy listening at ${listener}. Windows proxy settings are active and traffic is being captured automatically.`, 'success');
      await this.watchSessions();
    } catch (error: unknown) {
      const message = `Start failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
      this.showNotice('Proxy could not start', `${message} See Settings for recovery options.`, 'Open Settings', 'settings');
    }
  }

  async setupCa(): Promise<void> {
    const proceed = window.confirm(
      'Transmog will create a durable local interception certificate if needed, then ask Windows to trust its public certificate for your account. You must manually approve the Windows certificate dialog. Continue?',
    );
    if (!proceed) return;
    const data = new FormData(this.proxyForm);
    this.setProxyOutput('Preparing the interception certificate…', 'progress');
    try {
      let bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      let sha256 = bootstrap.ownedCaSha256;
      if (!bootstrap.caFilesPresent) {
        const identity = await invoke<CaIdentity>('create_ca', {
          request: {
            certificatePath: String(data.get('certificate') ?? ''),
            privateKeyPath: String(data.get('privateKey') ?? ''),
            commonName: 'Transmog local interception CA',
            validityDays: 3650,
          },
        });
        sha256 = identity.sha256;
        this.caThumbprint.value = identity.sha256;
      }
      if (sha256 === null) {
        throw new Error('The existing certificate files do not have a recorded Transmog identity. Remove the files and run setup again.');
      }
      this.setProxyOutput('Approve the Windows certificate dialog to trust the Transmog interception CA.', 'progress');
      await invoke<void>('install_certificate', {
        path: String(data.get('certificate') ?? ''),
        sha256,
      });
      bootstrap = await invoke<DesktopBootstrap>('desktop_bootstrap');
      if (!bootstrap.ownedCaTrusted) throw new Error('Windows did not report the certificate as trusted.');
      this.caThumbprint.value = sha256;
      const message = 'HTTPS interception is ready. Windows trusts the exact Transmog certificate for the current user.';
      this.setProxyOutput(message, 'success');
      this.showNotice('HTTPS interception is ready', 'You can start the proxy now.', 'Start proxy', 'start-proxy');
    } catch (error: unknown) {
      const message = `Certificate setup failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.showNotice('Certificate setup needs attention', message, 'Try again', 'setup-ca');
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
      const message = `CA created with a current-user-only private-key ACL. SHA-256 ${identity.sha256}. Trust the public CA before intercepting HTTPS.`;
      this.setProxyOutput(message, 'success');
      this.diagnostics.textContent = message;
    } catch (error: unknown) {
      const message = `CA creation failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
    }
  }

  async installCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('install_certificate', {
        path: String(data.get('certificate') ?? ''),
        sha256: String(data.get('thumbprint') ?? ''),
      });
      const message = 'The exact public CA is trusted for the current user and its SHA-256 identity will be restored after restart.';
      this.setProxyOutput(message, 'success');
      this.diagnostics.textContent = message;
    } catch (error: unknown) {
      const message = `CA installation failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
    }
  }

  async removeCa(): Promise<void> {
    const data = new FormData(this.proxyForm);
    try {
      await invoke<void>('remove_certificate', { sha256: String(data.get('thumbprint') ?? '') });
      const message = 'The exact public CA was removed from current-user trust. The durable CA files and identity were retained so setup can trust it again.';
      this.setProxyOutput(message, 'success');
      this.diagnostics.textContent = message;
    } catch (error: unknown) {
      const message = `CA removal failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
    }
  }

  async stopProxy(): Promise<void> {
    this.setProxyOutput('Stopping and restoring host settings…', 'progress');
    try {
      const status = await invoke<AppStatus>('stop_application');
      this.renderAppStatus(status);
      this.setProxyOutput('Proxy stopped and any current-user Windows proxy changes were restored.', 'success');
    } catch (error: unknown) {
      const message = `Stop failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
    }
  }

  async recoverProxy(): Promise<void> {
    try {
      const restored = await invoke<boolean>('recover_windows_proxy');
      const message = restored
        ? 'The exact journaled Windows proxy settings were restored.'
        : 'No Windows proxy recovery journal was present.';
      this.setProxyOutput(message, 'success');
      this.diagnostics.textContent = message;
    } catch (error: unknown) {
      const message = `Recovery failed: ${describeError(error)}`;
      this.setProxyOutput(message, 'error');
      this.diagnostics.textContent = message;
    }
  }

  async refreshSessions(event?: Event): Promise<void> {
    event?.preventDefault();
    const data = new FormData(this.filterForm);
    try {
      const page = await invoke<SessionPage>('query_sessions', {
        query: {
          cursor: null,
          latest: true,
          limit: this.sessionLimit,
          terminal: null,
          method: optionalText(data.get('method')),
          host: optionalText(data.get('host')),
        },
      });
      this.renderSessions(page.sessions);
      const message = `Loaded ${page.sessions.length} sessions · evicted ${page.evicted} · gaps ${page.sequenceGaps} · subscriber lag ${page.subscriberLag}`;
      this.sessionOutput.textContent = message;
      this.sessionOutput.dataset.kind = 'success';
      this.diagnostics.textContent = message;
    } catch (error: unknown) {
      const message = `Session query failed: ${describeError(error)}`;
      this.sessionOutput.textContent = message;
      this.sessionOutput.dataset.kind = 'error';
      this.diagnostics.textContent = message;
    }
  }

  async watchSessions(): Promise<void> {
    if (this.watching) {
      this.sessionOutput.textContent = 'Live session refresh is already enabled.';
      this.sessionOutput.dataset.kind = 'success';
      return;
    }
    this.watching = true;
    this.sessionOutput.textContent = 'Enabling live session refresh…';
    this.sessionOutput.dataset.kind = 'progress';
    const onEvent = new Channel<SessionHint>();
    onEvent.onmessage = () => { void this.refreshSessions(); };
    this.sessionUpdates = onEvent;
    try {
      await invoke<void>('watch_sessions', { onEvent });
      await this.refreshSessions();
      this.sessionOutput.textContent = 'Watching live traffic. Waiting for proxied requests.';
      this.sessionOutput.dataset.kind = 'success';
      this.diagnostics.textContent = 'Live session refresh enabled.';
    } catch (error: unknown) {
      this.watching = false;
      this.sessionUpdates = null;
      const message = `Live refresh failed: ${describeError(error)}`;
      this.sessionOutput.textContent = message;
      this.sessionOutput.dataset.kind = 'error';
      this.diagnostics.textContent = message;
    }
  }

  private renderSessions(sessions: SessionSummary[]): void {
    if (this.followLatest) this.programmaticSessionScroll = true;
    this.sessionRows.replaceChildren();
    if (sessions.length === 0) {
      const row = document.createElement('tr');
      const cell = document.createElement('td');
      cell.colSpan = 8;
      cell.textContent = 'No matching sessions.';
      row.append(cell);
      this.sessionRows.append(row);
      this.programmaticSessionScroll = false;
      this.renderFollowState();
      return;
    }
    for (const session of sessions.slice(0, 200)) {
      const row = document.createElement('tr');
      row.tabIndex = 0;
      row.dataset.sessionId = session.id;
      row.classList.toggle('selected', this.selectedSessionId === session.id);
      row.setAttribute('aria-selected', String(this.selectedSessionId === session.id));
      const values = [
        session.status?.toString() ?? '—', session.host, session.path, session.protocol,
        `${session.durationMs} ms`, `${session.responseBytes} B`,
        `${session.terminal}${session.loss ? ' · loss' : ''}${session.capturing ? ' · capture' : ''}`,
      ];
      const methodCell = document.createElement('td');
      const inspect = document.createElement('button');
      inspect.type = 'button';
      inspect.className = 'session-link';
      inspect.textContent = session.method;
      inspect.addEventListener('click', (event) => {
        event.stopPropagation();
        void this.inspectSession(session);
      });
      methodCell.append(inspect);
      row.append(methodCell);
      for (const value of values) {
        const cell = document.createElement('td');
        cell.textContent = value;
        row.append(cell);
      }
      row.addEventListener('click', () => { void this.inspectSession(session); });
      row.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          void this.inspectSession(session);
        }
      });
      this.sessionRows.append(row);
    }
    if (this.followLatest) {
      requestAnimationFrame(() => {
        this.sessionScroller.scrollTop = this.sessionScroller.scrollHeight;
        requestAnimationFrame(() => { this.programmaticSessionScroll = false; });
      });
    }
    this.renderFollowState();
  }

  private async inspectSession(session: SessionSummary): Promise<void> {
    this.followLatest = false;
    this.renderFollowState();
    this.selectedMethod.textContent = session.method;
    this.selectedUrl.textContent = `${session.host}${session.path}`;
    this.selectedStatus.textContent = session.status === null ? 'Pending' : `${session.status}`;
    this.selectedStatus.classList.toggle('success', session.status !== null && session.status >= 200 && session.status < 400);
    this.requestInspectorOutput.textContent = 'Loading bounded request evidence…';
    this.responseInspectorOutput.textContent = 'Loading bounded response evidence…';
    try {
      const detail = await invoke<SessionDetail>('session_detail', { id: session.id });
      this.selectedSessionId = session.id;
      for (const row of this.sessionRows.querySelectorAll<HTMLTableRowElement>('tr[data-session-id]')) {
        const selected = row.dataset.sessionId === session.id;
        row.classList.toggle('selected', selected);
        row.setAttribute('aria-selected', String(selected));
      }
      this.requestInspectorOutput.textContent = JSON.stringify({
        requests: detail.requests,
        hookEffects: detail.hookEffects,
        routeSelection: detail.routeSelection,
        routeAttempts: detail.routeAttempts,
      }, null, 2);
      this.responseInspectorOutput.textContent = JSON.stringify({
        responses: detail.responses,
        terminal: detail.terminal,
        websocket: detail.websocket,
        diagnostics: detail.diagnostics,
        sequenceLoss: detail.sequenceLoss,
      }, null, 2);
      this.bodyBoundary.replaceChildren();
      for (const body of detail.storedBodies) {
        const option = document.createElement('option');
        option.value = body.boundary;
        option.textContent = `${body.boundary} · ${body.availability} · ${body.retainedBytes} retained`;
        this.bodyBoundary.append(option);
      }
      this.bodyPreviewOutput.textContent = detail.storedBodies.length === 0
        ? 'No retained body boundaries are available.'
        : 'Choose a representation and inspect the selected boundary.';
    } catch (error: unknown) {
      const message = `Inspector unavailable: ${describeError(error)}`;
      this.requestInspectorOutput.textContent = message;
      this.responseInspectorOutput.textContent = message;
    }
  }

  resumeLatest(): void {
    this.selectedSessionId = null;
    this.followLatest = true;
    for (const row of this.sessionRows.querySelectorAll<HTMLTableRowElement>('tr[data-session-id]')) {
      row.classList.remove('selected');
      row.setAttribute('aria-selected', 'false');
    }
    this.programmaticSessionScroll = true;
    this.sessionScroller.scrollTop = this.sessionScroller.scrollHeight;
    requestAnimationFrame(() => { this.programmaticSessionScroll = false; });
    this.renderFollowState();
  }

  private renderFollowState(): void {
    this.followStatus.textContent = this.followLatest
      ? 'Following latest traffic'
      : this.selectedSessionId === null
        ? 'Live updates continue · scroll position paused'
        : 'Live updates continue · selected request pinned';
    this.followButton.disabled = this.followLatest;
  }

  async exportLiveCapture(): Promise<void> {
    this.sessionOutput.textContent = 'Exporting a sealed TMCap snapshot…';
    this.sessionOutput.dataset.kind = 'progress';
    try {
      const result = await invoke<{destination: string; records: number; bytes: number}>('export_live_capture');
      const message = `Exported ${result.records} records (${result.bytes} bytes) to ${result.destination}`;
      this.sessionOutput.textContent = message;
      this.sessionOutput.dataset.kind = 'success';
      this.diagnostics.textContent = message;
      this.showNotice('TMCap export complete', result.destination, null, null);
    } catch (error: unknown) {
      const message = `TMCap export failed: ${describeError(error)}`;
      this.sessionOutput.textContent = message;
      this.sessionOutput.dataset.kind = 'error';
      this.showNotice('TMCap export failed', message, null, null);
    }
  }

  async inspectBody(): Promise<void> {
    if (this.selectedSessionId === null || this.bodyBoundary.value.length === 0) {
      this.bodyPreviewOutput.textContent = 'Select a session with retained body metadata first.';
      return;
    }
    this.bodyPreviewOutput.textContent = 'Loading bounded body representation…';
    this.bodyImagePreview.hidden = true;
    this.bodyImagePreview.removeAttribute('src');
    try {
      const inspection = await invoke<BodyInspection>('inspect_body', {
        request: {
          sessionId: this.selectedSessionId,
          boundary: this.bodyBoundary.value,
          representation: this.bodyRepresentation.value,
          decodeContent: this.bodyDecoded.checked,
          offset: 0,
          maxBytes: Number(this.bodyMaxBytes.value),
        },
      });
      const summary = `${inspection.metadata.boundary} · ${inspection.representation} · ${inspection.displayBytes} bytes${inspection.decoded ? ' · decoded' : ' · encoded'}${inspection.truncated ? ' · truncated' : ''}`;
      if (inspection.previewHandle !== null) {
        this.bodyImagePreview.src = `transmog-preview://localhost/preview/${encodeURIComponent(inspection.previewHandle)}`;
        this.bodyImagePreview.hidden = false;
        const safety = inspection.previewMimeType === 'image/svg+xml'
          ? 'Loaded as a sandboxed image; scripts and external resources are blocked.'
          : 'Normalized to PNG in an isolated decoder, then loaded as an image.';
        this.bodyPreviewOutput.textContent = `${summary}\n${safety}`;
      } else {
        this.bodyPreviewOutput.textContent = `${summary}${inspection.warning ? `\n${inspection.warning}` : ''}\n\n${inspection.display}`;
      }
    } catch (error: unknown) {
      this.bodyPreviewOutput.textContent = `Body inspector unavailable: ${describeError(error)}`;
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
    for (const editor of this.breakpointEditors) editor.dispose();
    this.breakpointEditors = [];
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
      const editorHost = document.createElement('div');
      editorHost.className = 'monaco-editor-host';
      editorHost.setAttribute('aria-label', `Edit ${paused.phase}`);
      const editor = monaco.editor.create(editorHost, {
        value: paused.bodyHex ?? JSON.stringify(paused.requestHead ?? paused.responseHead, null, 2),
        language: paused.phase.endsWith('body') ? 'plaintext' : 'json',
        automaticLayout: true,
        accessibilitySupport: 'on',
        ariaLabel: `Edit paused ${paused.phase}`,
        minimap: { enabled: false },
        tabFocusMode: true,
        theme: this.monacoTheme(),
      });
      this.breakpointEditors.push(editor);
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
            ? { action: 'replace-body', body: parseHex(editor.getValue()) }
            : paused.phase === 'request-head'
              ? { action: 'replace-request-head', head: JSON.parse(editor.getValue()) as unknown }
              : { action: 'replace-response-head', head: JSON.parse(editor.getValue()) as unknown };
          void this.submitBreakpoint(paused, action);
        } catch (error: unknown) {
          this.diagnostics.textContent = `Invalid replacement draft: ${describeError(error)}`;
        }
      });
      actions.append(continueButton, abortButton, replace);
      card.append(title, editorHost, actions);
      this.pausedList.append(card);
    }
  }

  async saveScript(): Promise<void> {
    try {
      const status = await invoke<ScriptStatus>('save_script', { draft: this.scriptDraft() });
      this.renderScriptStatus('Draft saved without activation.', status);
    } catch (error: unknown) {
      this.scriptFailure('Save failed', error);
    }
  }

  async validateScript(event?: Event): Promise<void> {
    event?.preventDefault();
    try {
      this.candidate = await invoke<ScriptCandidate>('validate_script', { draft: this.scriptDraft() });
      const model = this.sourceEditor?.getModel();
      if (model !== null && model !== undefined) {
        monaco.editor.setModelMarkers(model, 'transmog-rust', []);
      }
      this.scriptOutput.textContent = 'Script validated. Test it in the sandbox, then enable it for new requests.';
    } catch (error: unknown) {
      this.candidate = null;
      this.scriptFailure('Rust validation failed', error);
    }
  }

  async testScript(): Promise<void> {
    try {
      if (this.candidate === null) await this.validateScript();
      if (this.candidate === null) return;
      const draft = this.scriptDraft();
      const handler = draft.handlers[0];
      if (handler === undefined) throw new Error('export at least one supported handler');
      const responseHandler = handler.startsWith('onResponse');
      const bodyHandler = handler.endsWith('Body');
      const action = await invoke<Record<string, unknown>>('test_script', {
        candidateId: this.candidate.candidateId,
        invocation: {
          context: { exchangeId: 'editor-test', nowUnixMs: 0, handler },
          request: {
            method: 'GET',
            scheme: 'https',
            host: draft.prefilter.host ?? 'example.test',
            port: 443,
            path: draft.prefilter.pathPrefix ?? '/',
            query: null,
            headers: [{ name: 'User-Agent', value: Array.from(new TextEncoder().encode('Transmog editor test')), sensitive: false }],
          },
          response: responseHandler ? { status: 200, headers: [] } : null,
          body: bodyHandler ? { bytes: Array.from(new TextEncoder().encode('editor test body')), truncated: false } : null,
        },
      });
      this.scriptOutput.textContent = `Sandbox test passed with action “${String(action.action ?? 'continue')}”. No traffic was changed.`;
    } catch (error: unknown) {
      this.scriptFailure('Sandbox test aborted', error);
    }
  }

  async activateScript(): Promise<void> {
    try {
      if (this.candidate === null) await this.validateScript();
      if (this.candidate === null) return;
      const status = await invoke<ScriptStatus>('activate_script', { candidateId: this.candidate.candidateId });
      this.candidate = null;
      this.renderScriptStatus('Validated script enabled for new requests.', status);
      this.scriptRevision += 1;
    } catch (error: unknown) {
      this.scriptFailure('Activation failed', error);
    }
  }

  async disableScript(): Promise<void> {
    try {
      const status = await invoke<ScriptStatus>('disable_script', { scriptId: 'user-script' });
      this.renderScriptStatus('Script disabled for new exchanges.', status);
    } catch (error: unknown) {
      this.scriptFailure('Disable failed', error);
    }
  }

  async compareActiveScript(): Promise<void> {
    try {
      const status = await invoke<ScriptStatus>('script_status');
      const draft = this.scriptDraft();
      const active = status.active.find((revision) => revision.manifest.id === draft.id);
      if (active === undefined) throw new Error('this script is not currently enabled');
      this.revisionDiff?.dispose();
      for (const model of this.diffModels) model.dispose();
      this.diffModels = [
        monaco.editor.createModel(active.source, 'typescript'),
        monaco.editor.createModel(draft.source, 'typescript'),
      ];
      this.scriptDiffEditor.hidden = false;
      this.revisionDiff = monaco.editor.createDiffEditor(this.scriptDiffEditor, {
        automaticLayout: true,
        accessibilitySupport: 'on',
        ariaLabel: 'Enabled script and current draft comparison',
        readOnly: true,
        theme: this.monacoTheme(),
      });
      const [original, modified] = this.diffModels;
      if (original === undefined || modified === undefined) throw new Error('script comparison could not be prepared');
      this.revisionDiff.setModel({ original, modified });
      this.scriptOutput.textContent = 'Comparing the active script with the current draft.';
    } catch (error: unknown) {
      this.scriptFailure('Script comparison unavailable', error);
    }
  }

  private async refreshScripts(): Promise<void> {
    try {
      this.renderScriptStatus('Script workspace loaded.', await invoke<ScriptStatus>('script_status'));
    } catch (error: unknown) {
      this.scriptFailure('Script workspace unavailable', error);
    }
  }

  private scriptDraft(): ScriptDraft {
    if (this.sourceEditor === null) throw new Error('script editor is still loading');
    const data = new FormData(this.scriptForm);
    const source = this.sourceEditor.getValue();
    const handlers = (['onRequestHead', 'onRequestBody', 'onResponseHead', 'onResponseBody'] as const)
      .filter((handler) => new RegExp(`\\bexport\\s+function\\s+${handler}\\b`).test(source));
    if (handlers.length === 0) throw new Error('source must export at least one supported synchronous handler');
    const writeHeaders = String(data.get('scriptHeaders') ?? '')
      .split(',')
      .map((value) => value.trim().toLowerCase())
      .filter((value) => value.length > 0);
    return {
      id: 'user-script',
      revision: this.scriptRevision,
      source,
      handlers,
      prefilter: { method: null, host: optionalText(data.get('scriptHost')), pathPrefix: optionalText(data.get('scriptPath')) },
      capabilities: {
        readSensitiveHeaders: data.get('sensitive') === 'on',
        readBodies: data.get('readBodies') === 'on',
        writeHeaders,
        writeBody: data.get('writeBody') === 'on',
        respond: data.get('respond') === 'on',
        abort: data.get('abort') === 'on',
      },
      limits: {
        maxInputBodyBytes: 1024 * 1024,
        maxOutputBytes: 1024 * 1024,
        maxLogBytes: 16 * 1024,
        maxHeapBytes: 64 * 1024 * 1024,
        maxDurationMs: 100,
      },
      priority: 0,
    };
  }

  private renderScriptStatus(message: string, status: ScriptStatus): void {
    this.scriptOutput.textContent = `${message} ${status.active.length} active script${status.active.length === 1 ? '' : 's'}; ${status.saved.length} saved draft${status.saved.length === 1 ? '' : 's'}.`;
  }

  private scriptFailure(prefix: string, error: unknown): void {
    const message = describeError(error);
    this.scriptOutput.textContent = `${prefix}: ${message}\n\nThe draft was not activated. Correct the source or capabilities, validate again, and rerun the sandbox test.`;
    const model = this.sourceEditor?.getModel();
    if (model !== null && model !== undefined) {
      monaco.editor.setModelMarkers(model, 'transmog-rust', [{
        severity: monaco.MarkerSeverity.Error,
        message,
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: 1,
        endColumn: Math.max(2, model.getLineMaxColumn(1)),
      }]);
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

  async activateUserAgentRule(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.automationForm);
    const encode = (value: string): number[] => Array.from(new TextEncoder().encode(value));
    const host = optionalText(data.get('host'));
    const path = optionalText(data.get('path'));
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const previous = current.rules.find((rule) => rule.id === 'desktop-user-agent');
      const rule = {
        id: 'desktop-user-agent',
        revision: (previous?.revision ?? 0) + 1,
        priority: 0,
        matcher: {
          method: null,
          scheme: null,
          host,
          port: null,
          pathPrefix: path,
          query: null,
          requestHeaders: [],
          responseHeaders: [],
          responseStatus: null,
          responseStatusClass: null,
        },
        request: {
          headers: [{
            operation: 'set',
            field: { name: encode('User-Agent'), value: encode(String(data.get('userAgent') ?? '')) },
          }],
          replaceBody: null,
          discardBody: false,
          abortReason: null,
          responseAsset: null,
          allowNonIdempotentBodyReplacement: false,
        },
        response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
      };
      const rules = current.rules.filter((candidate) => candidate.id !== rule.id);
      rules.push(rule);
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostics.textContent = 'User-Agent override enabled for new matching requests.';
    } catch (error: unknown) {
      this.automationOutput.textContent = `User-Agent override could not be enabled: ${describeError(error)}`;
      this.automationOutput.dataset.kind = 'error';
    }
  }

  async refreshAutomation(): Promise<void> {
    try {
      this.renderAutomation(await invoke<AutomationStatus>('automation_status'));
    } catch (error: unknown) {
      this.automationOutput.textContent = `Automation query failed: ${describeError(error)}`;
    }
  }

  async activateAutoResponseRule(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.autoResponseForm);
    try {
      let assetReference = String(data.get('responseSource') ?? '');
      if (assetReference.length === 0) {
        const suffix = crypto.randomUUID().replaceAll('-', '');
        const assetId = `desktop-response-${suffix}`;
        const asset = await invoke<ResponseAsset>('create_response_asset', {
          input: {
            id: assetId,
            revision: 1,
            status: Number(data.get('status') ?? 200),
            headers: [],
            body: Array.from(new TextEncoder().encode(String(data.get('body') ?? ''))),
            mediaType: optionalText(data.get('mediaType')),
          },
        });
        assetReference = `${asset.id}@${asset.revision}`;
        await this.refreshResponseAssets(assetReference);
      }
      const current = await invoke<AutomationStatus>('automation_status');
      const previous = current.rules.find((rule) => rule.id === 'desktop-auto-response');
      const rule = {
        id: 'desktop-auto-response',
        revision: (previous?.revision ?? 0) + 1,
        priority: 0,
        matcher: {
          method: null,
          scheme: null,
          host: optionalText(data.get('host')),
          port: null,
          pathPrefix: optionalText(data.get('path')),
          query: null,
          requestHeaders: [],
          responseHeaders: [],
          responseStatus: null,
          responseStatusClass: null,
        },
        request: {
          headers: [],
          replaceBody: null,
          discardBody: false,
          abortReason: null,
          responseAsset: assetReference,
          allowNonIdempotentBodyReplacement: false,
        },
        response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
      };
      const rules = current.rules.filter((candidate) => candidate.id !== rule.id);
      rules.push(rule);
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostics.textContent = 'Auto-response enabled; new matching requests will not contact the origin.';
    } catch (error: unknown) {
      this.automationOutput.textContent = `Auto-response could not be enabled: ${describeError(error)}`;
      this.automationOutput.dataset.kind = 'error';
    }
  }

  private async refreshResponseAssets(selected?: string): Promise<void> {
    try {
      const assets = await invoke<ResponseAsset[]>('response_assets');
      const current = selected ?? this.responseAssetSelect.value;
      this.responseAssetSelect.replaceChildren();
      const create = document.createElement('option');
      create.value = '';
      create.textContent = 'Create a new response below';
      this.responseAssetSelect.append(create);
      assets.forEach((asset, index) => {
        const option = document.createElement('option');
        option.value = `${asset.id}@${asset.revision}`;
        option.textContent = `Saved response ${index + 1} · ${asset.status} · ${asset.bodyBytes} bytes`;
        this.responseAssetSelect.append(option);
      });
      this.responseAssetSelect.value = current;
    } catch (error: unknown) {
      this.automationOutput.textContent = `Saved responses are unavailable: ${describeError(error)}`;
      this.automationOutput.dataset.kind = 'error';
    }
  }

  async disableBuiltInAutomation(id: string): Promise<void> {
    try {
      const current = await invoke<AutomationStatus>('automation_status');
      const rules = current.rules.filter((rule) => rule.id !== id);
      if (rules.length === current.rules.length) {
        this.renderAutomation(current);
        return;
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostics.textContent = 'Traffic action disabled for new requests.';
    } catch (error: unknown) {
      this.automationOutput.textContent = `Traffic action could not be disabled: ${describeError(error)}`;
      this.automationOutput.dataset.kind = 'error';
    }
  }

  private async activateRuleSet(
    rules: AutomationStatus['rules'],
    generation: number,
  ): Promise<AutomationStatus> {
    const candidate = await invoke<AutomationCandidate>('validate_automation', {
      document: { schemaVersion: 1, generation, rules },
    });
    return invoke<AutomationStatus>('activate_automation', { candidateId: candidate.candidateId });
  }

  private renderAutomation(status: AutomationStatus): void {
    const actions: string[] = [];
    if (status.rules.some((rule) => rule.id === 'desktop-user-agent')) actions.push('User-Agent override');
    if (status.rules.some((rule) => rule.id === 'desktop-auto-response')) actions.push('Auto-response');
    const other = status.rules.filter((rule) => !rule.id.startsWith('desktop-')).length;
    if (other > 0) actions.push(`${other} advanced rule${other === 1 ? '' : 's'}`);
    this.automationOutput.textContent = actions.length === 0
      ? 'No built-in traffic actions are active.'
      : `Active for new requests: ${actions.join(', ')}.`;
    this.automationOutput.dataset.kind = 'success';
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
      this.populateSettings(state);
      this.applyTheme(state.preferences.theme);
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
      state.preferences.configureSystemProxy = true;
      state.privacy.retainResponseBodies = data.get('defaultBodies') === 'on';
      state.privacy.retainBodySamples = data.get('captureBodies') === 'on';
      state.privacy.rememberRecentArtifacts = data.get('rememberArtifacts') === 'on';
      state.privacy.includePathsInSupportBundles = data.get('supportPaths') === 'on';
      const saved = await invoke<ProductState>('save_product_state', { productState: state });
      this.populateSettings(saved);
      this.applyTheme(saved.preferences.theme);
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
      this.renderAppStatus(status);
    } catch (error: unknown) {
      this.diagnostics.textContent = `Status unavailable: ${describeError(error)}`;
    }
  }

  private renderAppStatus(status: AppStatus): void {
    this.statusLabel.textContent = lifecycleLabel(status.lifecycle);
    this.listenerValue.textContent = status.listener ?? 'Not listening';
    this.diagnostics.textContent = status.hostRestorePending
      ? 'Host restoration is pending and must be retried before restart.'
      : status.summary;
    this.shadowRoot?.querySelector('.status')?.setAttribute('data-lifecycle', status.lifecycle);
  }

  private populateSettings(state: ProductState): void {
    const elements = this.settingsForm.elements;
    (elements.namedItem('theme') as HTMLSelectElement).value = state.preferences.theme;
    (elements.namedItem('pageSize') as HTMLInputElement).value = String(state.preferences.sessionPageSize);
    this.sessionLimit = state.preferences.sessionPageSize;
    (elements.namedItem('defaultBodies') as HTMLInputElement).checked = state.privacy.retainResponseBodies;
    (elements.namedItem('captureBodies') as HTMLInputElement).checked = state.privacy.retainBodySamples;
    (elements.namedItem('rememberArtifacts') as HTMLInputElement).checked = state.privacy.rememberRecentArtifacts;
    (elements.namedItem('supportPaths') as HTMLInputElement).checked = state.privacy.includePathsInSupportBundles;
  }

  private applyTheme(theme: ProductState['preferences']['theme']): void {
    this.dataset.theme = theme;
    this.applyMonacoTheme(theme);
  }

  private applyMonacoTheme(theme: ProductState['preferences']['theme']): void {
    const dark = theme === 'dark' || (theme === 'system' && this.systemTheme.matches);
    monaco.editor.setTheme(dark ? 'vs-dark' : 'vs');
  }

  private monacoTheme(): 'vs' | 'vs-dark' {
    const theme = this.dataset.theme as ProductState['preferences']['theme'] | undefined;
    return theme === 'dark' || ((theme === undefined || theme === 'system') && this.systemTheme.matches)
      ? 'vs-dark'
      : 'vs';
  }

  private setProxyOutput(message: string, kind: 'progress' | 'success' | 'error'): void {
    this.proxyOutput.textContent = message;
    this.proxyOutput.dataset.kind = kind;
  }

  private showNotice(
    title: string,
    message: string,
    actionLabel: string | null,
    action: 'setup-ca' | 'start-proxy' | 'settings' | 'recover-proxy' | null,
  ): void {
    this.noticeTitle.textContent = title;
    this.noticeMessage.textContent = message;
    this.noticeAction = action;
    this.noticeActionButton.hidden = actionLabel === null;
    this.noticeActionButton.textContent = actionLabel ?? '';
    this.notice.hidden = false;
  }

  runNoticeAction(): void {
    const action = this.noticeAction;
    this.dismissNotice();
    if (action === 'setup-ca') void this.setupCa();
    else if (action === 'start-proxy') void this.startProxy();
    else if (action === 'settings') this.activateView('settings');
    else if (action === 'recover-proxy') void this.recoverProxy();
  }

  dismissNotice(): void {
    this.noticeAction = null;
    this.notice.hidden = true;
  }

  private async reportFrontendIssue(code: string, error: unknown): Promise<void> {
    try {
      await invoke<void>('record_frontend_diagnostic', { code, message: describeError(error) });
    } catch {
      // The startup log remains useful even when the IPC bridge itself failed.
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
  if (typeof error === 'object' && error !== null) {
    const message = (error as {message?: unknown}).message;
    if (typeof message === 'string') return message;
    try {
      return JSON.stringify(error).slice(0, 512);
    } catch {
      return 'unserializable failure';
    }
  }
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
