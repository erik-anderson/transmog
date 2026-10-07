import initialState from '../initial-state.json';
import { attr, observable } from '@microsoft/webui-framework';
import type * as Monaco from 'monaco-editor/editor/editor.api';
import { WorkspaceElement } from '../workspace-element.js';
import type { PausedExchange } from '../models.js';
import { loadMonaco, waitForStyles } from '../editor-runtime.js';
import { describeError, parseHex } from '../utilities.js';

export class PausedExchangeElement extends WorkspaceElement {
  @attr theme = 'system';
  @observable paused: PausedExchange | null = initialState.paused;
  @observable draftText = initialState.draftText;
  @observable editorLoaded = initialState.editorLoaded;
  @observable editorReady = initialState.editorReady;
  @observable editorError = initialState.editorError;
  monacoStyles!: HTMLLinkElement;
  editorHost!: HTMLDivElement;
  private editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private loading: Promise<void> | null = null;
  private monaco: typeof Monaco | null = null;
  private model: Monaco.editor.ITextModel | null = null;
  private visibility: IntersectionObserver | null = null;
  private readonly systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
  private readonly systemThemeChanged = (): void => { this.themeChanged(); };

  protected hydratedCallback(): void {
    this.systemTheme.addEventListener('change', this.systemThemeChanged);
    // Client-created rows mount eagerly; allocating an editor still waits for visibility.
    this.visibility = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      this.visibility?.disconnect();
      this.visibility = null;
      void this.ensureEditor();
    });
    this.visibility.observe(this);
  }

  pausedChanged(previous: PausedExchange | undefined, paused: PausedExchange | null): void {
    if (paused && paused.decisionId !== previous?.decisionId) {
      this.draftText = paused.bodyHex ?? JSON.stringify(paused.requestHead ?? paused.responseHead, null, 2);
      this.editor?.setValue(this.draftText);
    }
  }

  themeChanged(): void { this.monaco?.editor.setTheme(this.editorTheme()); }
  private editorTheme(): 'vs' | 'vs-dark' {
    return this.theme === 'dark' || (this.theme === 'system' && this.systemTheme.matches) ? 'vs-dark' : 'vs';
  }
  updateDraft(event: Event): void { this.draftText = (event.currentTarget as HTMLTextAreaElement).value; }

  private async ensureEditor(): Promise<void> {
    if (this.editor || this.loading) return this.loading ?? undefined;
    this.loading = (async () => {
      try {
        this.monaco = (await loadMonaco()).monaco;
        if (!this.isConnected) return;
        this.editorLoaded = true;
        this.$flushUpdates();
        await waitForStyles(this.monacoStyles);
        if (!this.isConnected || !this.paused) return;
        this.editorReady = true;
        this.$flushUpdates();
        this.model = this.monaco.editor.createModel(this.draftText, this.paused.phase.endsWith('body') ? 'plaintext' : 'json');
        this.editor = this.monaco.editor.create(this.editorHost, {
          model: this.model,
          automaticLayout: true,
          accessibilitySupport: 'on',
          ariaLabel: 'Edit paused ' + this.paused.phase,
          minimap: { enabled: false },
          tabFocusMode: true,
          theme: this.editorTheme(),
        });
      } catch (error: unknown) {
        this.model?.dispose();
        this.model = null;
        this.editorReady = false;
        this.editorError = 'Editor unavailable: ' + describeError(error) + '. The replacement draft remains editable.';
      }
    })();
    try { await this.loading; } finally { this.loading = null; }
  }
  decide(action: 'continue' | 'abort'): void {
    this.submit(action === 'continue' ? { action } : { action, reason: 'aborted by Transmog operator' });
  }
  replaceDraft(): void {
    if (!this.paused) return;
    try {
      const value = this.editor?.getValue() ?? this.draftText;
      this.submit(this.paused.phase.endsWith('body')
        ? { action: 'replace-body', body: parseHex(value) }
        : { action: this.paused.phase === 'request-head' ? 'replace-request-head' : 'replace-response-head', head: JSON.parse(value) as unknown });
    } catch (error: unknown) { this.editorError = 'Invalid replacement draft: ' + describeError(error); }
  }
  private submit(action: object): void {
    if (this.paused) this.$emit('breakpoint-decision', { paused: this.paused, action });
  }
  disconnectedCallback(): void {
    this.systemTheme.removeEventListener('change', this.systemThemeChanged);
    this.visibility?.disconnect();
    this.visibility = null;
    this.editor?.dispose();
    this.model?.dispose();
    this.model = null;
    this.editor = null;
    super.disconnectedCallback();
  }
}

PausedExchangeElement.define('paused-exchange');
