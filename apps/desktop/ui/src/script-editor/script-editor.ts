import "../draft-status/draft-status.js";
import initialState from "../initial-state.json";
import "../editor-surface/editor-surface.js";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import type {
  ScriptDraft,
  ScriptParameter,
  ScriptStatus,
  ScriptCandidate,
  ScriptLanguage,
  ScriptRuntimeStatus,
  SelectedResponse,
} from "../models.js";
import { describeError, optionalText, parseHeaderLines } from "../utilities.js";
import {
  activeDraft,
  editingDraft,
  scriptFingerprint,
  scriptExecutionFingerprint,
  scriptSettings,
} from "./script-state.js";
import type { EditorSurface } from "../editor-surface/editor-surface.js";
import type * as Monaco from "monaco-editor/editor/editor.api";
import { loadMonaco, waitForStyles } from "../editor-runtime.js";

interface ScriptHelpSection {
  id: string;
  title: string;
  paragraphs: string[];
  phases: Array<{ handler: string; input: string; decisions: string }>;
  examples: Array<{ title: string; settings: string; code: string }>;
}
interface ScriptHelp {
  apiRevision: number;
  failureCategories: string[];
  sections: ScriptHelpSection[];
}
function trafficScope(draft: ScriptDraft): string {
  const filters = draft.prefilter;
  const scope = [
    filters.method || "all methods",
    filters.host || "all hosts",
    filters.pathPrefix ? `path starts with ${filters.pathPrefix}` : "all paths",
  ];
  if (draft.handlers.includes("onRequestBody")) {
    scope.push(
      `request bodies: ${draft.requestBodyFilter.contentTypes.join(", ") || "all media types"}`,
    );
  }
  if (draft.handlers.includes("onResponseBody")) {
    const body = draft.responseBodyFilter;
    scope.push(
      `response bodies: ${body.contentTypes.join(", ") || "all media types"}; ${body.statuses.join(", ") || "all statuses"}; ${body.includeLocal ? "including" : "excluding"} local responses`,
    );
  }
  return scope.join(" · ");
}
interface ParameterRow {
  id: number;
  name: string;
  type: "string" | "boolean" | "integer";
  label: string;
  defaultValue: string;
  override: string;
  useDefault: boolean;
  choices: string;
  min: string;
  max: string;
}
interface ValidatedDraft {
  token: ScriptCandidate;
  draft: ScriptDraft;
  version: number;
}

export class ScriptEditor extends WorkspaceElement {
  @attr theme = "system";
  @attr({ mode: "boolean" }) active = false;
  @observable selection: SelectedResponse | null = null;
  @observable apiRevision = 0;
  @observable failureCategories: string[] = [];
  @observable helpSections: ScriptHelpSection[] = [];
  @observable helpSearch = "";
  @observable draftScope = "Loading filters…";
  private allHelpSections: ScriptHelpSection[] = [];
  scriptHelpDialog!: HTMLDialogElement;
  scriptHelpButton!: HTMLButtonElement;
  scriptHelpSearch!: HTMLInputElement;
  scriptHelpContent!: HTMLDivElement;
  @observable language: ScriptLanguage = "typescript";
  @observable reviewingTypeScript = false;
  @observable authoringStatus = "";
  @observable buildInfo = "No successful build yet.";
  @observable sourceAlignment = "";
  private lastBuild: ScriptStatus["built"][number] | null = null;
  @observable sourceLabel = "TypeScript source";
  @observable diffLabel = "Active (left) and draft (right)";
  @observable returnReady = false;
  @observable canResume = false;
  @observable runtimeIssue = "";
  @observable appliedBuildText = "No applied build.";
  @observable historyEntries: Array<{ index: number; label: string; source: string }> = [];
  @observable canUndoHistoryRemoval = false;
  @observable historyOpen = false;
  @observable conflictVisible = false;
  @observable conflictReviewed = false;
  @observable canUndoSavedLoad = false;
  @observable typeScriptIssues: Array<{ line: number; column: number; message: string }> = [];
  @observable typeScriptIssue = false;
  private javascriptHistory: string[] = [];
  private removedHistorySource: string | null = null;
  private conflictSaved: ScriptDraft | null = null;
  private previousConflictDraft: ScriptDraft | null = null;
  private appliedRevision: number | null = null;
  private runtimeTimer: ReturnType<typeof setTimeout> | null = null;
  private returnToExpandedSource = false;
  @observable retainedSource: string | null = null;
  private reviewJavascript = "";
  private returnVersion = -1;
  private diagnosticsVersion = 0;
  @observable scriptBusy = false;
  @observable scriptReady = false;
  @observable scriptDirty = true;
  @observable scriptUnapplied = true;
  @observable scriptActive = false;
  @observable activeScriptText = "Paused";
  @observable scriptText = initialState.scriptText;
  @observable statusKind = "";
  @observable editorLoaded = initialState.editorLoaded;
  @observable diffVisible = initialState.diffVisible;
  @observable diffStale = false;
  @observable activeSettings = "";
  @observable draftSettings = "";
  @observable testHandlers: Array<{ name: string }> = [{ name: "onRequestHead" }];
  @observable parameterRows: ParameterRow[] = [];
  private nextParameterId = 0;
  @observable settingsError = "";
  @observable testResult = "";
  @observable testEvidence = "";
  @observable testOutdated = false;
  scriptTestUrl!: HTMLInputElement;
  scriptTestMethod!: HTMLInputElement;
  scriptTestHeaders!: HTMLTextAreaElement;
  scriptTestBody!: HTMLTextAreaElement;
  scriptTestResponseHeaders!: HTMLTextAreaElement;
  scriptTestStatus!: HTMLInputElement;
  scriptTestLocal!: HTMLInputElement;
  scriptTestFailure!: HTMLSelectElement;
  scriptTestHandler!: HTMLSelectElement;
  scriptMenu!: HTMLDivElement;
  scriptMore!: HTMLButtonElement;
  monacoStyles!: HTMLLinkElement;
  scriptForm!: HTMLFormElement;
  scriptEditor!: HTMLDivElement;
  scriptDiffEditor!: HTMLDivElement;
  private editVersion = 0;
  private testVersion = 0;
  private savedFingerprint = "";
  private savedDraft: ScriptDraft | null = null;
  private initialDraft: ScriptDraft | null = null;
  private activeFingerprint = "";
  private loadedDraft: ScriptDraft | null = null;
  private sourceEditor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private draftModel: Monaco.editor.ITextModel | null = null;
  private revisionDiff: Monaco.editor.IStandaloneDiffEditor | null = null;
  private diffModels: Monaco.editor.ITextModel[] = [];
  private diffViewModel: Monaco.editor.IDiffEditorViewModel | null = null;
  private candidate: ValidatedDraft | null = null;
  private scriptRevision = 1;
  private loading: Promise<void> | null = null;
  private monaco: typeof Monaco | null = null;
  private declarationsDisposable: Monaco.IDisposable | null = null;
  private javascriptDeclarationsDisposable: Monaco.IDisposable | null = null;
  private readonly systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
  private readonly forcedColors = window.matchMedia("(forced-colors: active)");
  private readonly systemThemeChanged = (): void => {
    this.themeChanged();
  };

  openScriptHelp(): void {
    this.scriptHelpDialog.showModal();
    this.scriptHelpSearch.focus();
  }
  scriptHelpKeyboard(event: KeyboardEvent): void {
    event.stopPropagation();
    if (event.key === "Escape" && !event.isComposing) {
      event.preventDefault();
      this.closeScriptHelp();
    }
  }
  closeScriptHelp(): void {
    this.scriptHelpDialog.close();
  }
  scriptHelpClosed(): void {
    if (this.active && this.isConnected) {
      this.scriptHelpButton.focus();
    }
  }
  searchScriptHelp(event: Event): void {
    this.helpSearch = (event.target as HTMLInputElement).value;
    const terms = this.helpSearch.toLocaleLowerCase().split(/\s+/).filter(Boolean);
    this.helpSections = this.allHelpSections.filter((section) => {
      const text = [
        section.title,
        ...section.paragraphs,
        ...section.phases.flatMap((phase) => [phase.handler, phase.input, phase.decisions]),
        ...section.examples.flatMap((example) => [example.title, example.settings, example.code]),
        ...(section.id === "evidence" ? this.failureCategories : []),
      ];
      const searchable = text.join(" ").toLocaleLowerCase();
      return terms.every((term) => searchable.includes(term));
    });
    this.$flushUpdates();
    this.scriptHelpContent.scrollTop = 0;
  }
  markScriptChanged(): void {
    this.editVersion++;
    this.returnReady = false;
    this.conflictReviewed = false;
    this.typeScriptIssues = [];
    this.typeScriptIssue = false;
    this.candidate = null;
    this.testOutdated = Boolean(this.testResult);
    this.diffStale = this.diffVisible;
    const model = this.sourceEditor?.getModel();
    if (model) {
      this.monaco?.editor.setModelMarkers(model, "transmog-rust", []);
    }
    this.updateDraftState();
    void this.refreshAuthoringStatus();
  }
  markTestChanged(): void {
    this.testVersion++;
    this.testOutdated = Boolean(this.testResult);
  }
  private updateDraftState(): void {
    if (!this.scriptReady) {
      return;
    }
    let draft: ScriptDraft;
    try {
      draft = this.scriptDraft();
      this.settingsError = "";
    } catch (error) {
      this.settingsError = describeError(error);
      this.draftScope = "Unavailable until settings errors are resolved.";
      this.scriptDirty = true;
      this.scriptUnapplied = true;
      return;
    }
    this.draftScope = trafficScope(draft);
    const fingerprint = scriptFingerprint(draft);
    this.scriptDirty = fingerprint !== this.savedFingerprint;
    this.scriptUnapplied = scriptExecutionFingerprint(draft) !== this.activeFingerprint;
    this.testHandlers = draft.handlers.map((name) => ({ name }));
    const visibleSource = this.sourceEditor!.getValue();
    const visibleLanguage = this.reviewingTypeScript ? "typescript" : this.language;
    const label = visibleLanguage === "typescript" ? "TypeScript" : "JavaScript";
    const build = this.lastBuild;
    this.sourceAlignment = !build
      ? "No compiled build yet."
      : visibleLanguage === "typescript" && build.language !== "typescript"
        ? "Retained TypeScript has not been compared with the current compiled JavaScript."
        : visibleSource === (visibleLanguage === "javascript" ? build.javascript : build.source)
          ? `${label} source matches compiled build ${build.manifest.revision}.`
          : `${label} source differs from compiled build ${build.manifest.revision}.`;
  }
  scriptKeyboard(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      void this.saveScript();
    }
  }
  private async runScript(message: string, operation: () => Promise<void>): Promise<void> {
    if (this.scriptBusy || !this.scriptReady) {
      return;
    }
    this.scriptBusy = true;
    this.scriptText = message;
    this.statusKind = "progress";
    try {
      await operation();
    } finally {
      this.scriptBusy = false;
    }
  }
  async saveScript(): Promise<void> {
    if (!this.scriptDirty) {
      return;
    }
    await this.runScript("Saving draft…", () => this.saveDraft());
  }
  discardScript(): void {
    const draft = this.savedDraft ?? this.initialDraft;
    if (this.scriptBusy || !this.scriptReady || !draft) {
      return;
    }
    this.applyDraft(draft);
    this.feedback("Changes discarded. The applied script is unchanged.");
  }
  private applyDraft(draft: ScriptDraft): void {
    this.loadedDraft = structuredClone(draft);
    const set = (name: string, value: string | boolean) => {
      const input = this.scriptForm.elements.namedItem(name) as HTMLInputElement;
      if (typeof value === "boolean") {
        input.checked = value;
      } else {
        input.value = value;
      }
    };
    set("scriptHost", draft.prefilter.host ?? "");
    set("scriptMethod", draft.prefilter.method ?? "");
    set("scriptPath", draft.prefilter.pathPrefix ?? "");
    set("scriptHeaders", draft.capabilities.writeHeaders.join(", "));
    set("readBodies", draft.capabilities.readBodies);
    set("rewriteRequest", draft.capabilities.rewriteRequest);
    set("reroute", draft.capabilities.reroute);
    set("writeStatus", draft.capabilities.writeStatus);
    set("annotate", draft.capabilities.annotate);
    set("requestTypes", draft.requestBodyFilter.contentTypes.join(", "));
    set("responseTypes", draft.responseBodyFilter.contentTypes.join(", "));
    set("responseStatuses", draft.responseBodyFilter.statuses.join(", "));
    set("includeLocal", draft.responseBodyFilter.includeLocal);
    this.loadParameters(draft.parameters);
    set("writeBody", draft.capabilities.writeBody);
    set("respond", draft.capabilities.respond);
    set("abort", draft.capabilities.abort);
    set("sensitive", draft.capabilities.readSensitiveHeaders);
    this.reviewingTypeScript = false;
    this.language = draft.language ?? "typescript";
    this.retainedSource = draft.retainedSource ?? null;
    this.setJavaScriptHistory(draft.javascriptHistory ?? []);
    this.closeComparison();
    this.replaceSource(draft.source, this.language);
    this.markScriptChanged();
  }
  async validateScript(event?: Event): Promise<void> {
    event?.preventDefault();
    if (this.reviewingTypeScript) {
      return;
    }
    this.closeMenu();
    await this.runScript("Validating current source and capabilities…", async () => {
      await this.validateDraft();
    });
  }
  async testScript(): Promise<void> {
    if (this.reviewingTypeScript) {
      return;
    }
    await this.runScript("Checking source and running the sandbox…", () => this.testDraft());
  }
  async primaryAction(): Promise<void> {
    if (!this.scriptActive || this.scriptUnapplied) {
      await this.activateScript();
    }
  }
  async activateScript(): Promise<void> {
    await this.runScript("Validating and saving changes before activation…", () =>
      this.activateDraft(),
    );
  }
  async disableScript(): Promise<void> {
    await this.runScript("Pausing script…", () => this.disableActive());
  }
  async compareActiveScript(): Promise<void> {
    this.closeMenu();
    await this.runScript("Preparing comparison…", () => this.compareDraft());
  }
  private closeMenu(): void {
    if (this.scriptMenu?.matches(":popover-open")) {
      this.scriptMenu.hidePopover();
      this.scriptMore.focus();
    }
  }
  menuClosed(event: ToggleEvent): void {
    const root = this.getRootNode() as Document | ShadowRoot;
    if (event.newState === "closed" && this.scriptMenu.contains(root.activeElement)) {
      this.scriptMore.focus();
    }
  }
  closeComparison(): void {
    const diffSurface = this.scriptDiffEditor?.closest("editor-surface") as EditorSurface | null;
    const restoreSource = Boolean(diffSurface?.expanded && this.returnToExpandedSource);
    if (diffSurface?.expanded) {
      diffSurface.toggle();
    }
    this.returnToExpandedSource = false;
    this.returnReady = false;
    this.conflictReviewed = false;
    this.diffVisible = false;
    this.activeSettings = "";
    this.draftSettings = "";
    this.disposeComparison();
    const sourceSurface = this.scriptEditor?.closest("editor-surface") as EditorSurface | null;
    if (restoreSource && sourceSurface && !sourceSurface.expanded) {
      sourceSurface.toggle();
    }
    this.focusEditor();
  }
  private disposeComparison(): void {
    this.revisionDiff?.setModel(null);
    // Cancel asynchronous diff work before releasing either text model.
    this.diffViewModel?.dispose();
    this.diffViewModel = null;
    this.revisionDiff?.dispose();
    this.revisionDiff = null;
    for (const model of this.diffModels) {
      model.dispose();
    }
    this.diffModels = [];
  }
  useSelectedTestRequest(): void {
    const request = this.selection?.detail.requests.find(
      (head) => head.boundary === "client-request",
    );
    if (!request) {
      return;
    }
    this.scriptTestUrl.value = request.target ?? "";
    this.scriptTestMethod.value = request.method ?? "GET";
    this.scriptTestHeaders.value = request.headers
      .filter((header) => !header.sensitive)
      .map((header) => header.name + ": " + header.value)
      .join("\n");
    this.scriptTestBody.value = "";
    this.markTestChanged();
  }
  protected hydratedCallback(): void {
    this.systemTheme.addEventListener("change", this.systemThemeChanged);
    this.forcedColors.addEventListener("change", this.systemThemeChanged);
    if (this.active) {
      void this.ensureEditor();
    }
  }
  activeChanged(): void {
    if (this.active) {
      void this.ensureEditor();
      this.scheduleRuntimeRefresh();
    } else {
      this.scriptHelpDialog?.close();
      this.stopRuntimeRefresh();
    }
  }
  focusEditor(): void {
    this.sourceEditor?.layout();
    this.sourceEditor?.focus();
  }
  themeChanged(): void {
    this.monaco?.editor.setTheme(this.monacoTheme());
  }
  private monacoTheme(): "vs" | "vs-dark" | "hc-black" | "hc-light" {
    const dark = this.theme === "dark" || (this.theme === "system" && this.systemTheme.matches);
    return this.forcedColors.matches ? (dark ? "hc-black" : "hc-light") : dark ? "vs-dark" : "vs";
  }
  private async ensureEditor(): Promise<void> {
    if (this.loading) {
      return this.loading;
    }
    if (this.sourceEditor) {
      return;
    }
    this.loading = (async () => {
      try {
        this.monaco = (await loadMonaco()).monaco;
        await this.initializeScriptEditor();
      } catch (error: unknown) {
        this.sourceEditor?.dispose();
        this.sourceEditor = null;
        this.draftModel?.dispose();
        this.draftModel = null;
        this.scriptText = `Script editor could not open: ${describeError(error)}`;
        this.statusKind = "error";
        await this.reportFrontendIssue("script-editor-initialization-failed", error);
      }
    })();
    try {
      await this.loading;
    } finally {
      this.loading = null;
    }
  }
  disconnectedCallback(): void {
    this.scriptHelpDialog?.close();
    this.editVersion++;
    this.testVersion++;
    this.scriptReady = false;
    this.stopRuntimeRefresh();
    this.systemTheme.removeEventListener("change", this.systemThemeChanged);
    this.forcedColors.removeEventListener("change", this.systemThemeChanged);
    this.sourceEditor?.dispose();
    this.sourceEditor = null;
    this.draftModel?.dispose();
    this.draftModel = null;
    this.disposeComparison();
    this.declarationsDisposable?.dispose();
    this.declarationsDisposable = null;
    this.javascriptDeclarationsDisposable?.dispose();
    this.javascriptDeclarationsDisposable = null;
    super.disconnectedCallback();
  }
  private async initializeScriptEditor(): Promise<void> {
    if (this.sourceEditor !== null || this.scriptEditor === undefined) {
      return;
    }
    const [
      { ModuleKind, ModuleResolutionKind, ScriptTarget, typescriptDefaults, javascriptDefaults },
      declarations,
      status,
      help,
    ] = await Promise.all([
      loadMonaco(),
      invoke<string>("script_declarations"),
      invoke<ScriptStatus>("script_status"),
      invoke<ScriptHelp>("script_help"),
    ]);
    if (!this.isConnected) {
      return;
    }
    this.apiRevision = help.apiRevision;
    this.failureCategories = help.failureCategories;
    this.helpSections = help.sections;
    this.allHelpSections = help.sections;
    this.editorLoaded = true;
    this.$flushUpdates();
    await waitForStyles(this.monacoStyles);
    if (!this.isConnected) {
      return;
    }
    typescriptDefaults.setCompilerOptions({
      allowNonTsExtensions: true,
      module: ModuleKind.ESNext,
      moduleResolution: ModuleResolutionKind.NodeJs,
      noEmit: true,
      strict: true,
      lib: ["esnext"],
      target: ScriptTarget.ESNext,
    });
    typescriptDefaults.setDiagnosticsOptions({
      noSemanticValidation: false,
      noSyntaxValidation: false,
    });
    this.declarationsDisposable = typescriptDefaults.addExtraLib(
      declarations,
      `file:///transmog-script-api/v${help.apiRevision}.d.ts`,
    );
    javascriptDefaults.setCompilerOptions({
      ...typescriptDefaults.getCompilerOptions(),
      allowJs: true,
      checkJs: false,
    });
    this.javascriptDeclarationsDisposable?.dispose();
    this.javascriptDeclarationsDisposable = javascriptDefaults.addExtraLib(
      declarations,
      `file:///transmog-script-api/v${help.apiRevision}.d.ts`,
    );
    const draft = editingDraft(status);
    this.loadedDraft = draft ?? null;
    this.language = draft?.language ?? "typescript";
    this.retainedSource = draft?.retainedSource ?? null;
    this.setJavaScriptHistory(draft?.javascriptHistory ?? []);
    this.conflictVisible = false;
    this.sourceLabel = this.language === "typescript" ? "TypeScript source" : "JavaScript source";
    this.draftModel = this.monaco!.editor.createModel(
      draft?.source ??
        help.sections.find((section) => section.id === "examples")!.examples[0]!.code,
      draft?.language ?? "typescript",
      this.monaco!.Uri.parse(
        `file:///transmog-scripts/draft/main.${draft?.language === "javascript" ? "js" : "ts"}`,
      ),
    );
    this.sourceEditor = this.monaco!.editor.create(this.scriptEditor, {
      model: this.draftModel,
      automaticLayout: true,
      accessibilitySupport: "on",
      ariaLabel: `Traffic script ${this.language === "typescript" ? "TypeScript" : "JavaScript"} source`,
      minimap: { enabled: false },
      tabFocusMode: true,
      theme: this.monacoTheme(),
    });
    if (draft) {
      this.scriptRevision = draft.revision;
      const set = (name: string, value: string | boolean) => {
        const input = this.scriptForm.elements.namedItem(name) as HTMLInputElement;
        if (typeof value === "boolean") {
          input.checked = value;
        } else {
          input.value = value;
        }
      };
      set("scriptHost", draft.prefilter.host ?? "");
      set("scriptMethod", draft.prefilter.method ?? "");
      set("scriptPath", draft.prefilter.pathPrefix ?? "");
      set("scriptHeaders", draft.capabilities.writeHeaders.join(", "));
      set("readBodies", draft.capabilities.readBodies);
      set("rewriteRequest", draft.capabilities.rewriteRequest);
      set("reroute", draft.capabilities.reroute);
      set("writeStatus", draft.capabilities.writeStatus);
      set("annotate", draft.capabilities.annotate);
      set("requestTypes", draft.requestBodyFilter.contentTypes.join(", "));
      set("responseTypes", draft.responseBodyFilter.contentTypes.join(", "));
      set("responseStatuses", draft.responseBodyFilter.statuses.join(", "));
      set("includeLocal", draft.responseBodyFilter.includeLocal);
      this.loadParameters(draft.parameters);
      set("writeBody", draft.capabilities.writeBody);
      set("respond", draft.capabilities.respond);
      set("abort", draft.capabilities.abort);
      set("sensitive", draft.capabilities.readSensitiveHeaders);
      this.savedFingerprint = scriptFingerprint(draft);
    }
    this.sourceEditor.onDidChangeModelContent(() => this.markScriptChanged());
    this.scriptReady = true;
    this.initialDraft = structuredClone(this.scriptDraft());
    this.savedDraft = draft ? structuredClone(draft) : null;
    this.$flushUpdates();
    this.renderScriptStatus(status);
    this.updateDraftState();
    void this.refreshAuthoringStatus();
    this.scheduleRuntimeRefresh();
    this.scriptText = draft
      ? "Ready to edit. Changes apply to new requests when saved and enabled."
      : "Start with this example, then save and enable it for new requests.";
  }
  private async saveDraft(): Promise<void> {
    const version = this.editVersion;
    try {
      const { draft, generation } = await this.prepareDraft();
      if (!this.current(version)) {
        return this.changedDuring("saving");
      }
      const status = await invoke<ScriptStatus>("save_script", {
        draft,
        expectedGeneration: generation,
      });
      if (!this.isConnected) {
        return;
      }
      this.savedDraft = structuredClone(draft);
      this.savedFingerprint = scriptFingerprint(draft);
      this.candidate = null;
      this.renderScriptStatus(status);
      this.updateDraftState();
      this.feedback(
        this.current(version)
          ? "Draft saved. The active script is unchanged."
          : "Earlier edits were saved. Newer edits are still unsaved.",
      );
    } catch (error: unknown) {
      this.scriptFailure("Save failed", error);
      if (!this.conflictVisible) {
        await this.showError("Script draft could not be saved", this.scriptText);
      }
    }
  }
  private current(version: number): boolean {
    return this.isConnected && version === this.editVersion;
  }
  private changedDuring(operation: string): void {
    this.feedback(
      `The draft changed during ${operation}. Run the action again to use the current edits.`,
    );
  }
  private async validateDraft(): Promise<ValidatedDraft | null> {
    const version = this.editVersion;
    try {
      await this.checkTypeScript();
      const { draft, generation } = await this.prepareDraft();
      if (!this.current(version)) {
        this.changedDuring("validation");
        return null;
      }
      const token = await invoke<ScriptCandidate>("validate_script", {
        draft,
        expectedGeneration: generation,
      });
      if (!this.current(version)) {
        this.changedDuring("validation");
        return null;
      }
      this.candidate = { token, draft, version };
      const status = await invoke<ScriptStatus>("script_status");
      if (!this.current(version)) {
        this.candidate = null;
        this.changedDuring("validation");
        return null;
      }
      this.renderScriptStatus(status);
      this.updateDraftState();
      const model = this.sourceEditor?.getModel();
      if (model) {
        this.monaco!.editor.setModelMarkers(model, "transmog-rust", []);
      }
      this.feedback(
        "Current source and capabilities are valid. Save & enable applies them; sandbox testing is optional.",
      );
      return this.candidate;
    } catch (error: unknown) {
      if (this.current(version)) {
        this.scriptFailure("Validation failed", error);
      } else {
        this.changedDuring("validation");
      }
      return null;
    }
  }
  private async testDraft(): Promise<void> {
    const testVersion = this.testVersion;
    const version = this.editVersion;
    this.testOutdated = Boolean(this.testResult);
    const input = {
      url: this.scriptTestUrl.value,
      method: this.scriptTestMethod.value,
      headers: this.scriptTestHeaders.value,
      body: this.scriptTestBody.value,
      responseHeaders: this.scriptTestResponseHeaders.value,
      status: Number(this.scriptTestStatus.value),
      localResponse: this.scriptTestLocal.checked,
      failure: this.scriptTestFailure.value,
      handler: this.scriptTestHandler.value,
    };
    try {
      const candidate = this.candidate ?? (await this.validateDraft());
      if (!candidate || !this.current(version)) {
        return;
      }
      const handler =
        candidate.draft.handlers.find((name) => name === input.handler) ??
        candidate.draft.handlers[0];
      const target = new URL(input.url);
      if (!["http:", "https:"].includes(target.protocol)) {
        throw new Error("Use an HTTP or HTTPS test URL");
      }
      if (!handler) {
        throw new Error("Export at least one supported handler");
      }
      const hasResponse = handler.startsWith("onResponse") || handler === "onCompleted";
      this.scriptText = "Running the sandbox for this input…";
      this.statusKind = "progress";
      const result = await invoke<{ decision: { action: string } }>("test_script", {
        candidateId: candidate.token.candidateId,
        invocation: {
          context: { exchangeId: "editor-test", nowUnixMs: Date.now(), handler },
          parameters: {},
          state: {},
          localResponse: input.localResponse,
          failure: handler === "onFailed" ? input.failure : null,
          request: {
            method: input.method,
            scheme: target.protocol.slice(0, -1),
            host: target.hostname,
            port: Number(target.port || (target.protocol === "https:" ? 443 : 80)),
            path: target.pathname,
            query: target.search ? target.search.slice(1) : null,
            headers: parseHeaderLines(input.headers).map((header) => ({
              name: header.name,
              value: Array.from(new TextEncoder().encode(header.value)),
              sensitive: false,
              redacted: false,
            })),
          },
          response: hasResponse
            ? {
                status: input.status,
                headers: parseHeaderLines(input.responseHeaders).map((h) => ({
                  name: h.name,
                  value: Array.from(new TextEncoder().encode(h.value)),
                  sensitive: false,
                  redacted: false,
                })),
              }
            : null,
          body: handler.endsWith("Body")
            ? { bytes: Array.from(new TextEncoder().encode(input.body)) }
            : null,
        },
      });
      if (!this.isConnected) {
        return;
      }
      this.testOutdated = !this.current(version) || testVersion !== this.testVersion;
      this.testResult = `Callback completed · ${handler} · Action: ${result.decision.action} · Filters bypassed`;
      this.testEvidence = [
        `Handler: ${handler}`,
        `${input.method} ${input.url}`,
        `Local response: ${input.localResponse ? "yes" : "no"}`,
        ...(handler === "onFailed" ? [`Failure category: ${input.failure}`] : []),
        ...(hasResponse ? [`Response fixture: ${input.status}\n${input.responseHeaders}`] : []),
        ...(input.headers ? [`Request headers:\n${input.headers}`] : []),
        ...(handler.endsWith("Body") ? [`Input body:\n${input.body}`] : []),
        `\nReturned result:\n${JSON.stringify(result, null, 2)}`,
      ].join("\n");
      if (this.testEvidence.length > 32768) {
        this.testEvidence =
          this.testEvidence.slice(0, 32768) + "\n[Evidence preview shortened to 32,768 characters]";
      }
      this.feedback(
        this.testOutdated
          ? "Sandbox completed for earlier edits or input. Run it again to check the current version."
          : "Callback completed with empty exchange state. Filters were bypassed. No request was sent; the script was not enabled.",
      );
    } catch (error: unknown) {
      if (this.current(version) && testVersion === this.testVersion) {
        this.testResult = "Sandbox did not complete for this input";
        this.testEvidence = describeError(error);
        this.testOutdated = false;
        this.scriptFailure("Sandbox test aborted", error);
      } else {
        this.feedback(
          "The earlier sandbox test failed after its draft or input changed. Test the current version.",
        );
      }
    }
  }
  private async activateDraft(): Promise<void> {
    if (this.reviewingTypeScript) {
      return;
    }
    const version = this.editVersion;
    try {
      // Always revalidate against the current workspace generation, including after Save draft.
      const candidate = await this.validateDraft();
      if (!candidate || !this.current(version)) {
        return;
      }
      this.scriptText = "Starting the script and saving this version…";
      this.statusKind = "progress";
      await this.checkTypeScript();
      if (!this.current(version)) {
        this.changedDuring("activation");
        return;
      }
      const status = await invoke<ScriptStatus>("activate_script", {
        candidateId: candidate.token.candidateId,
      });
      if (!this.isConnected) {
        return;
      }
      this.candidate = null;
      this.savedDraft = structuredClone(candidate.draft);
      this.savedFingerprint = scriptFingerprint(candidate.draft);
      this.scriptRevision = candidate.draft.revision + 1;
      this.renderScriptStatus(status);
      this.updateDraftState();
      this.feedback(
        this.current(version)
          ? "Saved and enabled for new requests."
          : "The validated version was saved and enabled. Newer edits are still unsaved and unapplied.",
      );
    } catch (error: unknown) {
      this.scriptFailure("Activation failed", error);
    }
  }
  private async disableActive(): Promise<void> {
    try {
      const status = await invoke<ScriptStatus>("disable_script", { scriptId: "user-script" });
      if (!this.isConnected) {
        return;
      }
      this.candidate = null;
      this.renderScriptStatus(status);
      this.updateDraftState();
      this.feedback("Script paused for new requests. Your draft is preserved.");
    } catch (error: unknown) {
      this.scriptFailure("Pause failed", error);
    }
  }
  private async compareDraft(): Promise<void> {
    const version = this.editVersion;
    try {
      const draft = this.scriptDraft();
      const status = await invoke<ScriptStatus>("script_status");
      if (!this.current(version)) {
        this.changedDuring("comparison");
        return;
      }
      const active = status.active.find((revision) => revision.manifest.id === draft.id);
      if (!active) {
        throw new Error("This script is not currently enabled");
      }
      await this.showSourceComparison(
        activeDraft(active),
        draft,
        "Active (left) and draft (right)",
      );
      this.feedback("Comparing active source and settings with the current draft.");
    } catch (error: unknown) {
      this.scriptFailure("Script comparison unavailable", error);
    }
  }
  private async showSourceComparison(
    originalDraft: ScriptDraft,
    draft: ScriptDraft,
    label: string,
  ): Promise<boolean> {
    if (!this.active || !this.isConnected) {
      return false;
    }
    this.disposeComparison();
    this.diffLabel = label;
    this.diffModels = [
      this.monaco!.editor.createModel(originalDraft.source, originalDraft.language),
      this.monaco!.editor.createModel(draft.source, draft.language),
    ];
    this.activeSettings = scriptSettings(originalDraft);
    this.draftSettings = scriptSettings(draft);
    this.diffStale = false;
    this.diffVisible = true;
    this.$flushUpdates();
    this.revisionDiff = this.monaco!.editor.createDiffEditor(this.scriptDiffEditor, {
      automaticLayout: true,
      accessibilitySupport: "on",
      ariaLabel: label,
      readOnly: true,
      theme: this.monacoTheme(),
    });
    const [original, modified] = this.diffModels;
    if (!original || !modified) {
      throw new Error("Script comparison could not be prepared");
    }
    this.diffViewModel = this.revisionDiff.createViewModel({ original, modified });
    const viewModel = this.diffViewModel;
    this.revisionDiff.setModel(viewModel);
    this.revealComparison();
    await viewModel.waitForDiff();
    return this.isConnected && this.diffViewModel === viewModel && this.diffVisible;
  }
  private async prepareDraft(): Promise<{ draft: ScriptDraft; generation: number }> {
    const draft = this.scriptDraft();
    const status = await invoke<ScriptStatus>("script_status");
    this.requireSavedBaseline(status);
    const revisions = [
      ...status.saved.filter((item) => item.id === draft.id).map((item) => item.revision),
      ...[...status.active, ...(status.paused ?? [])]
        .filter((item) => item.manifest.id === draft.id)
        .map((item) => item.manifest.revision + 1),
    ];
    const revision = Math.max(this.scriptRevision, ...revisions);
    if (!Number.isSafeInteger(revision)) {
      throw new Error("Script revision limit reached");
    }
    this.scriptRevision = revision;
    return { draft: { ...draft, revision }, generation: status.generation };
  }
  private csv(value: FormDataEntryValue | null): string[] {
    return String(value ?? "")
      .split(",")
      .map((v) => v.trim())
      .filter(Boolean);
  }
  private integer(value: string, label: string): number {
    if (!/^-?\d+$/.test(value) || !Number.isSafeInteger(Number(value))) {
      throw new Error(`${label} must be a whole number`);
    }
    return Number(value);
  }
  addParameter(): void {
    if (this.parameterRows.length >= 64) {
      return;
    }
    this.parameterRows = [
      ...this.parameterRows,
      {
        id: this.nextParameterId++,
        name: "",
        label: "",
        type: "string",
        defaultValue: "",
        override: "",
        useDefault: true,
        choices: "",
        min: "0",
        max: "100",
      },
    ];
    this.markScriptChanged();
  }
  removeParameter(id: number): void {
    this.parameterRows = this.parameterRows.filter((p) => p.id !== id);
    this.markScriptChanged();
  }
  changeParameter(id: number, field: string, event: Event): void {
    const input = event.target as HTMLInputElement;
    const value = field === "useDefault" ? input.checked : input.value;
    this.parameterRows = this.parameterRows.map((p) =>
      p.id !== id
        ? p
        : {
            ...p,
            [field]: value,
            ...(field === "type"
              ? {
                  defaultValue: value === "boolean" ? "false" : value === "integer" ? "0" : "",
                  override: "",
                  useDefault: true,
                }
              : {}),
          },
    );
    this.markScriptChanged();
  }
  private loadParameters(parameters: Record<string, ScriptParameter>): void {
    this.parameterRows = Object.entries(parameters).map(([name, p]) => ({
      id: this.nextParameterId++,
      name,
      label: p.label,
      type: p.type,
      defaultValue: String(p.default),
      override: p.value === null ? "" : String(p.value),
      useDefault: p.value === null,
      choices: p.type === "string" ? p.choices.join(", ") : "",
      min: p.type === "integer" ? String(p.min) : "0",
      max: p.type === "integer" ? String(p.max) : "100",
    }));
  }
  private parameters(): Record<string, ScriptParameter> {
    const entries = new Map<string, ScriptParameter>();
    for (const p of this.parameterRows) {
      if (!/^[A-Za-z0-9_]{1,64}$/.test(p.name) || entries.has(p.name)) {
        throw new Error("Parameters need unique names using letters, numbers, or underscores");
      }
      const label = p.label || p.name;
      const item: ScriptParameter =
        p.type === "boolean"
          ? {
              type: p.type,
              label,
              default: p.defaultValue === "true",
              value: p.override === "" ? null : p.override === "true",
            }
          : p.type === "integer"
            ? {
                type: p.type,
                label,
                default: this.integer(p.defaultValue, p.name),
                value: p.useDefault ? null : this.integer(p.override, p.name),
                min: this.integer(p.min, "Minimum"),
                max: this.integer(p.max, "Maximum"),
              }
            : {
                type: p.type,
                label,
                default: p.defaultValue,
                value: p.useDefault ? null : p.override,
                choices: this.csv(p.choices),
              };
      entries.set(p.name, item);
    }
    return Object.fromEntries(entries);
  }
  private scriptDraft(): ScriptDraft {
    if (!this.sourceEditor) {
      throw new Error("Script editor is still loading");
    }
    const data = new FormData(this.scriptForm);
    const source = this.reviewingTypeScript ? this.reviewJavascript : this.sourceEditor.getValue();
    const handlers = this.sourceHandlers(source);
    const writeHeaders = [
      ...new Set(
        String(data.get("scriptHeaders") ?? "")
          .split(",")
          .map((value) => value.trim().toLowerCase())
          .filter((value) => value.length > 0),
      ),
    ];
    return {
      id: "user-script",
      revision: this.scriptRevision,
      source,
      language: this.language,
      retainedSource: this.reviewingTypeScript ? this.sourceEditor.getValue() : this.retainedSource,
      javascriptHistory: [...this.javascriptHistory],
      handlers,
      parameters: this.parameters(),
      requestBodyFilter: {
        contentTypes: this.csv(data.get("requestTypes")),
        statuses: [],
        includeLocal: false,
      },
      responseBodyFilter: {
        contentTypes: this.csv(data.get("responseTypes")),
        statuses: this.csv(data.get("responseStatuses")).map((v) =>
          this.integer(v, "Response status"),
        ),
        includeLocal: data.get("includeLocal") === "on",
      },
      prefilter: {
        method: optionalText(data.get("scriptMethod")),
        host: optionalText(data.get("scriptHost")),
        pathPrefix: optionalText(data.get("scriptPath")),
      },
      capabilities: {
        rewriteRequest: data.get("rewriteRequest") === "on",
        reroute: data.get("reroute") === "on",
        writeStatus: data.get("writeStatus") === "on",
        annotate: data.get("annotate") === "on",
        readSensitiveHeaders: data.get("sensitive") === "on",
        readBodies: data.get("readBodies") === "on",
        writeHeaders,
        writeBody: data.get("writeBody") === "on",
        respond: data.get("respond") === "on",
        abort: data.get("abort") === "on",
      },
      limits: this.loadedDraft?.limits ?? {
        maxInputBodyBytes: 1024 * 1024,
        maxOutputBytes: 1024 * 1024,
        maxLogBytes: 16 * 1024,
        maxHeapBytes: 64 * 1024 * 1024,
        maxDurationMs: 100,
      },
      priority: this.loadedDraft?.priority ?? 0,
    };
  }
  private renderScriptStatus(status: ScriptStatus): void {
    this.lastBuild = status.built?.find((item) => item.manifest.id === "user-script") ?? null;
    const applied =
      status.active.find((item) => item.manifest.id === "user-script") ??
      status.paused?.find((item) => item.manifest.id === "user-script");
    const buildIsApplied =
      applied &&
      this.lastBuild?.manifest.revision === applied.manifest.revision &&
      this.lastBuild?.javascriptHash === applied.javascriptHash;
    this.buildInfo = this.lastBuild
      ? `Compiled build ${this.lastBuild.manifest.revision}${buildIsApplied ? " (applied)" : " (not applied)"}\nLanguage: ${this.lastBuild.language}\nCompiler: ${this.lastBuild.compiler}\nSource SHA-256: ${this.lastBuild.manifest.sourceHash}\nJavaScript SHA-256: ${this.lastBuild.javascriptHash}\nAPI declarations SHA-256: ${this.lastBuild.declarationsHash}`
      : "No successful build yet.";
    this.canResume = Boolean(status.paused?.some((item) => item.manifest.id === "user-script"));
    this.$emit("script-activity-change", {
      generation: status.generation,
      activeCount: status.active.length,
    });
    const active = status.active.find((item) => item.manifest.id === "user-script");
    this.scriptActive = Boolean(active);
    this.appliedRevision = active?.manifest.revision ?? null;
    this.activeScriptText = active
      ? `On - applied build ${active.manifest.revision}`
      : applied
        ? `Paused - applied build ${applied.manifest.revision}`
        : "Paused";
    this.appliedBuildText = applied
      ? `Applied build ${applied.manifest.revision}${active ? " is enabled" : " is paused"} · ${trafficScope(activeDraft(applied))}`
      : "No applied build.";
    this.updateRuntimeStatus(status.runtimes ?? []);
    const fingerprint = active ? scriptExecutionFingerprint(activeDraft(active)) : "";
    if (this.diffVisible && fingerprint !== this.activeFingerprint) {
      this.diffStale = true;
    }
    this.activeFingerprint = fingerprint;
    void this.refreshAuthoringStatus();
  }
  private sourceHandlers(source: string): ScriptDraft["handlers"] {
    return (
      [
        "onRequestHead",
        "onRequestBody",
        "onResponseHead",
        "onResponseBody",
        "onCompleted",
        "onFailed",
      ] as const
    ).filter((handler) => new RegExp(`\\bexport\\s+function\\s+${handler}\\b`).test(source));
  }
  private replaceSource(source: string, language: ScriptLanguage): void {
    const old = this.draftModel;
    this.sourceEditor!.setModel(null);
    old?.dispose();
    this.draftModel = this.monaco!.editor.createModel(
      source,
      language,
      this.monaco!.Uri.parse(
        `file:///transmog-scripts/draft/main.${language === "typescript" ? "ts" : "js"}`,
      ),
    );
    this.sourceEditor!.setModel(this.draftModel);
    this.sourceEditor!.updateOptions({
      ariaLabel: this.reviewingTypeScript
        ? "Retained TypeScript source for review"
        : `Traffic script ${language === "typescript" ? "TypeScript" : "JavaScript"} source`,
    });
    this.sourceLabel = this.reviewingTypeScript
      ? "Retained TypeScript for review"
      : language === "typescript"
        ? "TypeScript source"
        : "JavaScript source";
    this.markScriptChanged();
  }
  private async checkTypeScript(): Promise<void> {
    if (this.language !== "typescript" && !this.reviewingTypeScript) {
      return;
    }
    const model = this.draftModel!;
    const version = model.getVersionId();
    const { getTypeScriptWorker } = await loadMonaco();
    const check = async () => {
      // Monaco registers rich language services after attaching the first model.
      // Publishing can be requested before that asynchronous registration finishes.
      const started = performance.now();
      let createWorker: Awaited<ReturnType<typeof getTypeScriptWorker>>;
      for (;;) {
        if (model.isDisposed() || model !== this.draftModel || model.getVersionId() !== version) {
          throw new Error(
            "Source changed during TypeScript checking. Retry with the current draft.",
          );
        }
        try {
          createWorker = await getTypeScriptWorker();
          break;
        } catch (error: unknown) {
          if (describeError(error) !== "TypeScript not registered!") {
            throw error;
          }
          if (performance.now() - started >= 5000) {
            throw new Error(
              "TypeScript is still starting. Check TypeScript again before publishing.",
              { cause: error },
            );
          }
          await new Promise<void>((resolve) => setTimeout(resolve, 50));
        }
      }
      const worker = await createWorker(model.uri);
      const diagnostics = [
        ...(await worker.getSyntacticDiagnostics(model.uri.toString())),
        ...(await worker.getSemanticDiagnostics(model.uri.toString())),
      ];
      if (model.isDisposed() || model !== this.draftModel || model.getVersionId() !== version) {
        throw new Error("Source changed during TypeScript checking. Retry with the current draft.");
      }
      const errors = diagnostics.filter((item) => item.category === 1);
      this.typeScriptIssues = errors.slice(0, 20).map((item) => {
        const position = model.getPositionAt(item.start ?? 0);
        return {
          line: position.lineNumber,
          column: position.column,
          message: this.diagnosticMessage(item.messageText),
        };
      });
      this.typeScriptIssue = errors.length > 0;
      if (errors.length) {
        throw new Error(
          `TypeScript has ${errors.length} error(s). Choose an issue below to fix it, or ${this.reviewingTypeScript ? "Keep JavaScript" : "Edit JavaScript"}.`,
        );
      }
    };
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        check(),
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () =>
              reject(
                new Error(
                  `TypeScript checking is unavailable. Retry, or ${this.reviewingTypeScript ? "Keep JavaScript" : "Edit JavaScript"}.`,
                ),
              ),
            15000,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  private async refreshAuthoringStatus(): Promise<void> {
    const request = ++this.diagnosticsVersion;
    const version = this.editVersion;
    if (!this.scriptReady) {
      return;
    }
    if (this.language === "javascript" && !this.reviewingTypeScript) {
      this.authoringStatus =
        "Editing JavaScript. Retained TypeScript is used again only by your explicit choice.";
      return;
    }
    this.authoringStatus = "Checking TypeScript…";
    await new Promise<void>((resolve) => setTimeout(resolve, 250));
    if (request !== this.diagnosticsVersion || !this.current(version)) {
      return;
    }
    try {
      await this.checkTypeScript();
      if (request === this.diagnosticsVersion && this.current(version)) {
        this.authoringStatus = "TypeScript checks passed.";
      }
    } catch (error: unknown) {
      if (request === this.diagnosticsVersion && this.current(version)) {
        this.typeScriptIssue = true;
        this.authoringStatus = `${describeError(error)} ${this.runtimeIssue ? "The applied script is unavailable; see the runtime issue." : this.appliedBuildText}`;
      }
    }
  }
  async editJavaScript(): Promise<void> {
    this.closeMenu();
    await this.runScript("Opening JavaScript draft…", async () => {
      const version = this.editVersion;
      try {
        const status = await invoke<ScriptStatus>("script_status");
        if (!this.current(version)) {
          return this.changedDuring("language switching");
        }
        const build =
          status.built?.find((item) => item.manifest.id === "user-script") ??
          status.active.find((item) => item.manifest.id === "user-script");
        const seed =
          build?.javascript ??
          'export function onRequestHead() { return { decision: { action: "continue" } }; }\n';
        const history =
          this.retainedSource && this.retainedSource !== seed
            ? this.rememberJavaScript(this.retainedSource)
            : this.javascriptHistory;
        this.setJavaScriptHistory(history);
        this.retainedSource = this.sourceEditor!.getValue();
        this.language = "javascript";
        this.closeComparison();
        this.replaceSource(
          build?.javascript ??
            'export function onRequestHead() { return { decision: { action: "continue" } }; }\n',
          "javascript",
        );
        this.feedback(
          build
            ? `Editing JavaScript from compiled build ${build.manifest.revision}${[...status.active, ...(status.paused ?? [])].some((item) => item.manifest.id === "user-script" && item.manifest.revision === build.manifest.revision && item.javascriptHash === build.javascriptHash) ? " (applied)" : " (not applied)"}. TypeScript edits and previous JavaScript drafts are retained. ${this.appliedBuildText}`
            : "No successful build exists. A JavaScript starter is ready; your TypeScript is retained.",
        );
      } catch (error: unknown) {
        this.scriptFailure("Language switch failed", error);
      }
    });
  }
  reviewTypeScript(): void {
    this.closeMenu();
    if (
      this.scriptBusy ||
      this.reviewingTypeScript ||
      this.language !== "javascript" ||
      this.retainedSource === null
    ) {
      return;
    }
    this.reviewJavascript = this.sourceEditor!.getValue();
    this.reviewingTypeScript = true;
    this.closeComparison();
    this.replaceSource(this.retainedSource, "typescript");
    this.feedback(
      "Repair retained TypeScript if needed, then compare its generated JavaScript with your current JavaScript.",
    );
  }
  keepJavaScript(): void {
    if (this.scriptBusy) {
      return;
    }
    this.retainedSource = this.sourceEditor!.getValue();
    this.reviewingTypeScript = false;
    this.closeComparison();
    this.replaceSource(this.reviewJavascript, "javascript");
    this.feedback("Continuing with JavaScript. TypeScript edits are retained.");
  }
  async compareTypeScriptOutput(): Promise<void> {
    await this.runScript(
      "Checking retained TypeScript and preparing output comparison…",
      async () => {
        const version = this.editVersion;
        try {
          await this.checkTypeScript();
          const { draft, generation } = await this.prepareDraft();
          if (!this.current(version)) {
            return this.changedDuring("comparison");
          }
          const source = this.sourceEditor!.getValue();
          await invoke<ScriptCandidate>("validate_script", {
            draft: {
              ...draft,
              source,
              handlers: this.sourceHandlers(source),
              language: "typescript",
              retainedSource: this.reviewJavascript,
            },
            expectedGeneration: generation,
          });
          const status = await invoke<ScriptStatus>("script_status");
          if (!this.current(version)) {
            return this.changedDuring("comparison");
          }
          const build = status.built.find((item) => item.manifest.id === draft.id);
          if (!build || build.source !== source) {
            throw new Error("The build changed. Compare again.");
          }
          this.renderScriptStatus(status);
          this.updateDraftState();
          const ready = await this.showJavascriptComparison(
            this.reviewJavascript,
            build.javascript,
            "Current JavaScript (left) and TypeScript output (right)",
          );
          if (!ready || !this.current(version)) {
            return;
          }
          this.returnVersion = version;
          this.returnReady = true;
          this.feedback(
            "Review the JavaScript differences, then explicitly choose Use TypeScript output.",
          );
        } catch (error: unknown) {
          if (this.current(version)) {
            this.scriptFailure("Comparison failed", error);
          }
        }
      },
    );
  }
  useTypeScriptOutput(): void {
    if (!this.returnReady || this.returnVersion !== this.editVersion || this.scriptBusy) {
      return;
    }
    this.retainedSource = this.reviewJavascript;
    this.reviewingTypeScript = false;
    this.language = "typescript";
    this.sourceLabel = "TypeScript source";
    this.sourceEditor!.updateOptions({ ariaLabel: "Traffic script TypeScript source" });
    this.closeComparison();
    this.markScriptChanged();
    this.feedback(
      "TypeScript selected. Save and apply to publish its output; the previous JavaScript draft is retained.",
    );
  }
  private revealComparison(): void {
    if (!this.active || !this.isConnected || !this.diffVisible) {
      return;
    }
    const source = this.scriptEditor.closest("editor-surface") as EditorSurface;
    const diff = this.scriptDiffEditor.closest("editor-surface") as EditorSurface;
    if (source.expanded) {
      source.toggle();
      this.returnToExpandedSource = true;
      if (!diff.expanded) {
        diff.toggle();
      }
    } else if (!diff.expanded) {
      diff.scrollIntoView({ block: "nearest" });
    }
    this.revisionDiff?.getOriginalEditor().focus();
  }
  private async showJavascriptComparison(
    original: string,
    modified: string,
    label: string,
  ): Promise<boolean> {
    if (!this.active || !this.isConnected) {
      return false;
    }
    this.returnReady = false;
    this.disposeComparison();
    this.diffLabel = label;
    this.activeSettings = "";
    this.draftSettings = "";
    this.diffModels = [
      this.monaco!.editor.createModel(original, "javascript"),
      this.monaco!.editor.createModel(modified, "javascript"),
    ];
    this.diffStale = false;
    this.diffVisible = true;
    this.$flushUpdates();
    this.revisionDiff = this.monaco!.editor.createDiffEditor(this.scriptDiffEditor, {
      automaticLayout: true,
      accessibilitySupport: "on",
      ariaLabel: label,
      readOnly: true,
      theme: this.monacoTheme(),
    });
    const viewModel = this.revisionDiff.createViewModel({
      original: this.diffModels[0]!,
      modified: this.diffModels[1]!,
    });
    this.diffViewModel = viewModel;
    this.revisionDiff.setModel(viewModel);
    this.revealComparison();
    await viewModel.waitForDiff();
    return this.isConnected && this.diffViewModel === viewModel && this.diffVisible;
  }
  async viewAppliedJavaScript(): Promise<void> {
    this.closeMenu();
    await this.runScript("Opening applied JavaScript...", async () => {
      const version = this.editVersion;
      try {
        const status = await invoke<ScriptStatus>("script_status");
        if (!this.current(version)) {
          return;
        }
        const applied =
          status.active.find((item) => item.manifest.id === "user-script") ??
          status.paused?.find((item) => item.manifest.id === "user-script");
        if (!applied) {
          throw new Error("No applied JavaScript is available.");
        }
        await this.showJavascriptComparison(
          applied.javascript,
          applied.javascript,
          `Applied JavaScript - build ${applied.manifest.revision}`,
        );
      } catch (error: unknown) {
        if (this.current(version)) {
          this.scriptFailure("Applied JavaScript unavailable", error);
        }
      }
    });
  }
  async resumeScript(): Promise<void> {
    await this.runScript("Resuming applied JavaScript…", async () => {
      try {
        const status = await invoke<ScriptStatus>("script_status");
        const resumed = await invoke<ScriptStatus>("resume_script", {
          scriptId: "user-script",
          expectedGeneration: status.generation,
        });
        if (!this.isConnected) {
          return;
        }
        this.renderScriptStatus(resumed);
        this.updateDraftState();
        this.feedback("Previously applied JavaScript resumed. Draft edits remain unapplied.");
      } catch (error: unknown) {
        this.scriptFailure("Resume failed", error);
      }
    });
  }
  private requireSavedBaseline(status: ScriptStatus): void {
    const saved = editingDraft(status);
    if (
      (saved ? scriptFingerprint(saved) : "") !==
      (this.savedDraft ? scriptFingerprint(this.savedDraft) : "")
    ) {
      this.conflictSaved = saved ? structuredClone(saved) : null;
      this.conflictReviewed = false;
      this.conflictVisible = true;
      throw new Error(
        "The saved script changed in another window. Compare the saved script before replacing it.",
      );
    }
  }
  async compareSavedScript(): Promise<void> {
    await this.runScript("Comparing your draft with the saved script...", async () => {
      const version = this.editVersion;
      try {
        const status = await invoke<ScriptStatus>("script_status");
        if (!this.current(version)) {
          return;
        }
        const saved = editingDraft(status);
        if (!saved) {
          throw new Error("The saved script is no longer available. Your draft is preserved.");
        }
        this.conflictSaved = structuredClone(saved);
        const ready = await this.showSourceComparison(
          saved,
          this.scriptDraft(),
          "Saved in another window (left) and your draft (right)",
        );
        if (ready && this.current(version)) {
          this.conflictReviewed = true;
        }
      } catch (error: unknown) {
        if (this.current(version)) {
          this.scriptFailure("Conflict comparison failed", error);
        }
      }
    });
  }
  keepMyDraft(): void {
    if (this.scriptBusy || !this.conflictSaved || !this.conflictReviewed) {
      return;
    }
    this.savedDraft = structuredClone(this.conflictSaved);
    this.savedFingerprint = scriptFingerprint(this.savedDraft);
    this.conflictVisible = false;
    this.closeComparison();
    this.updateDraftState();
    this.feedback("Your draft is kept. Save it explicitly to replace the reviewed saved version.");
  }
  loadSavedDraft(): void {
    if (this.scriptBusy || !this.conflictSaved || !this.conflictReviewed) {
      return;
    }
    this.previousConflictDraft = this.scriptDraft();
    this.savedDraft = structuredClone(this.conflictSaved);
    this.savedFingerprint = scriptFingerprint(this.savedDraft);
    this.conflictVisible = false;
    this.applyDraft(this.savedDraft);
    this.canUndoSavedLoad = true;
    this.feedback(
      "Saved script loaded. Restore your previous draft if needed; live traffic is unchanged.",
    );
  }
  restorePreviousDraft(): void {
    if (this.scriptBusy || !this.previousConflictDraft) {
      return;
    }
    const previous = this.previousConflictDraft;
    this.previousConflictDraft = this.scriptDraft();
    this.applyDraft(previous);
    this.feedback("Previous draft restored. Save and apply are explicit.");
  }
  private setJavaScriptHistory(history: string[]): void {
    this.javascriptHistory = [...history];
    this.historyEntries = history.map((source, index) => ({
      source,
      index,
      label: `JavaScript draft ${index + 1}`,
    }));
  }
  private rememberJavaScript(source: string, history = this.javascriptHistory): string[] {
    if (!source || history.includes(source)) {
      return [...history];
    }
    if (history.length >= 8) {
      throw new Error(
        "Previous JavaScript drafts are full. Remove a draft below before retaining another.",
      );
    }
    return [source, ...history];
  }
  historyToggled(event: Event): void {
    this.historyOpen = (event.currentTarget as HTMLDetailsElement).open;
  }
  restoreJavaScriptDraft(index: number): void {
    const source = this.javascriptHistory[index];
    if (this.scriptBusy || this.reviewingTypeScript || source === undefined) {
      return;
    }
    try {
      const currentJs =
        this.language === "javascript" ? this.sourceEditor!.getValue() : this.retainedSource;
      const history = this.javascriptHistory.filter((_, item) => item !== index);
      this.setJavaScriptHistory(
        currentJs && currentJs !== source ? this.rememberJavaScript(currentJs, history) : history,
      );
      const typescript =
        this.language === "typescript" ? this.sourceEditor!.getValue() : this.retainedSource;
      this.retainedSource = typescript;
      this.language = "javascript";
      this.closeComparison();
      this.replaceSource(source, "javascript");
      this.historyOpen = false;
      this.$flushUpdates();
      this.scriptEditor.scrollIntoView({ block: "nearest" });
      this.focusEditor();
      this.feedback(
        "Previous JavaScript draft restored. Current JavaScript was retained. Save and apply to change live traffic.",
      );
    } catch (error: unknown) {
      this.scriptFailure("Restore failed", error);
    }
  }
  removeJavaScriptDraft(index: number): void {
    if (
      this.scriptBusy ||
      this.reviewingTypeScript ||
      this.javascriptHistory[index] === undefined
    ) {
      return;
    }
    this.removedHistorySource = this.javascriptHistory[index]!;
    this.setJavaScriptHistory(this.javascriptHistory.filter((_, item) => item !== index));
    this.canUndoHistoryRemoval = true;
    this.markScriptChanged();
  }
  undoJavaScriptRemoval(): void {
    if (this.scriptBusy || this.removedHistorySource === null) {
      return;
    }
    try {
      this.setJavaScriptHistory(this.rememberJavaScript(this.removedHistorySource));
      this.canUndoHistoryRemoval = false;
      this.removedHistorySource = null;
      this.markScriptChanged();
    } catch (error: unknown) {
      this.scriptFailure("Undo failed", error);
    }
  }
  private diagnosticMessage(value: unknown): string {
    if (typeof value === "string") {
      return value;
    }
    if (typeof value === "object" && value !== null && "messageText" in value) {
      const next =
        "next" in value && Array.isArray(value.next)
          ? value.next.map((item: unknown) => this.diagnosticMessage(item)).join(" ")
          : "";
      return `${String(value.messageText)}${next ? ` ${next}` : ""}`;
    }
    return String(value);
  }
  retryTypeScriptCheck(): void {
    void this.refreshAuthoringStatus();
  }
  goToTypeScriptIssue(issue: { line: number; column: number }): void {
    this.sourceEditor?.setPosition({ lineNumber: issue.line, column: issue.column });
    this.sourceEditor?.revealLineInCenter(issue.line);
    this.focusEditor();
  }
  private updateRuntimeStatus(status: ScriptRuntimeStatus[]): void {
    const runtime = status.find(
      (item) => item.scriptId === "user-script" && item.revision === this.appliedRevision,
    );
    this.runtimeIssue =
      this.scriptActive && runtime && !runtime.available
        ? (runtime.error ?? "The applied script is unavailable.")
        : "";
    if (this.runtimeIssue) {
      this.activeScriptText = `Unavailable - applied build ${this.appliedRevision}`;
    } else if (this.scriptActive) {
      this.activeScriptText = `On - applied build ${this.appliedRevision}`;
    }
  }
  private stopRuntimeRefresh(): void {
    if (this.runtimeTimer !== null) {
      clearTimeout(this.runtimeTimer);
      this.runtimeTimer = null;
    }
  }
  private scheduleRuntimeRefresh(): void {
    this.stopRuntimeRefresh();
    if (!this.active || !this.scriptReady || !this.isConnected) {
      return;
    }
    this.runtimeTimer = setTimeout(() => {
      void this.refreshRuntime();
    }, 1000);
  }
  private async refreshRuntime(): Promise<void> {
    try {
      if (!this.active || !this.isConnected || this.scriptBusy) {
        return;
      }
      const revision = this.appliedRevision;
      const runtimes = await invoke<ScriptRuntimeStatus[]>("script_runtime_status");
      if (
        !this.active ||
        !this.isConnected ||
        this.scriptBusy ||
        revision !== this.appliedRevision
      ) {
        return;
      }
      const current = runtimes.find((item) => item.scriptId === "user-script");
      if ((current?.revision ?? null) !== revision) {
        const status = await invoke<ScriptStatus>("script_status");
        if (
          !this.active ||
          !this.isConnected ||
          this.scriptBusy ||
          revision !== this.appliedRevision
        ) {
          return;
        }
        this.renderScriptStatus(status);
        this.updateDraftState();
      } else {
        this.updateRuntimeStatus(runtimes);
      }
    } catch {
      /* A later refresh or explicit retry can recover; authored work is preserved. */
    } finally {
      this.scheduleRuntimeRefresh();
    }
  }
  async retryScript(): Promise<void> {
    await this.runScript("Retrying the applied JavaScript build...", async () => {
      try {
        const status = await invoke<ScriptStatus>("script_status");
        const result = await invoke<ScriptStatus>("retry_script", {
          scriptId: "user-script",
          expectedGeneration: status.generation,
        });
        if (!this.isConnected) {
          return;
        }
        this.renderScriptStatus(result);
        this.updateDraftState();
        this.feedback("Applied JavaScript is available again. Draft edits are unchanged.");
      } catch (error: unknown) {
        this.scriptFailure("Retry failed", error);
      }
    });
  }
  private feedback(message: string): void {
    this.scriptText = message;
    this.statusKind = "success";
  }
  private scriptFailure(prefix: string, error: unknown): void {
    this.candidate = null;
    const message = describeError(error);
    const conflict =
      /revision must increase|candidate is unknown|workspace changed|newer script draft/.test(
        message,
      );
    const recovery = this.conflictVisible
      ? "Your edits are preserved. Compare with the saved script, then explicitly keep your draft or load the saved version."
      : conflict
        ? "The workspace changed. Run the action again to validate the current edits."
        : prefix === "Validation failed" || prefix === "Sandbox test aborted"
          ? "Review the source, capabilities, and test input, then retry."
          : "Your draft and the previously active script are preserved. Try the action again.";
    this.scriptText = `${prefix}: ${message}\n\n${recovery}`;
    this.statusKind = "error";
    if (prefix !== "Validation failed" || this.typeScriptIssue) {
      return;
    }
    const model = this.sourceEditor?.getModel();
    if (model) {
      this.monaco!.editor.setModelMarkers(model, "transmog-rust", [
        {
          severity: this.monaco!.MarkerSeverity.Error,
          message,
          startLineNumber: 1,
          startColumn: 1,
          endLineNumber: 1,
          endColumn: Math.max(2, model.getLineMaxColumn(1)),
        },
      ]);
    }
  }
}
ScriptEditor.define("script-editor");
