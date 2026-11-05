import "../pane-divider/pane-divider.js";
import "./debug-value.js";
import { ConsoleOutput, consoleRuns, consoleRunKey, type ClearedLog } from "./console-output.js";
import { breakpointGroups, type Point, type PointGroup } from "./breakpoints.js";
import { debuggerToggleKey } from "./shortcuts.js";
import { evidence } from "./models.js";
import {
  sourceKey,
  sourceTree,
  matchingFiles,
  searchSources,
  type DebuggerFile as File,
  type SourceNode,
  type SourceResult,
} from "./navigation.js";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { PropertyRow } from "./properties.js";
import {
  Inspection,
  ConsoleHistory,
  type InspectionScope,
  type ValueView,
  type ValueLease,
  type ConsoleHistoryState,
} from "./inspection.js";
import type {
  ScriptDiagnostics,
  DiagnosticViewState,
} from "../script-diagnostics/script-diagnostics.js";
import type { SourceLocation } from "../script-diagnostics/models.js";
import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import type * as Monaco from "monaco-editor";
import type { monaco as MonacoApi } from "../monaco.js";
import { loadMonaco, waitForStyles } from "../editor-runtime.js";
import type {
  DebugSessionInfo,
  DebugSessionView,
  DebugAction,
  DebugFrame,
} from "../debugger-workspace/models.js";
import type { DeveloperEvidence } from "../script-diagnostics/models.js";
import "../script-diagnostics/script-diagnostics.js";
import { describeError } from "../utilities.js";
import { boundedCopy, copyScalar, requestCopy } from "../inspection-copy.js";
interface DebuggerLayout {
  sourcesSplit: number;
  inspectorSplit: number;
  consoleSplit: number;
  sourcesVisible: boolean;
  inspectorVisible: boolean;
  consoleVisible: boolean;
}
interface View {
  skipPauses?: boolean;
  focus?: string | null;
  theme?: string;
  open: boolean;
  files: File[];
  targets: DebugSessionInfo[];
  breakpoints: Point[];
  viewState?: SavedView;
  layout?: DebuggerLayout;
  watches?: string[];
}
interface SavedView {
  consoleHistory?: ConsoleHistoryState;
  consoleLevel?: string;
  consoleFilter?: string;
  clearedLogs?: ClearedLog[];
  valueLeases?: ValueLease[];
  logpoints?: ConsoleRow[];
  eventCursors?: Array<[string, number]>;
  watches?: string[];
  watchInput?: string;
  watchEdit?: { expression: string; draft: string } | undefined;
  commands?: string[];
  groups?: Array<[string, boolean]>;
  split?: string;
  diagnostics?: DiagnosticViewState | undefined;
  scroll?: number;
  focus?: string | null;
  requests?: Array<[string, number]>;
  file?: string | undefined;
  artifact?: string | undefined;
  target: string;
  frame: number;
  input: string;
  console: ConsoleRow[];
  states: Array<[string, Monaco.editor.ICodeEditorViewState | null]>;
  tabs?: string[];
  history?: string[];
  historyIndex?: number;
}
interface ConsoleRow {
  generation?: number;
  id: number;
  input: string;
  output: string;
  thrown: boolean;
  value?: ValueView;
  session?: string;
  pause?: number;
  time?: number;
  label?: string;
  technical?: string;
  level?: string;
  source?: { artifact: string; location: SourceLocation };
}
interface Watch {
  id: number;
  expression: string;
  output: string;
  value?: ValueView;
}
export class DebuggerWorkspace extends WebUIElement {
  @observable debuggerTransition = "Opening debugger…";
  @observable debuggerConditionSaving = false;
  @observable debuggerPauseContext = "Paused";
  @observable debuggerRequestLabel = "";
  @observable debuggerRequestUrl = "";
  @attr({ attribute: "data-theme" }) theme = "system";
  @attr({ mode: "boolean" }) standalone = false;
  @observable debuggerSources: SourceNode[] = [];
  @observable debuggerTabs: Array<{
    key: string;
    file: File;
    label: string;
    selected: boolean;
    tabIndex: number;
  }> = [];
  @observable debuggerCanBack = false;
  @observable debuggerCanForward = false;
  @observable debuggerSourcesVisible = true;
  @observable debuggerInspectorVisible = true;
  @observable debuggerConsoleVisible = true;
  @observable debuggerSourcesSplit = "18";
  @observable debuggerSourcesShare = "18%";
  @observable debuggerInspectorSplit = "78";
  @observable debuggerInspectorShare = "22%";
  @observable debuggerCompact = false;
  @observable debuggerCompactPane = "source";
  @observable debuggerNavigationTitle = "Open source";
  @observable debuggerNavigationQuery = "";
  @observable debuggerNavigationStatus = "";
  @observable debuggerNavigationResults: SourceResult[] = [];
  navigationDialog!: HTMLDialogElement;
  navigationInput!: HTMLInputElement;
  consolePrompt!: HTMLTextAreaElement;
  private navigationSearch = false;
  private navigationGeneration = 0;
  private navigationTimer = 0;
  private history: string[] = [];
  private historyIndex = -1;
  private resizeObserver: ResizeObserver | null = null;
  @observable debuggerTargets: DebugSessionInfo[] = [];
  @observable debuggerWorkerOptions: Array<DebugSessionInfo & { state: string }> = [];
  @observable debuggerSkipPauses = false;
  @observable debuggerPauseNext = false;
  @observable debuggerControlSubmitting = false;
  @observable debuggerPauseButton = "Pause next callback (F8)";
  @observable debuggerPauseIcon = "pause";
  @observable debuggerPointGroups: PointGroup[] = [];
  @observable debuggerEnabledPoints = 0;
  @observable debuggerConditionKind = "pause";
  @observable debuggerLogExpression = "";
  @observable debuggerConditionTitle = "Breakpoint condition";
  @observable debuggerTarget = "";
  @observable debuggerSourceLabel = "Select a source";
  @observable debuggerStatus = "Attaching debugger…";
  @observable debuggerPaused = false;
  @observable debuggerClosed = true;
  @observable debuggerExceptions = "none";
  @observable debuggerResult = "";
  @observable debuggerSummary = "";
  @observable debuggerRestartable = false;
  @observable debuggerFrames: DebugFrame[] = [];
  @observable debuggerFrame = 0;
  @observable debuggerScopeValues: ValueView[] = [];
  @observable debuggerStack: Array<DebugFrame & { label: string; selected: boolean }> = [];
  @observable debuggerWatches: Watch[] = [];
  @observable debuggerWatchInput = "";
  @observable debuggerWatchEditing = 0;
  @observable debuggerWatchDraft = "";
  @observable debuggerWatchSaving = false;
  @observable debuggerWatchOpen = true;
  private watchEdit: Promise<boolean> = Promise.resolve(true);
  @observable debuggerCompletions: Array<{ name: string; selected: boolean }> = [];
  @observable debuggerCompletionIndex = 0;
  @observable debuggerCompletionActive = "";
  @observable debuggerPoints: Array<Point & { label: string }> = [];
  @observable debuggerEvidence: DeveloperEvidence[] = [];
  @observable debuggerConsole: ConsoleRow[] = [];
  @observable debuggerFeed: ConsoleRow[] = [];
  @observable debuggerConsoleFilter = "all";
  @observable debuggerConsoleLevel = "all";
  private readonly consoleLogs = new ConsoleOutput();
  private pendingConsoleScroll: number | null = null;
  private logKeys = new Map<string, number>();
  private logpointRows: ConsoleRow[] = [];
  private eventCursors = new Map<string, number>();
  private pauseControl: { session: string; request: number } | null = null;
  private continueContext: Monaco.editor.IContextKey<boolean> | null = null;
  private pendingValueLeases: ValueLease[] = [];
  private pendingConsoleValues: ConsoleRow[] = [];
  private quietSubmitting = 0;
  private pointEdits: Promise<unknown> = Promise.resolve();
  private pointEditsPending = 0;
  private pointEditRevision = 0;
  private breakpointKey = "";
  private quietRequests = new Set<string>();
  @observable debuggerInput = "";
  @observable debuggerSubmitting = false;
  @observable debuggerScope = "Global";
  @observable debuggerError = "";
  @observable debuggerRepresentation = "typescript";
  @observable debuggerCanSwitch = false;
  @observable debuggerCondition = "";
  @observable debuggerConditionSource = "";
  @observable debuggerConsoleSplit = "70";
  @observable debuggerConsoleShare = "30%";
  resizeConsole(event: CustomEvent<{ value: number; committed?: boolean }>): void {
    const value = Math.max(45, Math.min(85, event.detail.value));
    this.debuggerConsoleSplit = String(value);
    this.debuggerConsoleShare = `${100 - value}%`;
    if (event.detail.committed !== false) {
      void this.saveLayout();
    }
  }
  private applyLayout(layout?: DebuggerLayout): void {
    if (!layout) {
      return;
    }
    this.debuggerSourcesSplit = String(layout.sourcesSplit);
    this.debuggerSourcesShare = `${layout.sourcesSplit}%`;
    this.debuggerInspectorSplit = String(100 - layout.inspectorSplit);
    this.debuggerInspectorShare = `${layout.inspectorSplit}%`;
    this.debuggerConsoleSplit = String(layout.consoleSplit);
    this.debuggerConsoleShare = `${100 - layout.consoleSplit}%`;
    this.debuggerSourcesVisible = layout.sourcesVisible;
    this.debuggerInspectorVisible = layout.inspectorVisible;
    this.debuggerConsoleVisible = layout.consoleVisible;
  }
  private async saveLayout(): Promise<void> {
    try {
      await invoke("debugger_layout", {
        layout: {
          sourcesSplit: Number(this.debuggerSourcesSplit),
          inspectorSplit: 100 - Number(this.debuggerInspectorSplit),
          consoleSplit: Number(this.debuggerConsoleSplit),
          sourcesVisible: this.debuggerSourcesVisible,
          inspectorVisible: this.debuggerInspectorVisible,
          consoleVisible: this.debuggerConsoleVisible,
        },
      });
    } catch (error) {
      if (this.isConnected) {
        this.debuggerError = describeError(error);
      }
    }
  }
  resizeSidebar(side: string, event: CustomEvent<{ value: number; committed?: boolean }>): void {
    if (side === "sources") {
      const value = Math.max(15, Math.min(40, event.detail.value));
      this.debuggerSourcesSplit = String(value);
      this.debuggerSourcesShare = `${value}%`;
    } else {
      const value = Math.max(60, Math.min(85, event.detail.value));
      this.debuggerInspectorSplit = String(value);
      this.debuggerInspectorShare = `${100 - value}%`;
    }
    if (event.detail.committed !== false) {
      void this.saveLayout();
    }
  }
  toggleSidebar(side: string): void {
    if (this.debuggerCompact) {
      this.debuggerCompactPane = this.debuggerCompactPane === side ? "source" : side;
    } else if (side === "sources") {
      this.debuggerSourcesVisible = !this.debuggerSourcesVisible;
    } else {
      this.debuggerInspectorVisible = !this.debuggerInspectorVisible;
    }
    this.$flushUpdates();
    if (this.debuggerCompactPane === "source") {
      this.editor?.focus();
    }
    void this.saveLayout();
  }
  showCode(): void {
    this.debuggerCompactPane = "source";
  }
  toggleConsole(): void {
    this.debuggerConsoleVisible = !this.debuggerConsoleVisible;
    this.$flushUpdates();
    if (this.debuggerConsoleVisible) {
      this.consolePrompt.focus();
    } else {
      this.editor?.focus();
    }
    void this.saveLayout();
  }
  private updateNavigation(): void {
    const selected = this.selected ? sourceKey(this.selected) : "";
    this.debuggerSources = sourceTree(this.files, this.sourceGroups, selected);
    const tabs = this.debuggerTabs.map((tab) => ({
      ...tab,
      file: this.files.find((file) => sourceKey(file) === tab.key) ?? tab.file,
    }));
    this.debuggerTabs = tabs.map((tab) => {
      const name = tab.file.path.split("/").at(-1)!;
      const duplicates = tabs.filter((other) => other.file.path.split("/").at(-1) === name);
      const samePath = duplicates.filter(
        (other) => other.file.package === tab.file.package && other.file.path === tab.file.path,
      );
      return {
        ...tab,
        label:
          duplicates.length > 1
            ? `${tab.file.package}/${tab.file.path}${samePath.length > 1 ? ` · ${tab.file.state}` : ""}`
            : name,
        selected: tab.key === selected,
        tabIndex: tab.key === selected ? 0 : -1,
      };
    });
    this.debuggerCanBack = this.historyIndex > 0;
    this.debuggerCanForward = this.historyIndex < this.history.length - 1;
  }
  async navigateHistory(direction: number): Promise<void> {
    let index = this.historyIndex + direction;
    while (index >= 0 && index < this.history.length) {
      const file = this.files.find((file) => sourceKey(file) === this.history[index]);
      if (file) {
        this.historyIndex = index;
        await this.openFile(file, true, false);
        return;
      }
      index += direction;
    }
  }
  async closeTab(key: string): Promise<void> {
    const index = this.debuggerTabs.findIndex((tab) => tab.key === key);
    this.debuggerTabs = this.debuggerTabs.filter((tab) => tab.key !== key);
    if (this.selected && sourceKey(this.selected) === key) {
      const next = this.debuggerTabs[Math.max(0, index - 1)];
      if (next) {
        await this.openFile(next.file);
      } else {
        this.selected = null;
        this.editor?.setModel(null);
        this.model?.dispose();
        this.model = null;
        this.debuggerSourceLabel = "Select a source";
        this.debuggerCanSwitch = false;
        this.updateNavigation();
      }
    }
    this.$flushUpdates();
    const selected = this.debuggerTabs.find((tab) => tab.selected);
    if (selected) {
      this.shadowRoot?.getElementById(`source-tab-${selected.key}`)?.focus();
    } else {
      this.editor?.focus();
    }
  }
  async tabsKey(event: KeyboardEvent): Promise<void> {
    if (
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      !["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
    ) {
      return;
    }
    const tabs = this.debuggerTabs;
    if (!tabs.length) {
      return;
    }
    event.preventDefault();
    const current = tabs.findIndex((tab) => tab.selected);
    const index =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? tabs.length - 1
          : (current + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
    const tab = tabs[index]!;
    await this.openFile(tab.file);
    this.shadowRoot?.getElementById(`source-tab-${tab.key}`)?.focus();
  }
  openNavigation(search = false): void {
    this.navigationSearch = search;
    this.debuggerNavigationTitle = search ? "Search sources" : "Open source";
    this.debuggerNavigationQuery = "";
    this.debuggerNavigationStatus = search
      ? "Search saved and installed sources without activating them."
      : "";
    this.debuggerNavigationResults = search ? [] : matchingFiles(this.files, "");
    this.navigationDialog.showModal();
    this.navigationInput.focus();
  }
  closeNavigation(): void {
    this.navigationGeneration++;
    window.clearTimeout(this.navigationTimer);
    this.navigationDialog.close();
  }
  navigationCanceled(): void {
    this.navigationGeneration++;
    window.clearTimeout(this.navigationTimer);
  }
  navigationInputChanged(event: Event): void {
    this.debuggerNavigationQuery = (event.target as HTMLInputElement).value;
    window.clearTimeout(this.navigationTimer);
    const generation = ++this.navigationGeneration;
    if (!this.navigationSearch) {
      this.debuggerNavigationResults = matchingFiles(this.files, this.debuggerNavigationQuery);
      return;
    }
    this.debuggerNavigationResults = [];
    this.debuggerNavigationStatus = this.debuggerNavigationQuery.trim()
      ? "Searching…"
      : "Enter text to search sources.";
    if (!this.debuggerNavigationQuery.trim()) {
      return;
    }
    this.navigationTimer = window.setTimeout(() => {
      void this.searchCatalog(generation);
    }, 150);
  }
  private async searchCatalog(generation: number): Promise<void> {
    const current = (): boolean =>
      this.isConnected && this.navigationDialog.open && generation === this.navigationGeneration;
    const found = await searchSources(
      this.files,
      this.debuggerNavigationQuery,
      (file) => invoke<string>("debugger_file", { id: file.id, artifact: file.artifact }),
      current,
    );
    if (!current()) {
      return;
    }
    this.debuggerNavigationResults = found.results;
    this.debuggerNavigationStatus = `${found.results.length} results${found.limited ? " · some sources or matches omitted" : ""}`;
  }
  navigationKey(event: KeyboardEvent): void {
    if (event.isComposing) {
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      const result = this.debuggerNavigationResults.find((result) => result.selected);
      if (result) {
        void this.openNavigationResult(result);
      }
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const results = this.debuggerNavigationResults;
      const index = results.findIndex((result) => result.selected);
      const next = (index + (event.key === "ArrowDown" ? 1 : -1) + results.length) % results.length;
      this.debuggerNavigationResults = results.map((result, index) => ({
        ...result,
        selected: index === next,
      }));
      this.$flushUpdates();
      const selected = this.debuggerNavigationResults[next];
      if (selected) {
        this.shadowRoot
          ?.getElementById(`source-result-${selected.key}`)
          ?.scrollIntoView({ block: "nearest" });
      }
    }
  }
  async openNavigationResult(result: SourceResult): Promise<void> {
    this.closeNavigation();
    await this.openFile(result.file);
    if (this.isSelected(result.file)) {
      if (result.line) {
        this.editor?.setPosition({ lineNumber: result.line, column: 1 });
        this.editor?.revealLineInCenter(result.line);
      }
      this.editor?.focus();
    }
  }
  diagnostics?: ScriptDiagnostics;
  consoleOutput!: HTMLElement;
  @observable debuggerStackTruncated = false;
  private readonly commandHistory = new ConsoleHistory();
  private watchSerial = 0;
  private watchGeneration = 0;
  private inspectionGeneration = 0;
  private completionGeneration = 0;
  private completionStart = 0;
  private completionEnd = 0;
  private completionTimer = 0;
  private editorRegistrations: Monaco.IDisposable[] = [];
  private readonly inspection = new Inspection(
    (action, session) => this.action(action, session, true),
    (scope) => this.validInspection(scope),
    () => this.inspectionChanged(),
  );
  private inspectionScope(): InspectionScope {
    return {
      generation: this.views.get(this.debuggerTarget)?.stream.valueGeneration ?? this.pause,
      session: this.debuggerTarget,
      pause: this.pause,
      frame: this.debuggerPaused ? this.debuggerFrame : null,
    };
  }
  private validInspection(scope: InspectionScope): boolean {
    const stream = this.views.get(scope.session)?.stream;
    const event = stream?.paused?.event;
    return (
      !!stream &&
      !stream.closed &&
      (scope.generation === undefined ||
        stream.valueGeneration === undefined ||
        scope.generation === stream.valueGeneration) &&
      (scope.pause === 0 ? !stream.paused : event?.kind === "paused" && event.pause === scope.pause)
    );
  }
  private inspectionChanged(): void {
    this.debuggerScopeValues = this.debuggerScopeValues.map((value) => ({
      ...this.inspection.get(value.key)!,
    }));
    this.debuggerWatches = this.debuggerWatches.map((watch) =>
      watch.value ? { ...watch, value: { ...this.inspection.get(watch.value.key)! } } : watch,
    );
    this.debuggerConsole = this.debuggerConsole.map((row) =>
      row.value && this.inspection.get(row.value.key)
        ? { ...row, value: { ...this.inspection.get(row.value.key)! } }
        : row,
    );
    this.updateConsoleFeed();
    this.renderPoints();
  }
  async inspectionToggled(event: CustomEvent<{ key: string; row?: PropertyRow }>): Promise<void> {
    await this.toggleValue(event.detail.key, event.detail.row);
  }
  async toggleValue(key: string, row?: PropertyRow): Promise<void> {
    await this.inspection.toggle(key, row);
  }
  private updateStack(): void {
    this.debuggerStack = this.debuggerFrames.map((frame) => {
      const location = frame.original ?? frame.location;
      return {
        ...frame,
        function: frame.function || "(anonymous)",
        selected: frame.index === this.debuggerFrame,
        label: location
          ? `${location.source.split("/").at(-1)}:${location.line}:${location.column}`
          : "Source unavailable",
      };
    });
  }
  private resetInspection(): void {
    this.inspectionGeneration++;
    this.closeCompletions();
    for (const scope of this.debuggerScopeValues) {
      this.inspection.remove(scope.key);
    }
    this.debuggerScopeValues = [];
    this.debuggerWatches = this.debuggerWatches.map((watch) => {
      this.inspection.remove(`watch-${watch.id}`);
      return {
        id: watch.id,
        expression: watch.expression,
        output: this.debuggerPaused ? "Waiting…" : "Not paused",
      };
    });
  }
  watchInput(event: Event): void {
    this.debuggerWatchInput = (event.target as HTMLInputElement).value;
  }
  async addWatch(event: Event): Promise<void> {
    event.preventDefault();
    const draft = this.debuggerWatchInput;
    if (await this.addWatchExpression(draft.trim())) {
      if (this.debuggerWatchInput === draft) {
        this.debuggerWatchInput = "";
      }
    }
  }
  private async addWatchExpression(expression: string): Promise<boolean> {
    if (!expression || expression.length > 1024 || this.debuggerWatchSaving) {
      return false;
    }
    if (this.debuggerWatches.some((watch) => watch.expression === expression)) {
      return true;
    }
    if (this.debuggerWatches.length >= 16) {
      this.debuggerError = "Watch is full. Remove an expression to add another.";
      return false;
    }
    return this.saveWatches([
      ...this.debuggerWatches,
      { id: ++this.watchSerial, expression, output: "Not paused" },
    ]);
  }
  private saveWatches(watches: Watch[]): Promise<boolean> {
    if (this.debuggerWatchSaving || this.debuggerTransition) {
      return Promise.resolve(false);
    }
    this.debuggerWatchSaving = true;
    const generation = this.generation;
    this.watchEdit = (async () => {
      try {
        // Commit session memory before presenting the edit, including across F12 close.
        await invoke("debugger_watches", { expressions: watches.map((watch) => watch.expression) });
        if (!this.isConnected || generation !== this.generation) {
          return false;
        }
        for (const old of this.debuggerWatches) {
          if (
            !watches.some((watch) => watch.id === old.id && watch.expression === old.expression)
          ) {
            this.inspection.remove(`watch-${old.id}`);
          }
        }
        // Keep values refreshed while the native save was pending.
        this.debuggerWatches = watches.map(
          (watch) =>
            this.debuggerWatches.find(
              (old) => old.id === watch.id && old.expression === watch.expression,
            ) ?? watch,
        );
        this.debuggerError = "";
        void this.refreshWatches();
        return true;
      } catch (error) {
        if (this.isConnected) {
          this.debuggerError = describeError(error);
        }
        return false;
      } finally {
        this.debuggerWatchSaving = false;
      }
    })();
    return this.watchEdit;
  }
  async removeWatch(id: number): Promise<void> {
    if (await this.saveWatches(this.debuggerWatches.filter((watch) => watch.id !== id))) {
      if (this.debuggerWatchEditing === id) {
        this.cancelWatchEdit();
      }
    }
  }
  editWatch(id: number): void {
    if (this.debuggerWatchSaving) {
      return;
    }
    const watch = this.debuggerWatches.find((watch) => watch.id === id);
    if (!watch) {
      return;
    }
    this.debuggerWatchEditing = id;
    this.debuggerWatchDraft = watch.expression;
    this.$flushUpdates();
    const input = this.shadowRoot?.querySelector<HTMLTextAreaElement>(".watch-edit textarea");
    input?.focus();
    input?.select();
  }
  editWatchInput(event: Event): void {
    this.debuggerWatchDraft = (event.target as HTMLTextAreaElement).value;
  }
  cancelWatchEdit(): void {
    if (this.debuggerWatchSaving) {
      return;
    }
    const id = this.debuggerWatchEditing;
    this.debuggerWatchEditing = 0;
    this.debuggerWatchDraft = "";
    this.$flushUpdates();
    this.shadowRoot?.querySelector<HTMLButtonElement>(`[data-watch-edit="${id}"]`)?.focus();
  }
  watchEditKey(event: KeyboardEvent): void {
    if (event.key === "Escape" && !event.isComposing) {
      event.preventDefault();
      event.stopPropagation();
      this.cancelWatchEdit();
    } else if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.isComposing &&
      event.composedPath()[0] instanceof HTMLTextAreaElement
    ) {
      event.preventDefault();
      event.stopPropagation();
      void this.saveWatchEdit(event);
    }
  }
  async saveWatchEdit(event: Event): Promise<void> {
    event.preventDefault();
    const id = this.debuggerWatchEditing;
    const expression = this.debuggerWatchDraft.trim();
    if (!expression || expression.length > 1024) {
      return;
    }
    if (this.debuggerWatches.some((watch) => watch.id !== id && watch.expression === expression)) {
      this.debuggerError = "That expression is already watched.";
      return;
    }
    const watches = this.debuggerWatches.map((watch) =>
      watch.id === id ? { id, expression, output: "Not paused" } : watch,
    );
    if (await this.saveWatches(watches)) {
      this.cancelWatchEdit();
    }
  }
  watchDisclosure(event: Event): void {
    this.debuggerWatchOpen = (event.target as HTMLDetailsElement).open;
  }
  private sourceSelection(limit: number): string {
    const selection = this.editor?.getSelection();
    if (!selection || selection.isEmpty() || !this.model) {
      return "";
    }
    const length = this.model.getValueLengthInRange(selection);
    if (length > limit) {
      this.debuggerError = `Select at most ${limit.toLocaleString()} characters.`;
      return "";
    }
    return this.model.getValueInRange(selection).trim();
  }
  private async watchSelection(): Promise<void> {
    const expression = this.sourceSelection(1024);
    if (!expression) {
      return;
    }
    this.debuggerInspectorVisible = true;
    if (this.debuggerCompact) {
      this.debuggerCompactPane = "inspector";
    }
    this.debuggerWatchOpen = true;
    void this.saveLayout();
    if (await this.addWatchExpression(expression)) {
      const watch = this.debuggerWatches.find((watch) => watch.expression === expression);
      this.$flushUpdates();
      this.shadowRoot
        ?.querySelector(`[data-watch-edit="${watch?.id}"]`)
        ?.scrollIntoView({ block: "nearest" });
    }
  }
  private async evaluateSelection(): Promise<void> {
    const expression = this.sourceSelection(8192);
    if (!expression) {
      return;
    }
    this.debuggerConsoleVisible = true;
    this.$flushUpdates();
    void this.saveLayout();
    await this.evaluate(expression);
  }
  private stackText(): string {
    const lines = this.debuggerFrames.map((frame) => {
      const location = frame.original ?? frame.location;
      return `    at ${copyScalar(frame.function || "(anonymous)")} (${
        location
          ? `${copyScalar(location.source)}:${location.line}:${location.column}`
          : "Source unavailable"
      })`;
    });
    if (this.debuggerStackTruncated) {
      lines.push("[Call stack shortened]");
    }
    return boundedCopy(lines.join("\n"));
  }
  async copyStack(): Promise<void> {
    if (!this.debuggerPaused || !this.debuggerFrames.length) {
      return;
    }
    try {
      await navigator.clipboard.writeText(this.stackText());
    } catch {
      this.debuggerError = "Copy failed. The clipboard is unavailable.";
    }
  }
  stackMenu(event: MouseEvent | KeyboardEvent): void {
    if (!this.debuggerPaused || !this.debuggerFrames.length) {
      return;
    }
    const pause = this.pause;
    const target = this.debuggerTarget;
    const returnFocus = this.shadowRoot?.activeElement;
    requestCopy(event, {
      source: event.currentTarget as HTMLElement,
      current: () =>
        this.isConnected &&
        this.debuggerPaused &&
        this.pause === pause &&
        this.debuggerTarget === target,
      actions: [{ id: "stack", label: "Copy stack trace", read: () => this.stackText() }],
      restoreFocus: () => {
        if (returnFocus instanceof HTMLElement && returnFocus.isConnected) {
          returnFocus.focus({ preventScroll: true });
        } else {
          this.editor?.focus();
        }
      },
    });
  }
  async refreshWatches(): Promise<void> {
    if (!this.debuggerPaused) {
      return;
    }
    const generation = this.inspectionGeneration;
    const watchGeneration = ++this.watchGeneration;
    const scope = this.inspectionScope();
    await Promise.all(
      this.debuggerWatches.map(async (watch) => {
        const event = await this.inspection.request(
          { kind: "inspect", expression: watch.expression, pause: scope.pause, frame: scope.frame },
          scope,
        );
        if (
          generation !== this.inspectionGeneration ||
          watchGeneration !== this.watchGeneration ||
          !this.debuggerWatches.some(
            (current) => current.id === watch.id && current.expression === watch.expression,
          )
        ) {
          if (event?.kind === "evaluated" && event.value.handle && this.validInspection(scope)) {
            this.inspection.releaseLeases([{ scope, handles: [event.value.handle] }]);
          }
          return;
        }
        const value =
          event?.kind === "evaluated"
            ? this.inspection.set(`watch-${watch.id}`, watch.expression, event.value, scope)
            : undefined;
        if (!value) {
          this.inspection.remove(`watch-${watch.id}`);
        }
        this.debuggerWatches = this.debuggerWatches.map((current) =>
          current.id === watch.id
            ? {
                id: current.id,
                expression: current.expression,
                output:
                  event?.kind === "evaluated"
                    ? event.value.display
                    : event?.kind === "rejected"
                      ? event.message
                      : "Inspection unavailable",
                ...(value ? { value } : {}),
              }
            : current,
        );
      }),
    );
  }
  private closeCompletions(): void {
    this.completionGeneration++;
    this.debuggerCompletions = [];
    this.debuggerCompletionActive = "";
  }
  async completePrompt(): Promise<void> {
    const generation = ++this.completionGeneration;
    if (
      this.consolePrompt.getRootNode() instanceof ShadowRoot &&
      (this.consolePrompt.getRootNode() as ShadowRoot).activeElement !== this.consolePrompt
    ) {
      return;
    }
    const input = this.debuggerInput;
    const end = this.consolePrompt.selectionStart;
    const match = /[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*\.?$/.exec(input.slice(0, end));
    if (!match || match[0].length > 1024 || match[0].split(".").length > 8) {
      this.debuggerCompletions = [];
      return;
    }
    const scope = this.inspectionScope();
    const event = await this.inspection.request(
      { kind: "complete", expression: match[0], pause: scope.pause, frame: scope.frame },
      scope,
    );
    if (
      generation !== this.completionGeneration ||
      input !== this.debuggerInput ||
      end !== this.consolePrompt.selectionStart ||
      scope.session !== this.debuggerTarget ||
      scope.frame !== this.inspectionScope().frame
    ) {
      return;
    }
    this.completionStart = end - match[0].split(".").at(-1)!.length;
    this.completionEnd = end;
    this.debuggerCompletionIndex = 0;
    this.debuggerCompletions =
      event?.kind === "completions"
        ? event.matches
            .filter((name) => name !== match[0])
            .map((name, index) => ({ name, selected: index === 0 }))
        : [];
    this.debuggerCompletionActive = this.debuggerCompletions[0]
      ? `completion-${this.debuggerCompletions[0].name}`
      : "";
  }
  acceptCompletion(name: string): void {
    this.debuggerInput =
      this.debuggerInput.slice(0, this.completionStart) +
      name +
      this.debuggerInput.slice(this.completionEnd);
    this.closeCompletions();
    this.$flushUpdates();
    this.consolePrompt.focus();
    this.consolePrompt.setSelectionRange(
      this.completionStart + name.length,
      this.completionStart + name.length,
    );
  }
  completionMouse(event: MouseEvent): void {
    event.preventDefault();
  }
  private matchesFrameSource(): boolean {
    const frame = this.debuggerFrames.find((frame) => frame.index === this.debuggerFrame);
    const position = this.selected?.language === "javascript" ? frame?.location : frame?.original;
    return (
      this.debuggerPaused &&
      this.selected?.artifact === this.views.get(this.debuggerTarget)?.info.artifact &&
      position?.source === this.selected?.source
    );
  }
  private installHover(): void {
    for (const language of ["typescript", "javascript"]) {
      this.editorRegistrations.push(
        this.monaco!.languages.registerHoverProvider(language, {
          provideHover: async (model, position, token) => {
            if (model !== this.model || !this.matchesFrameSource()) {
              return;
            }
            const generation = this.inspectionGeneration;
            const line = model.getLineContent(position.lineNumber);
            const match = [...line.matchAll(/[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*/g)].find(
              (match) =>
                match.index < position.column && match.index + match[0].length >= position.column,
            );
            if (!match || match[0].length > 1024) {
              return;
            }
            const scope = this.inspectionScope();
            const event = await this.inspection.request(
              { kind: "inspect", expression: match[0], pause: scope.pause, frame: scope.frame },
              scope,
            );
            if (event?.kind === "evaluated" && event.value.handle && this.validInspection(scope)) {
              this.inspection.releaseLeases([{ scope, handles: [event.value.handle] }]);
            }
            if (
              token.isCancellationRequested ||
              generation !== this.inspectionGeneration ||
              model !== this.model ||
              !this.matchesFrameSource()
            ) {
              return;
            }
            if (event?.kind !== "evaluated") {
              return;
            }
            return {
              range: new this.monaco!.Range(
                position.lineNumber,
                match.index + 1,
                position.lineNumber,
                match.index + match[0].length + 1,
              ),
              contents: [
                { value: `\`\`\`text\n${event.value.display.replaceAll("```", "ʼʼʼ")}\n\`\`\`` },
              ],
            };
          },
        }),
      );
    }
  }
  private focusUnlisten: UnlistenFn | null = null;
  private movingUnlisten: UnlistenFn | null = null;
  private activeElement(): Element | null {
    let active = this.ownerDocument.activeElement;
    while (active?.shadowRoot?.activeElement) {
      active = active.shadowRoot.activeElement;
    }
    return active;
  }
  private toggleUnlisten: UnlistenFn | null = null;
  private async watchNativeToggle(): Promise<void> {
    const unlisten = await listen("debugger-toggle", () => {
      if (this.standalone) {
        void this.place("closed");
      }
    });
    if (!this.isConnected) {
      unlisten();
      return;
    }
    this.toggleUnlisten = unlisten;
  }
  sourceHost!: HTMLElement;
  sourceStyles!: HTMLLinkElement;
  conditionDialog!: HTMLDialogElement;
  private monaco: typeof MonacoApi | null = null;
  private editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private model: Monaco.editor.ITextModel | null = null;
  private decorations: Monaco.editor.IEditorDecorationsCollection | null = null;
  private files: File[] = [];
  private sourceGroups = new Map<string, boolean>();
  sourceGroupToggled(group: { key: string }, event: Event): void {
    this.sourceGroups.set(group.key, (event.target as HTMLDetailsElement).open);
  }
  private points: Point[] = [];
  private positions = new Map<number, SourceLocation>();
  private projectionKey = "";
  private selected: File | null = null;
  private pause = 0;
  private cursors = new Map<string, number>();
  private views = new Map<string, DebugSessionView>();
  private requests = new Map<string, number>();
  private restoreFrame: number | null = null;
  private preferredRepresentation = "typescript";
  private lastFocus: string | null = null;
  private serial = 0;
  private evaluationSubmission: Promise<number | undefined> | null = null;
  private timer = 0;
  private busy = false;
  private generation = 0;
  private sourceGeneration = 0;
  private conditionContext?: {
    file: File;
    line: number;
    anchor: string;
    point?: Point | undefined;
  };
  private states = new Map<string, Monaco.editor.ICodeEditorViewState | null>();
  private readonly shortcutListener = (event: KeyboardEvent): void => this.keyboard(event);
  private readonly toggleShortcut = (event: KeyboardEvent): void => {
    debuggerToggleKey(event, () => this.place("closed"));
  };
  connectedCallback(): void {
    super.connectedCallback();
    void this.watchNativeToggle().catch((error) => {
      this.debuggerError = describeError(error);
    });
    document.addEventListener("keydown", this.toggleShortcut, true);
    this.resizeObserver = new ResizeObserver(([entry]) => {
      this.debuggerCompact = (entry?.contentRect.width ?? 1000) < 620;
    });
    this.resizeObserver.observe(this);
    this.addEventListener("keydown", this.shortcutListener, true);
    void this.start();
  }
  private async start(): Promise<void> {
    const version = ++this.generation;
    const focus = this.activeElement();
    try {
      const moving = await listen<boolean>("debugger-moving", (event) => {
        this.debuggerTransition = event.payload ? "Moving debugger…" : "";
      });
      if (!this.isConnected || version !== this.generation) {
        moving();
        return;
      }
      this.movingUnlisten = moving;
      const view = await invoke<View>("debugger_workspace");
      if (!this.isConnected || version !== this.generation) {
        return;
      }
      this.apply(view);
      this.applyLayout(view.layout);
      const { monaco } = await loadMonaco();
      await waitForStyles(this.sourceStyles);
      if (!this.isConnected || version !== this.generation) {
        return;
      }
      this.monaco = monaco;
      this.editor = monaco.editor.create(this.sourceHost, {
        readOnly: true,
        renderValidationDecorations: "off",
        automaticLayout: true,
        glyphMargin: true,
        minimap: { enabled: false },
        ariaLabel: "Debugger source",
        theme: matchMedia("(prefers-color-scheme: dark)").matches ? "vs-dark" : "vs",
        tabFocusMode: true,
      });
      this.decorations = this.editor.createDecorationsCollection();
      this.continueContext = this.editor.createContextKey("debuggerCanContinue", false);
      this.installHover();
      this.editor.onMouseDown((event) => {
        if (
          (event.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
            event.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS) &&
          event.event.leftButton &&
          event.target.position
        ) {
          void this.toggle(event.target.position.lineNumber);
        }
      });
      this.editor.onContextMenu((event) => {
        if (
          event.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
          event.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS
        ) {
          if (event.target.position) {
            this.editor?.setPosition(event.target.position);
          }
          this.editor?.focus();
          void this.editor?.getAction("editor.action.showContextMenu")?.run();
        }
      });
      for (const [id, label, run] of [
        ["breakpoint", "Toggle breakpoint", () => this.toggle(this.line())],
        ["condition", "Add or edit conditional breakpoint", () => this.condition(this.line())],
        ["logpoint", "Add or edit logpoint", () => this.condition(this.line(), "log")],
        ["disable", "Enable or disable breakpoint", () => this.disable(this.line())],
      ] as const) {
        this.addEditorAction({
          id: `debugger.${id}`,
          label,
          contextMenuGroupId: "debugger",
          run: () => {
            void run();
          },
        });
      }
      this.addEditorAction({
        id: "debugger.continueTo",
        label: "Continue to here",
        contextMenuGroupId: "debugger",
        precondition: "debuggerCanContinue",
        run: () => {
          void this.continueTo(this.line());
        },
      });
      this.addEditorAction({
        id: "debugger.watchSelection",
        label: "Add selection to Watch",
        contextMenuGroupId: "debugger.inspection",
        precondition: "editorHasSelection",
        run: () => this.watchSelection(),
      });
      this.addEditorAction({
        id: "debugger.evaluateSelection",
        label: "Evaluate selection in Console",
        contextMenuGroupId: "debugger.inspection",
        precondition: "editorHasSelection",
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyE],
        run: () => this.evaluateSelection(),
      });
      const ready = await invoke<View>("acknowledge_debugger");
      if (!this.isConnected || version !== this.generation) {
        return;
      }
      this.apply(ready);
      this.debuggerWatches = (ready.watches ?? ready.viewState?.watches ?? []).map(
        (expression) => ({
          id: ++this.watchSerial,
          expression,
          output: "Not paused",
        }),
      );
      if (ready.viewState) {
        const saved = ready.viewState;
        this.sourceGroups = new Map(saved.groups ?? []);
        this.resizeConsole(
          new CustomEvent("pane-resize", { detail: { value: Number(saved.split ?? 70) } }),
        );
        this.debuggerTarget = saved.target;
        this.debuggerFrame = saved.frame;
        this.restoreFrame = saved.frame;
        this.requests = new Map(saved.requests ?? []);
        this.lastFocus = saved.focus ?? null;
        this.debuggerInput = saved.input;
        this.pendingConsoleValues = saved.console.filter(
          (row) => row.value?.value.handle && !row.value.expired,
        );
        this.debuggerConsole = saved.console.map((row) =>
          row.value
            ? {
                ...row,
                value: { ...row.value, expired: true, expanded: false, rows: [], loading: false },
              }
            : row,
        );
        this.pendingValueLeases = saved.valueLeases ?? [];
        this.logpointRows = saved.logpoints ?? [];
        this.eventCursors = new Map(saved.eventCursors ?? []);
        this.commandHistory.restore(
          saved.consoleHistory ?? {
            entries: saved.commands ?? [],
            index: saved.commands?.length ?? 0,
            draft: "",
          },
        );
        this.consoleLogs.restore(saved.clearedLogs ?? []);
        this.debuggerConsoleLevel = saved.consoleLevel ?? "all";
        this.debuggerConsoleFilter = saved.consoleFilter ?? "all";
        this.debuggerWatchInput = saved.watchInput ?? "";
        this.debuggerWatchEditing =
          this.debuggerWatches.find((watch) => watch.expression === saved.watchEdit?.expression)
            ?.id ?? 0;
        this.debuggerWatchDraft = saved.watchEdit?.draft ?? "";
        this.serial = Math.max(
          0,
          ...saved.console.map((row) => row.id),
          ...this.logpointRows.map((row) => row.id),
        );
        this.states = new Map(saved.states);
        this.history = saved.history ?? [];
        this.historyIndex = saved.historyIndex ?? this.history.length - 1;
        this.debuggerTabs = (saved.tabs ?? []).flatMap((key) => {
          const file = this.files.find((file) => sourceKey(file) === key);
          return file
            ? [{ key, file, label: file.path.split("/").at(-1)!, selected: false, tabIndex: -1 }]
            : [];
        });
        this.$flushUpdates();
        if (saved.diagnostics) {
          this.diagnostics?.restoreView(saved.diagnostics);
        }
        this.pendingConsoleScroll = saved.scroll ?? 0;
      }
      const first =
        this.files.find(
          (file) => file.id === ready.viewState?.file && file.artifact === ready.viewState.artifact,
        ) ??
        this.files.find((file) => file.language === "typescript") ??
        this.files[0];
      if (first) {
        await this.openFile(first);
      }
      await this.refresh();
      const unlisten = await listen("debugger-focus", () => this.editor?.focus());
      if (!this.isConnected) {
        unlisten();
        return;
      }
      this.focusUnlisten = unlisten;
      this.timer = window.setInterval(() => {
        void this.refresh();
      }, 300);
    } catch (error) {
      this.debuggerError = describeError(error);
    } finally {
      if (this.isConnected && version === this.generation) {
        const restoreFocus = this.activeElement() === focus;
        this.debuggerTransition = "";
        this.$flushUpdates();
        if (restoreFocus) {
          this.editor?.focus();
        }
      }
    }
  }
  private apply(view: View): void {
    this.debuggerSkipPauses = view.skipPauses ?? false;
    if (view.focus && view.focus !== this.lastFocus) {
      this.lastFocus = view.focus;
      this.debuggerTarget = view.focus;
      this.clearPause();
    }
    this.theme = view.theme ?? "system";
    this.monaco?.editor.setTheme(
      this.theme === "dark" ||
        (this.theme === "system" && matchMedia("(prefers-color-scheme: dark)").matches)
        ? "vs-dark"
        : "vs",
    );
    this.files = view.files;
    if (!this.pointEditsPending) {
      this.points = view.breakpoints;
    }
    this.updateNavigation();
    this.debuggerTargets = view.targets;
    if (!view.targets.some((t) => t.session === this.debuggerTarget)) {
      this.debuggerTarget = view.targets[0]?.session ?? "";
    }
    this.renderPoints();
  }
  private async refresh(): Promise<void> {
    if (
      this.busy ||
      this.debuggerSubmitting ||
      this.debuggerControlSubmitting ||
      this.inspection.registering ||
      this.quietSubmitting ||
      this.debuggerTransition === "Moving debugger…" ||
      this.debuggerTransition === "Closing debugger…" ||
      !this.isConnected
    ) {
      return;
    }
    this.busy = true;
    const generation = this.generation;
    const pointRevision = this.pointEditRevision;
    const consoleScroll = this.consoleOutput.scrollTop;
    const follow =
      this.consoleOutput.scrollHeight -
        this.consoleOutput.scrollTop -
        this.consoleOutput.clientHeight <
      24;
    try {
      const workspace = await invoke<View>("debugger_workspace");
      const breakpointKey = JSON.stringify(workspace.breakpoints);
      const replies = await Promise.all(
        workspace.targets.map(async (target) => ({
          target,
          view: await invoke<DebugSessionView>("workspace_debug_session", {
            session: target.session,
            after:
              breakpointKey !== this.breakpointKey ? 0 : (this.cursors.get(target.session) ?? 0),
          }),
        })),
      );
      // Keep evaluation events unread until the submission's request ID is known.
      if (
        !this.isConnected ||
        generation !== this.generation ||
        pointRevision !== this.pointEditRevision ||
        this.debuggerSubmitting ||
        this.debuggerControlSubmitting ||
        this.inspection.registering ||
        this.quietSubmitting ||
        this.debuggerTransition === "Moving debugger…" ||
        this.debuggerTransition === "Closing debugger…"
      ) {
        return;
      }
      this.apply(workspace);
      this.breakpointKey = breakpointKey;
      if (
        replies.find((reply) => reply.target.session === this.debuggerTarget)?.view.stream.closed
      ) {
        this.clearPause();
      }
      for (const { target, view } of replies) {
        const eventSequences = new Map(
          view.stream.events.map(
            (item, index) =>
              [item, view.stream.cursor - view.stream.events.length + index + 1] as const,
          ),
        );
        const eventCursor = this.eventCursors.get(target.session) ?? 0;
        this.cursors.set(target.session, view.stream.cursor);
        const previous = this.views.get(target.session);
        if (!view.stream.inventory && previous?.stream.inventory) {
          view.stream.inventory = previous.stream.inventory;
        }
        for (const item of view.stream.events) {
          if (item.event.kind === "sources") {
            const inventory = view.stream.inventory ?? { sources: [], breakpoints: [] };
            const sources = new Map(inventory.sources.map((source) => [source.source, source]));
            const points = new Map(inventory.breakpoints.map((point) => [point.id, point]));
            for (const source of item.event.sources) {
              sources.set(source.source, source);
            }
            for (const point of item.event.breakpoints) {
              points.set(point.id, point);
            }
            const ids = new Set(this.points.map((point) => point.breakpoint.id));
            view.stream.inventory = {
              sources: [...sources.values()],
              breakpoints: [...points.values()].filter((point) => ids.has(point.id)),
            };
          }
        }
        this.views.set(target.session, view);
        const restored = this.pendingConsoleValues.filter(
          (row) =>
            row.session === target.session &&
            this.validInspection({
              session: target.session,
              pause: row.pause ?? 0,
              frame: null,
              generation: row.generation ?? row.pause ?? 0,
            }),
        );
        const keptHandles = new Set(
          restored.flatMap((row) => (row.value?.value.handle ? [row.value.value.handle] : [])),
        );
        this.inspection.releaseLeases(
          this.pendingValueLeases
            .filter((lease) => lease.scope.session === target.session)
            .map((lease) => ({
              ...lease,
              handles: lease.handles.filter((handle) => !keptHandles.has(handle)),
            })),
        );
        this.pendingValueLeases = this.pendingValueLeases.filter(
          (lease) => lease.scope.session !== target.session,
        );
        if (
          target.session === this.debuggerTarget &&
          view.stream.paused &&
          (!this.debuggerPaused ||
            this.pause !== (view.stream.paused.event as { pause?: number }).pause) &&
          !view.stream.events.some(
            (item) => item.event.kind === "paused" && (eventSequences.get(item) ?? 0) > eventCursor,
          )
        ) {
          view.stream.events.unshift({ ...view.stream.paused });
        }
        for (const item of view.stream.events) {
          if (eventSequences.has(item) && eventSequences.get(item)! <= eventCursor) {
            continue;
          }
          const event = item.event;
          const replyKey = `${target.session}/${item.request}`;
          const quiet = this.quietRequests.has(replyKey);
          if (event.kind !== "evaluating" && event.kind !== "output" && event.kind !== "sources") {
            this.quietRequests.delete(replyKey);
          }
          if (event.kind === "logpoint" && (eventSequences.get(item) ?? 0) > eventCursor) {
            const location = event.original ?? event.location;
            this.logpointRows = [
              ...this.logpointRows,
              {
                id: ++this.serial,
                input: "",
                output: event.text,
                thrown: event.thrown,
                level: event.omitted ? "warn" : event.thrown ? "error" : "info",
                session: target.session,
                time: event.timeUnixMs,
                label: `${target.label} · logpoint`,
                ...(location ? { source: { artifact: target.artifact, location } } : {}),
              },
            ].slice(-100);
          }
          if (
            (event.kind === "control" || event.kind === "rejected") &&
            this.pauseControl?.session === target.session &&
            this.pauseControl.request === item.request
          ) {
            this.pauseControl = null;
          }
          if (this.inspection.receive(target.session, item.request, event)) {
            continue;
          }
          if (
            event.kind === "paused" &&
            (!this.debuggerPaused || target.session === this.debuggerTarget)
          ) {
            this.debuggerTarget = target.session;
            this.pause = event.pause;
            this.debuggerPaused = true;
            this.resetInspection();
            this.debuggerStackTruncated = event.truncated;
            this.debuggerFrames = event.frames;
            this.debuggerFrame =
              event.frames.find((frame) => frame.index === this.restoreFrame)?.index ??
              event.frames[0]?.index ??
              0;
            this.restoreFrame = null;
            this.updateStack();
            this.debuggerScope = "Selected frame";
            this.debuggerStatus = event.reason;
            await this.revealFrame(
              event.frames.find((frame) => frame.index === this.debuggerFrame),
            );
            if (
              event.frames.length &&
              target.session === this.debuggerTarget &&
              this.pause === event.pause
            ) {
              void this.requestScopes();
              void this.refreshWatches();
            }
          } else if (event.kind === "resumed" && target.session === this.debuggerTarget) {
            this.clearPause();
          } else if (
            event.kind === "scopes" ||
            event.kind === "properties" ||
            event.kind === "property-preview"
          ) {
            // Replies submitted by the previous surface have no registry here.
            const values =
              event.kind === "scopes"
                ? event.scopes
                : event.properties.map((property) => property.value);
            this.inspection.releaseLeases([
              {
                scope: {
                  session: target.session,
                  pause: event.pause,
                  frame: null,
                  generation: item.valueGeneration ?? event.pause,
                },
                handles: values.flatMap((value) => (value.handle ? [value.handle] : [])),
              },
            ]);
          } else if (event.kind === "output") {
            const id = this.requests.get(`${target.session}/${item.request}`);
            this.debuggerConsole = this.debuggerConsole.map((row) =>
              row.id === id
                ? {
                    ...row,
                    output: `${row.output === "Running…" ? "" : row.output + "\n"}${event.level}: ${event.text}`,
                  }
                : row,
            );
          } else if (event.kind === "evaluated") {
            const key = `${target.session}/${item.request}`;
            const id = this.requests.get(key);
            if (id === undefined && event.value.handle && !keptHandles.has(event.value.handle)) {
              this.inspection.releaseLeases([
                {
                  scope: {
                    session: target.session,
                    pause: event.pause,
                    frame: null,
                    generation: item.valueGeneration ?? event.pause,
                  },
                  handles: [event.value.handle],
                },
              ]);
            }
            this.debuggerConsole = this.debuggerConsole.map((row) =>
              row.id === id
                ? {
                    ...row,
                    output: `${row.output === "Running…" ? "" : row.output + "\n"}${event.value.display}`,
                    thrown: event.thrown,
                    generation: item.valueGeneration ?? event.pause,
                    value: this.inspection.set(`console-${row.id}`, "Result", event.value, {
                      generation: item.valueGeneration ?? event.pause,
                      session: target.session,
                      pause: event.pause,
                      frame: null,
                    }),
                  }
                : row,
            );
            this.requests.delete(key);
          } else if (event.kind === "rejected") {
            if (!quiet) {
              this.debuggerError = event.message;
            }
            const key = `${target.session}/${item.request}`;
            const id = this.requests.get(key);
            this.debuggerConsole = this.debuggerConsole.map((row) =>
              row.id === id ? { ...row, output: event.message, thrown: true } : row,
            );
            this.requests.delete(key);
          }
        }
        for (const row of restored) {
          const value = this.inspection.set(row.value!.key, "Result", row.value!.value, {
            session: target.session,
            pause: row.pause ?? 0,
            frame: null,
            generation: row.generation ?? row.pause ?? 0,
          });
          this.debuggerConsole = this.debuggerConsole.map((current) =>
            current.id === row.id ? { ...current, value } : current,
          );
        }
        this.pendingConsoleValues = this.pendingConsoleValues.filter(
          (row) => row.session !== target.session,
        );
        this.eventCursors.set(target.session, view.stream.cursor);
        if (view.stream.closed) {
          for (const key of this.quietRequests) {
            if (key.startsWith(`${target.session}/`)) {
              this.quietRequests.delete(key);
            }
          }
          if (this.pauseControl?.session === target.session) {
            this.pauseControl = null;
          }
        }
      }
      this.debuggerEvidence = replies.flatMap(({ view }) => {
        const paused = view.stream.paused?.event;
        return evidence(view, paused?.kind === "paused" ? paused.exception : null).map((item) => ({
          ...item,
          logs: [],
        }));
      });
      this.inspection.expire();
      this.inspectionChanged();
      for (const id of this.views.keys()) {
        if (!workspace.targets.some((target) => target.session === id)) {
          this.views.delete(id);
          this.cursors.delete(id);
          this.eventCursors.delete(id);
          for (const key of this.quietRequests) {
            if (key.startsWith(`${id}/`)) {
              this.quietRequests.delete(key);
            }
          }
        }
      }
      if (this.debuggerPaused && !this.views.get(this.debuggerTarget)?.stream.paused) {
        this.clearPause();
      }
      this.consoleLogs.retain(this.views.values());
      if (!this.debuggerPaused) {
        this.debuggerStatus = this.debuggerTarget
          ? this.views.get(this.debuggerTarget)?.stream.closed
            ? "Worker ended"
            : this.views.get(this.debuggerTarget)?.stream.active
              ? "Running · breakpoints armed"
              : "Idle · breakpoints armed"
          : "Waiting for a worker";
        this.debuggerScope = this.views.get(this.debuggerTarget)?.stream.closed
          ? "Worker ended"
          : "Global";
      }
      await this.loadPositions();
      this.renderPoints();
      this.$flushUpdates();
      this.debuggerClosed = this.views.get(this.debuggerTarget)?.stream.closed ?? true;
      const selectedStream = this.views.get(this.debuggerTarget)?.stream;
      const request = this.debuggerPaused ? this.views.get(this.debuggerTarget)?.request : null;
      this.debuggerPauseContext = selectedStream?.operation?.callback
        ? `Paused in ${selectedStream.operation.callback}`
        : "Paused";
      this.debuggerRequestLabel = request
        ? `${request.method} ${request.url.replace(/^https?:\/\//u, "")}`
        : "";
      this.debuggerRequestUrl = request?.url ?? "";
      if (this.pauseControl?.session !== this.debuggerTarget || this.debuggerClosed) {
        this.debuggerPauseNext = !this.debuggerClosed && (selectedStream?.pauseNext ?? false);
      }
      this.debuggerPauseButton = this.debuggerPaused
        ? "Continue (F8)"
        : this.debuggerPauseNext
          ? "Cancel callback pause (F8)"
          : "Pause next callback (F8)";
      this.debuggerPauseIcon = this.debuggerPaused ? "continue" : "pause";
      this.debuggerWorkerOptions = this.debuggerTargets.map((target) => {
        const stream = this.views.get(target.session)?.stream;
        return {
          ...target,
          state: !stream
            ? "Attaching"
            : stream.closed
              ? "Ended"
              : stream.paused
                ? "Paused"
                : stream.active
                  ? "Running"
                  : "Idle",
        };
      });
      if (!this.debuggerPaused && !this.debuggerClosed) {
        this.debuggerStatus = this.debuggerPauseNext
          ? "Pause scheduled for next callback"
          : selectedStream?.skipPauses !== undefined &&
              selectedStream.skipPauses !== this.debuggerSkipPauses
            ? "Updating pause settings…"
            : `${selectedStream?.active ? "Running" : "Idle"} · ${this.debuggerSkipPauses ? "pauses skipped" : "breakpoints armed"}`;
      }
      const outcome = this.views.get(this.debuggerTarget)?.outcome;
      this.debuggerSummary = outcome?.summary ?? "";
      this.debuggerResult =
        outcome?.result === null || outcome?.result === undefined
          ? ""
          : JSON.stringify(outcome.result, null, 2).slice(0, 32768) +
            (outcome.truncated ? "\n[Result shortened]" : "");
      this.debuggerExceptions = this.views.get(this.debuggerTarget)?.stream.exceptions ?? "none";
      const mode =
        this.debuggerTargets.find((target) => target.session === this.debuggerTarget)?.mode ?? "";
      this.debuggerRestartable =
        mode === "Workspace script" ||
        mode.startsWith("Sandbox") ||
        mode.includes("visualizer") ||
        mode.includes("Visualizer");
      this.trimConsole();
      this.$flushUpdates();
      if (this.pendingConsoleScroll !== null) {
        this.consoleOutput.scrollTop = this.pendingConsoleScroll;
        this.pendingConsoleScroll = null;
      } else if (follow && this.consoleOutput.scrollTop === consoleScroll) {
        this.consoleOutput.scrollTop = this.consoleOutput.scrollHeight;
      }
    } catch (error) {
      if (this.isConnected) {
        this.debuggerError = describeError(error);
      }
    } finally {
      this.busy = false;
    }
  }
  private clearPause(): void {
    this.debuggerPaused = false;
    this.pause = 0;
    this.debuggerFrames = [];
    this.debuggerStackTruncated = false;
    this.resetInspection();
    this.updateStack();
    this.renderPoints();
  }
  private line(): number {
    return this.editor?.getPosition()?.lineNumber ?? 1;
  }
  private isSelected(file: File): boolean {
    return this.selected?.id === file.id && this.selected.artifact === file.artifact;
  }
  async openFile(file: File, remember = true, record = true): Promise<void> {
    const generation = ++this.sourceGeneration;
    try {
      const text = await invoke<string>("debugger_file", { id: file.id, artifact: file.artifact });
      if (
        !this.isConnected ||
        generation !== this.sourceGeneration ||
        !this.monaco ||
        !this.editor
      ) {
        return;
      }
      if (this.selected) {
        this.states.set(
          `${this.selected.id}@${this.selected.artifact}`,
          this.editor.saveViewState(),
        );
      }
      const previous = this.model;
      this.model = this.monaco.editor.createModel(text, file.language);
      this.editor.setModel(this.model);
      previous?.dispose();
      this.selected = file;
      const key = sourceKey(file);
      if (record && this.history[this.historyIndex] !== key) {
        this.history = [...this.history.slice(0, this.historyIndex + 1), key].slice(-64);
        this.historyIndex = this.history.length - 1;
      }
      if (!this.debuggerTabs.some((tab) => tab.key === key)) {
        this.debuggerTabs = [
          ...this.debuggerTabs.slice(-15),
          { key, file, label: file.path.split("/").at(-1)!, selected: true, tabIndex: 0 },
        ];
      }
      this.debuggerCompactPane = "source";
      this.updateNavigation();
      this.debuggerError = "";
      if (remember) {
        this.preferredRepresentation = file.language;
      }
      this.editor.restoreViewState(this.states.get(`${file.id}@${file.artifact}`) ?? null);
      this.debuggerSourceLabel = `${file.package}/${file.path}`;
      this.debuggerRepresentation = file.language;
      this.debuggerCanSwitch = this.files.some(
        (f) => f.artifact === file.artifact && f.id === file.counterpart,
      );
      while (this.states.size > 64) {
        this.states.delete(this.states.keys().next().value!);
      }
      await this.loadPositions();
      this.renderPoints();
      this.$flushUpdates();
      this.shadowRoot
        ?.getElementById(`source-tab-${key}`)
        ?.scrollIntoView({ block: "nearest", inline: "nearest" });
    } catch (error) {
      this.debuggerError = describeError(error);
    }
  }
  async switchRepresentation(event: Event): Promise<void> {
    const language = (event.target as HTMLSelectElement).value;
    const file = this.files.find(
      (file) =>
        file.artifact === this.selected?.artifact &&
        file.language === language &&
        file.id === this.selected.counterpart,
    );
    if (file) {
      await this.openFile(file);
    }
    const frame = this.debuggerFrames.find((frame) => frame.index === this.debuggerFrame);
    const position = language === "javascript" ? frame?.location : frame?.original;
    if (position && file && this.isSelected(file) && position.source === file.source) {
      this.editor?.revealPositionInCenter({ lineNumber: position.line, column: position.column });
    }
  }
  private async revealFrame(frame: DebugFrame | undefined): Promise<void> {
    const generation = this.inspectionGeneration;
    const location =
      this.preferredRepresentation === "javascript"
        ? frame?.location
        : (frame?.original ?? frame?.location);
    if (!location) {
      return;
    }
    const target = this.debuggerTargets.find((target) => target.session === this.debuggerTarget);
    const file = this.files.find(
      (file) => file.artifact === target?.artifact && file.source === location.source,
    );
    if (file) {
      await this.openFile(file, false);
      if (!this.isSelected(file) || generation !== this.inspectionGeneration) {
        return;
      }
    } else {
      this.debuggerError = "This frame has no available source snapshot.";
      return;
    }
    this.editor?.revealPositionInCenter({ lineNumber: location.line, column: location.column });
  }
  private async requestScopes(): Promise<void> {
    const context = this.inspectionScope();
    const generation = this.inspectionGeneration;
    const event = await this.inspection.request(
      { kind: "scopes", pause: context.pause, frame: context.frame ?? 0 },
      context,
    );
    if (event?.kind !== "scopes") {
      return;
    }
    if (
      !this.validInspection(context) ||
      generation !== this.inspectionGeneration ||
      context.session !== this.debuggerTarget
    ) {
      this.inspection.releaseLeases([
        {
          scope: context,
          handles: event.scopes.flatMap((scope) => (scope.handle ? [scope.handle] : [])),
        },
      ]);
      return;
    }
    this.debuggerScopeValues = event.scopes.map((scope, index) =>
      this.inspection.set(
        `scope-${index}`,
        scope.name ? `${scope.kind} · ${scope.name}` : scope.kind,
        {
          kind: "object",
          display: scope.handle ? "" : "Unavailable",
          handle: scope.handle,
          truncated: false,
        },
        context,
      ),
    );
    const local =
      this.debuggerScopeValues.find((scope) => scope.label === "local") ??
      this.debuggerScopeValues[0];
    if (local?.value.handle) {
      void this.inspection.toggle(local.key);
    }
  }
  async selectFrame(index: number): Promise<void> {
    if (!this.debuggerPaused || !this.debuggerFrames.some((frame) => frame.index === index)) {
      return;
    }
    this.resetInspection();
    this.debuggerFrame = index;
    const generation = this.inspectionGeneration;
    this.updateStack();
    await this.revealFrame(this.debuggerFrames.find((frame) => frame.index === this.debuggerFrame));
    if (generation !== this.inspectionGeneration || !this.debuggerPaused) {
      return;
    }
    void this.requestScopes();
    void this.refreshWatches();
  }
  async targetChanged(event: Event): Promise<void> {
    this.sourceGeneration++;
    this.debuggerTarget = (event.target as HTMLSelectElement).value;
    this.clearPause();
    this.cursors.delete(this.debuggerTarget);
    await this.refresh();
  }
  async navigateSource(
    event: CustomEvent<{ artifact: string; location: SourceLocation }>,
  ): Promise<void> {
    const file = this.files.find(
      (file) =>
        file.artifact === event.detail.artifact && file.source === event.detail.location.source,
    );
    if (!file) {
      this.debuggerError = "This source snapshot is no longer available.";
      return;
    }
    await this.openFile(file);
    if (!this.isSelected(file)) {
      return;
    }
    this.editor?.revealPositionInCenter({
      lineNumber: event.detail.location.line,
      column: event.detail.location.column,
    });
    this.editor?.focus();
  }
  async showPoint(point: Point): Promise<void> {
    const candidates = this.files.filter((file) => file.id === point.file);
    const artifact = this.debuggerTargets.find(
      (target) => target.session === this.debuggerTarget,
    )?.artifact;
    const file =
      candidates.find((file) => file.artifact === artifact) ??
      candidates.find((file) => file.state === "Loaded") ??
      candidates[0];
    if (file) {
      await this.openFile(file);
      if (!this.isSelected(file)) {
        return;
      }
      this.editor?.revealLineInCenter(
        this.positions.get(point.breakpoint.id)?.line ?? point.breakpoint.location.line,
      );
    }
  }
  clearConsole(): void {
    this.pendingConsoleScroll = null;
    for (const row of this.debuggerConsole) {
      this.inspection.remove(`console-${row.id}`);
    }
    this.debuggerConsole = [];
    this.requests.clear();
    this.logpointRows = [];
    this.diagnostics?.clearLogs();
    this.consoleLogs.clear(this.views.values());
    this.updateConsoleFeed();
  }
  consoleInteracted(): void {
    this.pendingConsoleScroll = null;
  }
  consoleFilter(event: Event, level = false): void {
    if (level) {
      this.debuggerConsoleLevel = (event.target as HTMLSelectElement).value;
    } else {
      this.debuggerConsoleFilter = (event.target as HTMLSelectElement).value;
    }
    this.updateConsoleFeed();
  }
  async consoleSource(row: ConsoleRow): Promise<void> {
    if (row.source) {
      await this.navigateSource(new CustomEvent("diagnostic-source", { detail: row.source }));
    }
  }
  private updateConsoleFeed(): void {
    const logs: ConsoleRow[] = [];
    const keys = new Map<string, number>();
    for (const view of this.views.values()) {
      for (const run of consoleRuns(view)) {
        const context = run.context;
        const base = consoleRunKey(view, run);
        const add = (key: string, row: Omit<ConsoleRow, "id">): void => {
          const id = this.logKeys.get(key) ?? ++this.serial;
          keys.set(key, id);
          logs.push({ ...row, id });
        };
        for (const record of this.consoleLogs.records(view, run)) {
          const frame = record.frames.find((frame) => frame.original ?? frame.generated);
          const location = frame?.original ?? frame?.generated;
          add(`${base}/${record.sequence}`, {
            input: "",
            output: record.text + (record.truncated ? "\n[Shortened]" : ""),
            thrown: record.level === "error",
            level: record.level,
            session: view.info.session,
            time: record.timeUnixMs,
            label: `${view.info.label} · ${context.callback}`,
            technical: `Worker: ${context.worker}\nRevision: ${context.revision}\nInvocation: ${context.invocation}${context.exchangeId ? `\nRequest: ${context.exchangeId}` : ""}`,
            ...(location && context.artifact
              ? { source: { location, artifact: context.artifact } }
              : {}),
          });
        }
        const loss = this.consoleLogs.loss(view, run);
        if (loss.dropped || loss.incomplete) {
          add(`${base}/loss`, {
            input: "",
            output: [
              loss.dropped ? `${loss.dropped} console records omitted` : "",
              loss.incomplete ? "Console capture incomplete" : "",
            ]
              .filter(Boolean)
              .join(" · "),
            thrown: false,
            level: "warn",
            session: view.info.session,
            time: run.records.at(-1)?.timeUnixMs ?? 0,
            label: view.info.label,
          });
        }
      }
    }
    this.logKeys = keys;
    this.debuggerFeed = [...logs, ...this.logpointRows, ...this.debuggerConsole]
      .filter(
        (row) =>
          (this.debuggerConsoleFilter === "all" || row.session === this.debuggerTarget) &&
          (this.debuggerConsoleLevel === "all" ||
            !row.level ||
            row.level === this.debuggerConsoleLevel),
      )
      .sort((a, b) => (a.time ?? 0) - (b.time ?? 0) || a.id - b.id)
      .slice(-200);
  }
  private trimConsole(): void {
    const previous = this.debuggerConsole;
    let remaining = 128 * 1024;
    this.debuggerConsole = this.debuggerConsole
      .slice(-100)
      .map((row) => ({
        ...row,
        output:
          row.output.length > 8192
            ? row.output.slice(0, 8192) + "\n[Output shortened]"
            : row.output,
      }))
      .reverse()
      .filter((row) => {
        remaining -= row.input.length + row.output.length;
        return remaining >= 0;
      })
      .reverse();
    const retained = new Set(this.debuggerConsole.map((row) => row.id));
    for (const row of previous) {
      if (!retained.has(row.id)) {
        this.inspection.remove(`console-${row.id}`);
      }
    }
    for (const [key, id] of this.requests) {
      if (!retained.has(id)) {
        this.requests.delete(key);
      }
    }
    this.updateConsoleFeed();
  }
  async submitConsole(event: Event): Promise<void> {
    event.preventDefault();
    await this.evaluate();
  }
  async resume(action: "continue" | "over" | "into" | "out"): Promise<void> {
    if (!this.debuggerPaused) {
      return;
    }
    await this.action({ kind: "resume", pause: this.pause, action });
  }
  async togglePause(): Promise<void> {
    if (this.debuggerPaused) {
      await this.resume("continue");
      return;
    }
    if (this.debuggerClosed || this.debuggerControlSubmitting) {
      return;
    }
    const session = this.debuggerTarget;
    const enabled = !this.debuggerPauseNext;
    this.debuggerControlSubmitting = true;
    try {
      if (enabled && this.debuggerSkipPauses) {
        await invoke("workspace_skip_pauses", { skip: false });
        this.debuggerSkipPauses = false;
      }
      const request = await this.action({ kind: "pause-next", enabled }, session);
      if (request !== undefined) {
        this.pauseControl = { session, request };
        if (session === this.debuggerTarget) {
          this.debuggerPauseNext = enabled;
        }
      }
    } catch (error) {
      this.debuggerError = describeError(error);
    } finally {
      this.debuggerControlSubmitting = false;
    }
    await this.refresh();
  }
  async toggleSkipPauses(): Promise<void> {
    if (this.debuggerControlSubmitting) {
      return;
    }
    this.debuggerControlSubmitting = true;
    try {
      await invoke("workspace_skip_pauses", { skip: !this.debuggerSkipPauses });
      this.debuggerError = "";
    } catch (error) {
      this.debuggerError = describeError(error);
    } finally {
      this.debuggerControlSubmitting = false;
    }
    await this.refresh();
  }
  async continueTo(line: number): Promise<void> {
    const file = this.selected;
    if (
      !file ||
      !this.debuggerPaused ||
      this.views.get(this.debuggerTarget)?.info.artifact !== file.artifact
    ) {
      return;
    }
    const scope = this.inspectionScope();
    try {
      if (this.debuggerSkipPauses) {
        await invoke("workspace_skip_pauses", { skip: false });
      }
      if (!this.validInspection(scope) || scope.session !== this.debuggerTarget) {
        return;
      }
      await this.action(
        {
          kind: "continue-to",
          pause: scope.pause,
          location: { source: file.source, line, column: 1 },
        },
        scope.session,
      );
    } catch (error) {
      this.debuggerError = describeError(error);
    }
  }
  async stop(): Promise<void> {
    if (!this.debuggerClosed) {
      await this.action({ kind: "stop" });
    }
  }
  async revealRequest(): Promise<void> {
    if (!this.debuggerPaused || !this.debuggerRequestLabel) {
      return;
    }
    try {
      await invoke("workspace_debug_request", { session: this.debuggerTarget, pause: this.pause });
    } catch (error) {
      this.debuggerError = describeError(error);
    }
  }
  async policy(event: Event): Promise<void> {
    const policy = (event.target as HTMLSelectElement).value;
    if (policy === "none" || policy === "uncaught" || policy === "all") {
      await this.action({ kind: "exceptions", policy });
    }
  }
  async restart(): Promise<void> {
    if (!this.debuggerRestartable) {
      return;
    }
    try {
      await invoke("workspace_restart", { session: this.debuggerTarget });
    } catch (error) {
      this.debuggerError = describeError(error);
    }
  }
  private async action(
    action: DebugAction,
    session = this.debuggerTarget,
    quiet = false,
  ): Promise<number | undefined> {
    if (!session) {
      return;
    }
    if (quiet) {
      this.quietSubmitting++;
    }
    try {
      const request = await invoke<number>("workspace_debug_command", {
        session,
        action,
      });
      if (!quiet) {
        this.debuggerError = "";
      }
      if (quiet) {
        this.quietRequests.add(`${session}/${request}`);
      }
      return request;
    } catch (error) {
      if (!quiet) {
        this.debuggerError = describeError(error);
      }
      return;
    } finally {
      if (quiet) {
        this.quietSubmitting--;
      }
    }
  }
  private point(line: number): Point | undefined {
    return this.points.find((point) => this.positions.get(point.breakpoint.id)?.line === line);
  }
  async toggle(line: number): Promise<void> {
    if (!this.selected || !this.model || this.selected.state === "Retired") {
      return;
    }
    const previous = this.point(line);
    const points = previous
      ? this.points.filter((point) => point !== previous)
      : [
          ...this.points,
          {
            artifact: this.selected.artifact,
            file: this.selected.id,
            enabled: true,
            anchor: this.model.getLineContent(line),
            breakpoint: {
              id: Math.max(0, ...this.points.map((point) => point.breakpoint.id)) + 1,
              location: { source: this.selected.source, line, column: 1 },
            },
          },
        ];
    await this.savePoints(points);
  }
  private async savePoints(points: Point[]): Promise<boolean> {
    const changed = points.filter(
      (point) =>
        !this.points.some((previous) => JSON.stringify(previous) === JSON.stringify(point)),
    );
    const removed = this.points.filter(
      (previous) => !points.some((point) => point.breakpoint.id === previous.breakpoint.id),
    );
    this.points = points;
    this.pointEditRevision++;
    this.pointEditsPending++;
    this.renderPoints();
    const submission = this.pointEdits.then(() =>
      invoke<Point[]>("workspace_breakpoints", { points: changed, removed }),
    );
    this.pointEdits = submission.catch(() => undefined);
    try {
      const saved = await submission;
      this.debuggerError = "";
      if (this.pointEditsPending === 1) {
        this.points = saved;
      }
      return true;
    } catch (error) {
      this.debuggerError = describeError(error);
      return false;
    } finally {
      this.pointEditsPending--;
      if (!this.pointEditsPending) {
        this.projectionKey = "";
        await this.loadPositions();
        this.renderPoints();
        await this.refresh();
      }
    }
  }
  async removePoint(point: Point): Promise<void> {
    await this.savePoints(this.points.filter((p) => p.breakpoint.id !== point.breakpoint.id));
  }
  async pointChecked(point: Point, event: Event): Promise<void> {
    const enabled = (event.target as HTMLInputElement).checked;
    await this.savePoints(
      this.points.map((current) =>
        current.breakpoint.id === point.breakpoint.id ? { ...current, enabled } : current,
      ),
    );
  }
  async enableAllPoints(enabled: boolean): Promise<void> {
    await this.savePoints(this.points.map((point) => ({ ...point, enabled })));
  }
  breakpointGroupToggled(group: PointGroup, event: Event): void {
    this.sourceGroups.set(group.key, (event.target as HTMLDetailsElement).open);
  }
  async editPoint(point: Point): Promise<void> {
    await this.showPoint(point);
    if (this.selected?.id === point.file) {
      this.condition(
        this.positions.get(point.breakpoint.id)?.line ?? point.breakpoint.location.line,
        point.breakpoint.logExpression ? "log" : "pause",
        point,
      );
    }
  }
  async disable(line: number): Promise<void> {
    const point = this.point(line);
    if (point) {
      await this.savePoints(
        this.points.map((p) => (p === point ? { ...p, enabled: !p.enabled } : p)),
      );
    }
  }
  condition(line: number, kind = "", explicit?: Point): void {
    if (!this.selected || !this.model || this.selected.state === "Retired") {
      return;
    }
    this.debuggerError = "";
    const point = explicit ?? this.point(line);
    this.conditionContext = {
      file: this.selected,
      line,
      anchor: this.model.getLineContent(line),
      point,
    };
    this.debuggerConditionSource = `${this.selected.package}/${this.selected.path}:${line}`;
    this.debuggerCondition = point?.breakpoint.condition ?? "";
    this.debuggerConditionKind = kind || (point?.breakpoint.logExpression ? "log" : "pause");
    this.debuggerConditionTitle =
      this.debuggerConditionKind === "log" ? "Logpoint" : "Breakpoint condition";
    this.debuggerLogExpression = point?.breakpoint.logExpression ?? "";
    this.conditionDialog.showModal();
  }
  conditionInput(event: Event): void {
    this.debuggerCondition = (event.target as HTMLInputElement).value;
  }
  logExpressionInput(event: Event): void {
    this.debuggerLogExpression = (event.target as HTMLInputElement).value;
  }
  conditionKindChanged(event: Event): void {
    this.debuggerConditionKind = (event.target as HTMLSelectElement).value;
    this.debuggerConditionTitle =
      this.debuggerConditionKind === "log" ? "Logpoint" : "Breakpoint condition";
  }
  async saveCondition(event: Event): Promise<void> {
    event.preventDefault();
    if (this.debuggerConditionSaving) {
      return;
    }
    const context = this.conditionContext;
    if (!context) {
      return;
    }
    const condition = this.debuggerCondition.trim();
    const logExpression =
      this.debuggerConditionKind === "log" ? this.debuggerLogExpression.trim() : "";
    if (
      (this.debuggerConditionKind === "log" && !logExpression) ||
      new TextEncoder().encode(condition).length > 4096 ||
      new TextEncoder().encode(logExpression).length > 4096
    ) {
      this.debuggerError =
        logExpression || this.debuggerConditionKind !== "log"
          ? "Keep each expression under 4 KiB."
          : "Enter an expression to log.";
      return;
    }
    if (
      !this.files.some(
        (file) =>
          file.id === context.file.id &&
          file.artifact === context.file.artifact &&
          file.state !== "Retired",
      )
    ) {
      this.debuggerError = "This source revision is no longer available.";
      return;
    }
    const previous =
      context.point &&
      this.points.find((point) => point.breakpoint.id === context.point!.breakpoint.id);
    if (context.point && !previous) {
      this.debuggerError = "This breakpoint changed. Reopen its condition.";
      return;
    }
    const point: Point = previous ?? {
      artifact: context.file.artifact,
      file: context.file.id,
      enabled: true,
      anchor: context.anchor,
      breakpoint: {
        id: Math.max(0, ...this.points.map((point) => point.breakpoint.id)) + 1,
        location: { source: context.file.source, line: context.line, column: 1 },
      },
    };
    const changed = {
      ...point,
      breakpoint: {
        ...point.breakpoint,
        condition: condition || undefined,
        logExpression: logExpression || undefined,
      },
    };
    this.debuggerConditionSaving = true;
    try {
      if (
        (await this.savePoints([...this.points.filter((p) => p !== previous), changed])) &&
        this.conditionContext === context
      ) {
        this.closeCondition();
      }
    } finally {
      this.debuggerConditionSaving = false;
    }
  }
  closeCondition(): void {
    delete this.conditionContext;
    if (this.conditionDialog.open) {
      this.conditionDialog.close();
      this.editor?.focus();
    }
  }
  cancelCondition(event: Event): void {
    event.preventDefault();
    this.closeCondition();
  }
  consoleInput(event: Event): void {
    this.debuggerInput = (event.target as HTMLTextAreaElement).value;
    this.commandHistory.reset();
    this.closeCompletions();
    window.clearTimeout(this.completionTimer);
    this.completionTimer = window.setTimeout(() => {
      void this.completePrompt();
    }, 120);
  }
  async consoleKey(event: KeyboardEvent): Promise<void> {
    if (event.isComposing) {
      return;
    }
    if (this.debuggerCompletions.length) {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.closeCompletions();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        this.debuggerCompletionIndex =
          (this.debuggerCompletionIndex +
            (event.key === "ArrowDown" ? 1 : -1) +
            this.debuggerCompletions.length) %
          this.debuggerCompletions.length;
        this.debuggerCompletions = this.debuggerCompletions.map((item, index) => ({
          ...item,
          selected: index === this.debuggerCompletionIndex,
        }));
        this.debuggerCompletionActive = `completion-${this.debuggerCompletions[this.debuggerCompletionIndex]!.name}`;
        return;
      }
      if (event.key === "Tab") {
        event.preventDefault();
        this.acceptCompletion(this.debuggerCompletions[this.debuggerCompletionIndex]!.name);
        return;
      }
    }
    if (event.ctrlKey && event.key === " ") {
      event.preventDefault();
      await this.completePrompt();
      return;
    }
    const prompt = this.consolePrompt;
    if (
      (event.key === "ArrowUp" && !prompt.value.slice(0, prompt.selectionStart).includes("\n")) ||
      (event.key === "ArrowDown" && !prompt.value.slice(prompt.selectionEnd).includes("\n"))
    ) {
      if (
        !event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        prompt.selectionStart === prompt.selectionEnd
      ) {
        event.preventDefault();
        this.closeCompletions();
        this.debuggerInput = this.commandHistory.move(
          event.key === "ArrowUp" ? -1 : 1,
          this.debuggerInput,
        );
        this.$flushUpdates();
        prompt.setSelectionRange(prompt.value.length, prompt.value.length);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      await this.evaluate();
    }
  }
  async evaluate(expression?: string): Promise<void> {
    const draft = this.debuggerInput;
    const input = (expression ?? draft).trim();
    if (!input || !this.debuggerTarget || this.debuggerClosed || this.debuggerSubmitting) {
      return;
    }
    const session = this.debuggerTarget;
    const generation = this.generation;
    const id = ++this.serial;
    this.pendingConsoleScroll = null;
    this.debuggerSubmitting = true;
    this.closeCompletions();
    this.commandHistory.add(input, { preserveNavigation: expression !== undefined });
    this.debuggerConsole = [
      ...this.debuggerConsole,
      {
        id,
        input,
        output: "Running…",
        thrown: false,
        session,
        pause: this.pause,
        time: Date.now(),
      },
    ];
    this.trimConsole();
    this.$flushUpdates();
    this.consoleOutput.scrollTop = this.consoleOutput.scrollHeight;
    try {
      this.evaluationSubmission = this.action(
        {
          kind: "evaluate",
          expression: input,
          pause: this.pause,
          frame: this.debuggerPaused ? this.debuggerFrame : null,
        },
        session,
      );
      const request = await this.evaluationSubmission;
      if (!this.isConnected || generation !== this.generation) {
        return;
      }
      if (request === undefined) {
        this.debuggerConsole = this.debuggerConsole.map((row) =>
          row.id === id ? { ...row, output: this.debuggerError, thrown: true } : row,
        );
        return;
      }
      if (this.debuggerConsole.some((row) => row.id === id)) {
        this.requests.set(`${session}/${request}`, id);
      }
      if (expression === undefined && this.debuggerInput === draft) {
        this.debuggerInput = "";
      }
    } finally {
      this.evaluationSubmission = null;
      this.debuggerSubmitting = false;
    }
  }
  async place(placement: string): Promise<void> {
    if (placement !== "closed" && this.debuggerTransition) {
      return;
    }
    this.debuggerTransition = placement === "closed" ? "Closing debugger…" : "Moving debugger…";
    this.$flushUpdates();
    try {
      await this.pointEdits;
      await this.watchEdit;
      if (placement !== "closed" && this.evaluationSubmission) {
        // Transfer the console only after its submission can be correlated.
        await this.evaluationSubmission;
        if (!this.isConnected) {
          return;
        }
      }
      this.trimConsole();
      if (this.selected) {
        this.states.set(
          `${this.selected.id}@${this.selected.artifact}`,
          this.editor?.saveViewState() ?? null,
        );
      }
      const viewState: SavedView = {
        valueLeases: this.inspection.leases(),
        logpoints: this.logpointRows,
        eventCursors: [...this.eventCursors],
        watches: this.debuggerWatches.map((watch) => watch.expression),
        watchInput: this.debuggerWatchInput,
        watchEdit: this.debuggerWatchEditing
          ? {
              expression:
                this.debuggerWatches.find((watch) => watch.id === this.debuggerWatchEditing)
                  ?.expression ?? "",
              draft: this.debuggerWatchDraft,
            }
          : undefined,
        consoleHistory: this.commandHistory.snapshot(),
        consoleFilter: this.debuggerConsoleFilter,
        consoleLevel: this.debuggerConsoleLevel,
        clearedLogs: this.consoleLogs.snapshot(),
        groups: [...this.sourceGroups],
        split: this.debuggerConsoleSplit,
        file: this.selected?.id,
        artifact: this.selected?.artifact,
        target: this.debuggerTarget,
        focus: this.lastFocus,
        requests: [...this.requests],
        frame: this.debuggerFrame,
        input: this.debuggerInput,
        console: this.debuggerConsole.map((row) =>
          row.value
            ? {
                ...row,
                value: { ...row.value, expanded: false, rows: [], loading: false, issue: "" },
              }
            : row,
        ),
        states: [...this.states].slice(-64),
        tabs: this.debuggerTabs.map((tab) => tab.key),
        history: this.history,
        historyIndex: this.historyIndex,
        diagnostics: this.diagnostics?.saveView(),
        scroll: this.consoleOutput.scrollTop,
      };
      await invoke("place_debugger", { placement, viewState });
    } catch (error) {
      this.debuggerTransition = "";
      this.debuggerError = describeError(error);
    }
  }
  private addEditorAction(action: Monaco.editor.IActionDescriptor): void {
    if (this.editor) {
      this.editorRegistrations.push(this.editor.addAction(action));
    }
  }
  keyboard(event: KeyboardEvent): void {
    if (
      (event.key === "Escape" || event.key === "Enter") &&
      event
        .composedPath()
        .some((node) => node instanceof HTMLFormElement && node.classList.contains("watch-edit"))
    ) {
      this.watchEditKey(event);
      return;
    }
    if (event.key === "Escape" && this.debuggerCompletions.length && !event.isComposing) {
      event.preventDefault();
      event.stopPropagation();
      this.closeCompletions();
      return;
    }
    if (
      !event.defaultPrevented &&
      !event.isComposing &&
      !this.conditionDialog.open &&
      !this.navigationDialog.open
    ) {
      if (
        event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        (event.key.toLowerCase() === "p" || (event.shiftKey && event.key.toLowerCase() === "f"))
      ) {
        event.preventDefault();
        event.stopPropagation();
        this.openNavigation(event.shiftKey);
        return;
      }
      if (
        event.altKey &&
        !event.ctrlKey &&
        (event.key === "ArrowLeft" || event.key === "ArrowRight")
      ) {
        event.preventDefault();
        event.stopPropagation();
        void this.navigateHistory(event.key === "ArrowLeft" ? -1 : 1);
        return;
      }
      if (event.key === "Escape" && event.eventPhase !== Event.CAPTURING_PHASE) {
        event.preventDefault();
        this.toggleConsole();
        return;
      }
    }
    if (
      this.conditionDialog.open ||
      this.navigationDialog.open ||
      event.defaultPrevented ||
      event.isComposing ||
      event.repeat ||
      event.altKey ||
      event.metaKey
    ) {
      return;
    }
    if (event.key === "F5" && !event.ctrlKey) {
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) {
        void this.stop();
      } else {
        void this.resume("continue");
      }
      return;
    }
    const action =
      event.key === "F8" || (event.ctrlKey && event.key === "\\")
        ? "continue"
        : event.key === "F10" && !event.shiftKey
          ? "over"
          : event.key === "F11"
            ? event.shiftKey
              ? "out"
              : "into"
            : null;
    if (action) {
      event.preventDefault();
      event.stopPropagation();
      if (action === "continue") {
        void this.togglePause();
      } else {
        void this.resume(action);
      }
    }
  }
  private async loadPositions(): Promise<void> {
    if (!this.selected) {
      return;
    }
    const file = this.selected;
    const key = JSON.stringify([file.id, file.artifact, this.points]);
    if (key === this.projectionKey) {
      return;
    }
    const positions = await invoke<Array<[number, SourceLocation]>>(
      "debugger_breakpoint_positions",
      { id: file.id, artifact: file.artifact },
    );
    if (this.selected === file) {
      this.positions = new Map(positions);
      this.projectionKey = key;
    }
  }
  private renderPoints(): void {
    this.debuggerPoints = this.points.map((point) => {
      const file = this.files.find((file) => file.id === point.file);
      return {
        ...point,
        label: `${file ? `${file.package}/${file.path}` : point.file}:${point.breakpoint.location.line}${point.breakpoint.condition ? " · conditional" : ""}${point.enabled ? "" : " · disabled"}`,
      };
    });
    const bindings = [...this.views.values()]
      .filter((view) => !view.stream.closed)
      .flatMap((view) => view.stream.inventory?.breakpoints ?? []);
    this.debuggerPointGroups = breakpointGroups(this.points, bindings, this.sourceGroups);
    this.debuggerEnabledPoints = this.points.filter((point) => point.enabled).length;
    this.continueContext?.set(
      this.debuggerPaused &&
        this.selected?.artifact === this.views.get(this.debuggerTarget)?.info.artifact,
    );
    if (!this.editor || !this.monaco || !this.selected) {
      return;
    }
    const sourceBindings = [...this.views.values()]
      .filter((view) => !view.stream.closed && view.info.artifact === this.selected?.artifact)
      .flatMap((view) => view.stream.inventory?.breakpoints ?? []);
    const decorations: Monaco.editor.IModelDeltaDecoration[] = [];
    for (const point of this.points) {
      const bound = sourceBindings.find((binding) => binding.id === point.breakpoint.id)?.resolved;
      const position = this.positions.get(point.breakpoint.id);
      if (!position) {
        continue;
      }
      decorations.push({
        range: new this.monaco.Range(position.line, 1, position.line, 1),
        options: {
          glyphMarginClassName: point.breakpoint.logExpression
            ? !point.enabled
              ? "debug-logpoint-disabled"
              : bound
                ? "debug-logpoint"
                : "debug-logpoint-pending"
            : !point.enabled
              ? "debug-breakpoint-disabled"
              : bound
                ? "debug-breakpoint"
                : "debug-breakpoint-pending",
          glyphMarginHoverMessage: {
            value: point.breakpoint.logExpression
              ? `Log ${point.breakpoint.logExpression}${point.breakpoint.condition ? ` if ${point.breakpoint.condition}` : ""}`
              : point.breakpoint.condition
                ? `Breakpoint if ${point.breakpoint.condition}`
                : "Breakpoint",
          },
        },
      });
    }
    const frame = this.debuggerFrames.find((frame) => frame.index === this.debuggerFrame);
    const position = this.selected.language === "javascript" ? frame?.location : frame?.original;
    const target = this.debuggerTargets.find((target) => target.session === this.debuggerTarget);
    if (
      this.debuggerPaused &&
      target?.artifact === this.selected.artifact &&
      position?.source === this.selected.source
    ) {
      decorations.push({
        range: new this.monaco.Range(position.line, 1, position.line, 1),
        options: {
          isWholeLine: true,
          className: "debug-execution-line",
          glyphMarginClassName: "debug-execution",
        },
      });
      const text = this.model?.getLineContent(position.line) ?? "";
      const locals =
        this.debuggerScopeValues.find((scope) => scope.label.startsWith("local"))?.rows ?? [];
      const values = locals
        .filter(
          (row) =>
            row.depth === 0 &&
            !row.value.handle &&
            ["number", "boolean", "string", "null", "undefined"].includes(row.value.kind) &&
            /^[A-Za-z_$][\w$]*$/.test(row.name) &&
            new RegExp(`\\b${row.name.replaceAll("$", "\\$")}\\b`).test(text),
        )
        .slice(0, 3)
        .map((row) => `${row.name} = ${row.value.display.slice(0, 60)}`);
      if (values.length) {
        decorations.push({
          range: new this.monaco.Range(
            position.line,
            text.length + 1,
            position.line,
            text.length + 1,
          ),
          options: {
            showIfCollapsed: true,
            after: {
              content: `  ${values.join(" · ").replace(/[\r\n]/g, " ")}`,
              inlineClassName: "debug-inline-value",
            },
          },
        });
      }
    }
    this.decorations?.set(decorations);
  }
  disconnectedCallback(): void {
    this.inspection.dispose();
    this.inspectionGeneration++;
    this.closeCompletions();
    window.clearTimeout(this.completionTimer);
    for (const provider of this.editorRegistrations.splice(0)) {
      provider.dispose();
    }
    this.resizeObserver?.disconnect();
    window.clearTimeout(this.navigationTimer);
    this.navigationGeneration++;
    document.removeEventListener("keydown", this.toggleShortcut, true);
    this.removeEventListener("keydown", this.shortcutListener, true);
    this.focusUnlisten?.();
    this.movingUnlisten?.();
    this.toggleUnlisten?.();
    this.focusUnlisten = null;
    this.generation++;
    this.sourceGeneration++;
    window.clearInterval(this.timer);
    this.editor?.dispose();
    this.model?.dispose();
    super.disconnectedCallback();
  }
}
DebuggerWorkspace.define("debugger-workspace");
