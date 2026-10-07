import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import { WorkspaceElement } from '../workspace-element.js';
import type { ScriptDraft, ScriptStatus, ScriptCandidate } from '../models.js';
import { describeError, optionalText } from '../utilities.js';

import type * as Monaco from 'monaco-editor/editor/editor.api';
import { loadMonaco, waitForStyles } from '../editor-runtime.js';

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


export class ScriptEditor extends WorkspaceElement {
  @attr theme = 'system';
  @attr({mode:'boolean'}) active = false;
  @observable scriptText = initialState.scriptText;
  @observable editorLoaded = initialState.editorLoaded;
  @observable diffVisible = initialState.diffVisible;
  monacoStyles!: HTMLLinkElement;
  scriptForm!: HTMLFormElement;
  scriptEditor!: HTMLDivElement;
  scriptDiffEditor!: HTMLDivElement;
  private sourceEditor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private draftModel: Monaco.editor.ITextModel | null = null;
  private revisionDiff: Monaco.editor.IStandaloneDiffEditor | null = null;
  private diffModels: Monaco.editor.ITextModel[] = [];
  private candidate: ScriptCandidate | null = null;
  private scriptRevision = Date.now();
  private loading: Promise<void> | null = null;
  private monaco: typeof Monaco | null = null;
  private declarationsDisposable: Monaco.IDisposable | null = null;
  private readonly systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
  private readonly systemThemeChanged = (): void => { this.themeChanged(); };
  protected hydratedCallback(): void {
    this.systemTheme.addEventListener('change',this.systemThemeChanged);
    if (this.active) void this.ensureEditor();
  }
  activeChanged(): void { if (this.active) void this.ensureEditor(); }
  themeChanged(): void { this.monaco?.editor.setTheme(this.monacoTheme()); }
  private monacoTheme(): 'vs' | 'vs-dark' { return this.theme === 'dark' || (this.theme === 'system' && this.systemTheme.matches) ? 'vs-dark' : 'vs'; }
  private async ensureEditor(): Promise<void> {
    if (this.loading) return this.loading;
    this.loading = (async () => {
      try { this.monaco = (await loadMonaco()).monaco; await this.initializeScriptEditor(); }
      catch (error: unknown) {
        this.sourceEditor?.dispose(); this.sourceEditor = null;
        this.draftModel?.dispose(); this.draftModel = null;
        this.scriptText = `Script editor initialization failed: ${describeError(error)}`;
        await this.reportFrontendIssue('script-editor-initialization-failed',error);
      }
    })();
    try { await this.loading; } finally { this.loading = null; }
  }
  disconnectedCallback(): void {
    this.systemTheme.removeEventListener('change',this.systemThemeChanged);
    this.sourceEditor?.dispose(); this.sourceEditor = null;
    this.draftModel?.dispose(); this.draftModel = null;
    this.revisionDiff?.dispose(); this.revisionDiff = null;
    for (const model of this.diffModels) model.dispose(); this.diffModels = [];
    this.declarationsDisposable?.dispose(); this.declarationsDisposable = null;
    super.disconnectedCallback();
  }
  private async initializeScriptEditor(): Promise<void> {
    if (this.sourceEditor !== null || this.scriptEditor === undefined) return;
    const [{ ModuleKind, ModuleResolutionKind, ScriptTarget, typescriptDefaults }, declarations] = await Promise.all([loadMonaco(), invoke<string>('script_declarations')]);
    if (!this.isConnected) return;
    this.editorLoaded = true;
    this.$flushUpdates();
    await waitForStyles(this.monacoStyles);
    if (!this.isConnected) return;
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
    this.declarationsDisposable = typescriptDefaults.addExtraLib(
      declarations,
      'file:///transmog-script-api/v1.d.ts',
    );
    const model = this.monaco!.editor.createModel(SCRIPT_TEMPLATE, 'typescript', this.monaco!.Uri.parse('file:///transmog-scripts/draft/main.ts'));
    this.draftModel = model;
    this.sourceEditor = this.monaco!.editor.create(this.scriptEditor, {
      model,
      automaticLayout: true,
      accessibilitySupport: 'on',
      ariaLabel: 'Traffic script TypeScript source',
      minimap: { enabled: false },
      tabFocusMode: true,
      theme: this.monacoTheme(),
    });
    this.sourceEditor.onDidChangeModelContent(() => { this.candidate = null; });
    this.scriptText = 'Ready. Monaco diagnostics are advisory; Rust validation is authoritative.';
    await this.refreshScripts();
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
        this.monaco!.editor.setModelMarkers(model, 'transmog-rust', []);
      }
      this.scriptText = 'Script validated. Test it in the sandbox, then enable it for new requests.';
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
      this.scriptText = `Sandbox test passed with action “${String(action.action ?? 'continue')}”. No traffic was changed.`;
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
        this.monaco!.editor.createModel(active.source, 'typescript'),
        this.monaco!.editor.createModel(draft.source, 'typescript'),
      ];
      this.diffVisible = true;
      this.$flushUpdates();
      this.revisionDiff = this.monaco!.editor.createDiffEditor(this.scriptDiffEditor, {
        automaticLayout: true,
        accessibilitySupport: 'on',
        ariaLabel: 'Enabled script and current draft comparison',
        readOnly: true,
        theme: this.monacoTheme(),
      });
      const [original, modified] = this.diffModels;
      if (original === undefined || modified === undefined) throw new Error('script comparison could not be prepared');
      this.revisionDiff.setModel({ original, modified });
      this.scriptText = 'Comparing the active script with the current draft.';
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
    this.scriptText = `${message} ${status.active.length} active script${status.active.length === 1 ? '' : 's'}; ${status.saved.length} saved draft${status.saved.length === 1 ? '' : 's'}.`;
  }

  private scriptFailure(prefix: string, error: unknown): void {
    const message = describeError(error);
    this.scriptText = `${prefix}: ${message}\n\nThe draft was not activated. Correct the source or capabilities, validate again, and rerun the sandbox test.`;
    const model = this.sourceEditor?.getModel();
    if (model !== null && model !== undefined) {
      this.monaco!.editor.setModelMarkers(model, 'transmog-rust', [{
        severity: this.monaco!.MarkerSeverity.Error,
        message,
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: 1,
        endColumn: Math.max(2, model.getLineMaxColumn(1)),
      }]);
    }
  }

}

ScriptEditor.define('script-editor');
