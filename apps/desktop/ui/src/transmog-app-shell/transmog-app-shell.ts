import { WebUIElement } from '@microsoft/webui-framework';
import { Channel, invoke } from '@tauri-apps/api/core';
import { ModuleKind, ModuleResolutionKind, ScriptTarget, monaco, typescriptDefaults } from '../monaco.js';

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
  rules: Array<{id: string; revision: number; priority: number}>;
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
  proxyForm!: HTMLFormElement;
  caThumbprint!: HTMLInputElement;
  filterForm!: HTMLFormElement;
  sessionRows!: HTMLTableSectionElement;
  nextButton!: HTMLButtonElement;
  inspectorOutput!: HTMLPreElement;
  bodyBoundary!: HTMLSelectElement;
  bodyRepresentation!: HTMLSelectElement;
  bodyDecoded!: HTMLInputElement;
  bodyMaxBytes!: HTMLInputElement;
  bodyPreviewOutput!: HTMLPreElement;
  bodyImagePreview!: HTMLImageElement;
  pausedList!: HTMLDivElement;
  breakpointForm!: HTMLFormElement;
  automationForm!: HTMLFormElement;
  responseAssetForm!: HTMLFormElement;
  autoResponseForm!: HTMLFormElement;
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
  private nextCursor: string | null = null;
  private watching = false;
  private selectedSessionId: string | null = null;
  private sourceEditor: monaco.editor.IStandaloneCodeEditor | null = null;
  private revisionDiff: monaco.editor.IStandaloneDiffEditor | null = null;
  private diffModels: monaco.editor.ITextModel[] = [];
  private breakpointEditors: monaco.editor.IStandaloneCodeEditor[] = [];
  private candidate: ScriptCandidate | null = null;

  connectedCallback(): void {
    super.connectedCallback();
    queueMicrotask(() => { void this.initializeScriptEditor(); });
  }

  disconnectedCallback(): void {
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
      theme: 'vs-dark',
    });
    this.sourceEditor.onDidChangeModelContent(() => { this.candidate = null; });
    this.scriptOutput.textContent = 'Ready. Monaco diagnostics are advisory; Rust validation is authoritative.';
    await this.refreshScripts();
  }

  private ensureMonacoStyles(): void {
    if (this.shadowRoot?.querySelector('link[data-monaco]') !== null) return;
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = '/app.css';
    link.dataset.monaco = 'true';
    this.shadowRoot?.append(link);
  }

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
      this.selectedSessionId = id;
      this.inspectorOutput.textContent = JSON.stringify(detail, null, 2);
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
      this.inspectorOutput.textContent = `Inspector unavailable: ${describeError(error)}`;
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
        this.bodyPreviewOutput.textContent = `${summary}\nNormalized in an isolated decoder as ${inspection.previewMimeType ?? 'image/png'}. Active document content and metadata were discarded.`;
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
        theme: 'vs-dark',
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
      this.scriptOutput.textContent = `Validated ${this.candidate.scriptId}@${this.candidate.revision}\nSHA-256 ${this.candidate.sourceHash}\nCandidate ${this.candidate.candidateId}`;
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
      this.scriptOutput.textContent = `Sandbox test passed. No traffic was changed.\n${JSON.stringify(action, null, 2)}`;
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
      this.renderScriptStatus('Exact validated revision activated for new exchanges.', status);
    } catch (error: unknown) {
      this.scriptFailure('Activation failed', error);
    }
  }

  async disableScript(): Promise<void> {
    try {
      const data = new FormData(this.scriptForm);
      const status = await invoke<ScriptStatus>('disable_script', { scriptId: String(data.get('scriptId') ?? '') });
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
      if (active === undefined) throw new Error('this script has no active revision to compare');
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
        ariaLabel: 'Active revision and current draft comparison',
        readOnly: true,
        theme: 'vs-dark',
      });
      const [original, modified] = this.diffModels;
      if (original === undefined || modified === undefined) throw new Error('revision comparison models were not created');
      this.revisionDiff.setModel({ original, modified });
      this.scriptOutput.textContent = `Comparing active ${active.manifest.id}@${active.manifest.revision} with the current draft.`;
    } catch (error: unknown) {
      this.scriptFailure('Revision comparison unavailable', error);
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
      id: String(data.get('scriptId') ?? ''),
      revision: Number(data.get('scriptRevision') ?? 1),
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
      priority: Number(data.get('scriptPriority') ?? 0),
    };
  }

  private renderScriptStatus(message: string, status: ScriptStatus): void {
    this.scriptOutput.textContent = `${message}\n\n${JSON.stringify(status, null, 2)}`;
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
    const document = {
      schemaVersion: 1,
      generation: 0,
      rules: [{
        id: String(data.get('ruleId') ?? ''),
        revision: Number(data.get('revision') ?? 1),
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
      }],
    };
    try {
      const candidate = await invoke<AutomationCandidate>('validate_automation', { document });
      const status = await invoke<AutomationStatus>('activate_automation', { candidateId: candidate.candidateId });
      this.renderAutomation(status);
      this.diagnostics.textContent = `Activated ${candidate.ruleCount} native rule with ${candidate.registrationCount} attributed hook registration.`;
    } catch (error: unknown) {
      this.automationOutput.textContent = `Activation failed: ${describeError(error)}`;
    }
  }

  async refreshAutomation(): Promise<void> {
    try {
      this.renderAutomation(await invoke<AutomationStatus>('automation_status'));
    } catch (error: unknown) {
      this.automationOutput.textContent = `Automation query failed: ${describeError(error)}`;
    }
  }

  async createResponseAsset(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.responseAssetForm);
    try {
      const asset = await invoke<ResponseAsset>('create_response_asset', {
        input: {
          id: String(data.get('assetId') ?? ''),
          revision: Number(data.get('assetRevision') ?? 1),
          status: Number(data.get('status') ?? 200),
          headers: [],
          body: Array.from(new TextEncoder().encode(String(data.get('body') ?? ''))),
          mediaType: optionalText(data.get('mediaType')),
        },
      });
      this.automationOutput.textContent = JSON.stringify(asset, null, 2);
      this.diagnostics.textContent = `Created immutable response asset ${asset.id}@${asset.revision}.`;
    } catch (error: unknown) {
      this.automationOutput.textContent = `Asset creation failed: ${describeError(error)}`;
    }
  }

  async activateAutoResponseRule(event: Event): Promise<void> {
    event.preventDefault();
    const data = new FormData(this.autoResponseForm);
    const document = {
      schemaVersion: 1,
      generation: 0,
      rules: [{
        id: String(data.get('ruleId') ?? ''),
        revision: Number(data.get('revision') ?? 1),
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
          responseAsset: String(data.get('assetRef') ?? ''),
          allowNonIdempotentBodyReplacement: false,
        },
        response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
      }],
    };
    try {
      const candidate = await invoke<AutomationCandidate>('validate_automation', { document });
      const status = await invoke<AutomationStatus>('activate_automation', { candidateId: candidate.candidateId });
      this.renderAutomation(status);
      this.diagnostics.textContent = 'Activated head-based autoresponse; matching traffic will not contact the origin.';
    } catch (error: unknown) {
      this.automationOutput.textContent = `Autoresponse activation failed: ${describeError(error)}`;
    }
  }

  private renderAutomation(status: AutomationStatus): void {
    this.automationOutput.textContent = JSON.stringify(status, null, 2);
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
      (elements.namedItem('defaultBodies') as HTMLInputElement).checked = state.privacy.retainResponseBodies;
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
      state.privacy.retainResponseBodies = data.get('defaultBodies') === 'on';
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
