import "../pane-divider/pane-divider.js";
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
import { PropertyTree, type PropertyRow } from "./properties.js";
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
  DebugScope,
  SourceBreakpoint,
} from "../debugger-workspace/models.js";
import type { DeveloperEvidence } from "../script-diagnostics/models.js";
import "../script-diagnostics/script-diagnostics.js";
import { describeError } from "../utilities.js";
interface DebuggerLayout {
  sourcesSplit: number;
  inspectorSplit: number;
  consoleSplit: number;
  sourcesVisible: boolean;
  inspectorVisible: boolean;
  consoleVisible: boolean;
}
interface Point {
  artifact?: string;
  file: string;
  breakpoint: SourceBreakpoint;
  enabled: boolean;
  anchor: string;
}
interface View {
  focus?: string | null;
  theme?: string;
  open: boolean;
  files: File[];
  targets: DebugSessionInfo[];
  breakpoints: Point[];
  viewState?: SavedView;
  layout?: DebuggerLayout;
}
interface SavedView {
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
  console: Array<{ id: number; input: string; output: string; thrown: boolean }>;
  states: Array<[string, Monaco.editor.ICodeEditorViewState | null]>;
  tabs?: string[];
  history?: string[];
  historyIndex?: number;
}
export class DebuggerWorkspace extends WebUIElement {
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
  @observable debuggerScopes: DebugScope[] = [];
  @observable debuggerProperties: PropertyRow[] = [];
  @observable debuggerPoints: Array<Point & { label: string }> = [];
  @observable debuggerEvidence: DeveloperEvidence[] = [];
  @observable debuggerConsole: Array<{
    id: number;
    input: string;
    output: string;
    thrown: boolean;
  }> = [];
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
  private readonly propertyTree = new PropertyTree();
  private propertyBranches = new Map<number, string>();
  private focusUnlisten: UnlistenFn | null = null;
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
    try {
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
        if (event.target.position) {
          this.editor?.setPosition(event.target.position);
        }
        if (
          event.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ||
          event.target.type === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS
        ) {
          this.editor?.focus();
          void this.editor?.getAction("editor.action.showContextMenu")?.run();
        }
      });
      for (const [id, label, run] of [
        ["breakpoint", "Toggle breakpoint", () => this.toggle(this.line())],
        ["condition", "Add or edit conditional breakpoint", () => this.condition(this.line())],
        ["disable", "Enable or disable breakpoint", () => this.disable(this.line())],
      ] as const) {
        this.editor.addAction({
          id: `debugger.${id}`,
          label,
          contextMenuGroupId: "debugger",
          run: () => {
            void run();
          },
        });
      }
      const ready = await invoke<View>("acknowledge_debugger");
      if (!this.isConnected || version !== this.generation) {
        return;
      }
      this.apply(ready);
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
        this.debuggerConsole = saved.console;
        this.serial = Math.max(0, ...saved.console.map((row) => row.id));
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
        this.consoleOutput.scrollTop = saved.scroll ?? 0;
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
    }
  }
  private apply(view: View): void {
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
    this.points = view.breakpoints;
    this.updateNavigation();
    this.debuggerTargets = view.targets;
    if (!view.targets.some((t) => t.session === this.debuggerTarget)) {
      this.debuggerTarget = view.targets[0]?.session ?? "";
    }
    this.debuggerPoints = this.points.map((point) => {
      const file = this.files.find((file) => file.id === point.file);
      return {
        ...point,
        label: `${file ? `${file.package}/${file.path}` : point.file}:${point.breakpoint.location.line}${point.breakpoint.condition ? " · conditional" : ""}${point.enabled ? "" : " · disabled"}`,
      };
    });
    this.renderPoints();
  }
  private async refresh(): Promise<void> {
    if (this.busy || this.debuggerSubmitting || !this.isConnected) {
      return;
    }
    this.busy = true;
    const generation = this.generation;
    const follow =
      this.consoleOutput.scrollHeight -
        this.consoleOutput.scrollTop -
        this.consoleOutput.clientHeight <
      24;
    try {
      const workspace = await invoke<View>("debugger_workspace");
      const replies = await Promise.all(
        workspace.targets.map(async (target) => ({
          target,
          view: await invoke<DebugSessionView>("workspace_debug_session", {
            session: target.session,
            after: this.cursors.get(target.session) ?? 0,
          }),
        })),
      );
      // Keep evaluation events unread until the submission's request ID is known.
      if (!this.isConnected || generation !== this.generation || this.debuggerSubmitting) {
        return;
      }
      this.apply(workspace);
      if (
        replies.find((reply) => reply.target.session === this.debuggerTarget)?.view.stream.closed
      ) {
        this.clearPause();
      }
      for (const { target, view } of replies) {
        this.cursors.set(target.session, view.stream.cursor);
        const previous = this.views.get(target.session);
        if (!view.stream.inventory && previous?.stream.inventory) {
          view.stream.inventory = previous.stream.inventory;
        }
        for (const item of view.stream.events) {
          if (item.event.kind === "sources") {
            view.stream.inventory = {
              sources: item.event.sources,
              breakpoints: item.event.breakpoints,
            };
          }
        }
        this.views.set(target.session, view);
        if (
          target.session === this.debuggerTarget &&
          view.stream.paused &&
          (!this.debuggerPaused ||
            this.pause !== (view.stream.paused.event as { pause?: number }).pause) &&
          !view.stream.events.some((item) => item.event.kind === "paused")
        ) {
          view.stream.events.unshift(view.stream.paused);
        }
        for (const item of view.stream.events) {
          const event = item.event;
          if (
            event.kind === "paused" &&
            (!this.debuggerPaused || target.session === this.debuggerTarget)
          ) {
            this.debuggerTarget = target.session;
            this.pause = event.pause;
            this.debuggerPaused = true;
            this.propertyTree.reset();
            this.propertyBranches.clear();
            this.debuggerStackTruncated = event.truncated;
            this.debuggerFrames = event.frames;
            this.debuggerFrame =
              event.frames.find((frame) => frame.index === this.restoreFrame)?.index ??
              event.frames[0]?.index ??
              0;
            this.restoreFrame = null;
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
              await this.action({ kind: "scopes", pause: this.pause, frame: this.debuggerFrame });
            }
          } else if (event.kind === "resumed" && target.session === this.debuggerTarget) {
            this.clearPause();
          } else if (
            event.kind === "scopes" &&
            target.session === this.debuggerTarget &&
            event.frame === this.debuggerFrame &&
            event.pause === this.pause
          ) {
            this.debuggerScopes = event.scopes;
            const scope =
              event.scopes.find((s) => s.kind === "local" && s.handle) ??
              event.scopes.find((s) => s.handle);
            if (scope?.handle) {
              await this.selectScope(scope.handle);
            }
          } else if (
            event.kind === "properties" &&
            target.session === this.debuggerTarget &&
            this.propertyBranches.has(item.request) &&
            event.pause === this.pause
          ) {
            const key = this.propertyBranches.get(item.request)!;
            this.propertyBranches.delete(item.request);
            this.propertyTree.receive(key, event.properties, event.offset, event.next);
            this.debuggerProperties = this.propertyTree.rows();
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
            this.debuggerConsole = this.debuggerConsole.map((row) =>
              row.id === id
                ? {
                    ...row,
                    output: `${row.output === "Running…" ? "" : row.output + "\n"}${event.value.display}`,
                    thrown: event.thrown,
                  }
                : row,
            );
            this.requests.delete(key);
          } else if (event.kind === "rejected") {
            this.propertyBranches.delete(item.request);
            this.debuggerError = event.message;
            const key = `${target.session}/${item.request}`;
            const id = this.requests.get(key);
            this.debuggerConsole = this.debuggerConsole.map((row) =>
              row.id === id ? { ...row, output: event.message, thrown: true } : row,
            );
            this.requests.delete(key);
          }
        }
      }
      this.debuggerEvidence = replies.flatMap(({ view }) => {
        const paused = view.stream.paused?.event;
        return evidence(view, paused?.kind === "paused" ? paused.exception : null);
      });
      for (const id of this.views.keys()) {
        if (!workspace.targets.some((target) => target.session === id)) {
          this.views.delete(id);
          this.cursors.delete(id);
        }
      }
      if (this.debuggerPaused && !this.views.get(this.debuggerTarget)?.stream.paused) {
        this.clearPause();
      }
      if (!this.debuggerPaused) {
        this.debuggerStatus = this.debuggerTarget
          ? this.views.get(this.debuggerTarget)?.stream.closed
            ? "Worker ended"
            : "Running · breakpoints armed"
          : "Waiting for a worker";
        this.debuggerScope = this.views.get(this.debuggerTarget)?.stream.closed
          ? "Worker ended"
          : "Global";
      }
      await this.loadPositions();
      this.renderPoints();
      this.$flushUpdates();
      this.debuggerClosed = this.views.get(this.debuggerTarget)?.stream.closed ?? true;
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
      if (follow) {
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
    this.debuggerScopes = [];
    this.debuggerProperties = [];
    this.debuggerStackTruncated = false;
    this.propertyTree.reset();
    this.propertyBranches.clear();
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
      if (!this.isSelected(file)) {
        return;
      }
    } else {
      this.debuggerError = "This frame has no available source snapshot.";
      return;
    }
    this.editor?.revealPositionInCenter({ lineNumber: location.line, column: location.column });
  }
  async frameChanged(event: Event): Promise<void> {
    this.propertyTree.reset();
    this.propertyBranches.clear();
    this.debuggerProperties = [];
    this.debuggerFrame = Number((event.target as HTMLSelectElement).value);
    await this.revealFrame(this.debuggerFrames.find((frame) => frame.index === this.debuggerFrame));
    await this.action({ kind: "scopes", pause: this.pause, frame: this.debuggerFrame });
  }
  async targetChanged(event: Event): Promise<void> {
    this.sourceGeneration++;
    this.debuggerTarget = (event.target as HTMLSelectElement).value;
    this.clearPause();
    this.cursors.delete(this.debuggerTarget);
    await this.refresh();
  }
  async selectScope(handle: number | null): Promise<void> {
    if (!handle) {
      return;
    }
    this.propertyBranches.clear();
    await this.readProperties(this.propertyTree.select(handle));
  }
  async expandProperty(row: PropertyRow): Promise<void> {
    const request = this.propertyTree.toggle(row);
    this.debuggerProperties = this.propertyTree.rows();
    if (request) {
      await this.readProperties(request);
    }
  }
  private async readProperties(branch: {
    key: string;
    handle: number;
    offset: number;
  }): Promise<void> {
    const pause = this.pause;
    const frame = this.debuggerFrame;
    const request = await this.action({
      kind: "properties",
      pause,
      handle: branch.handle,
      offset: branch.offset,
    });
    if (request !== undefined && pause === this.pause && frame === this.debuggerFrame) {
      this.propertyBranches.set(request, branch.key);
    }
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
    this.debuggerConsole = [];
    this.requests.clear();
    this.diagnostics?.clearLogs();
  }
  private trimConsole(): void {
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
    for (const [key, id] of this.requests) {
      if (!retained.has(id)) {
        this.requests.delete(key);
      }
    }
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
  async stop(): Promise<void> {
    if (!this.debuggerClosed) {
      await this.action({ kind: "stop" });
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
  ): Promise<number | undefined> {
    if (!session) {
      return;
    }
    try {
      const request = await invoke<number>("workspace_debug_command", {
        session,
        action,
      });
      this.debuggerError = "";
      return request;
    } catch (error) {
      this.debuggerError = describeError(error);
      return;
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
    try {
      const changed = points.filter(
        (point) =>
          !this.points.some((previous) => JSON.stringify(previous) === JSON.stringify(point)),
      );
      const removed = this.points.filter(
        (previous) => !points.some((point) => point.breakpoint.id === previous.breakpoint.id),
      );
      const saved = await invoke<Point[]>("workspace_breakpoints", { points: changed, removed });
      this.debuggerError = "";
      this.points = saved;
      this.projectionKey = "";
      await this.loadPositions();
      this.renderPoints();
      await this.refresh();
      return true;
    } catch (error) {
      this.debuggerError = describeError(error);
      return false;
    }
  }
  async removePoint(point: Point): Promise<void> {
    await this.savePoints(this.points.filter((p) => p.breakpoint.id !== point.breakpoint.id));
  }
  async disable(line: number): Promise<void> {
    const point = this.point(line);
    if (point) {
      await this.savePoints(
        this.points.map((p) => (p === point ? { ...p, enabled: !p.enabled } : p)),
      );
    }
  }
  condition(line: number): void {
    if (!this.selected || !this.model || this.selected.state === "Retired") {
      return;
    }
    this.debuggerError = "";
    const point = this.point(line);
    this.conditionContext = {
      file: this.selected,
      line,
      anchor: this.model.getLineContent(line),
      point,
    };
    this.debuggerConditionSource = `${this.selected.package}/${this.selected.path}:${line}`;
    this.debuggerCondition = point?.breakpoint.condition ?? "";
    this.conditionDialog.showModal();
  }
  conditionInput(event: Event): void {
    this.debuggerCondition = (event.target as HTMLInputElement).value;
  }
  async saveCondition(event: Event): Promise<void> {
    event.preventDefault();
    const context = this.conditionContext;
    if (!context) {
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
      breakpoint: { ...point.breakpoint, condition: this.debuggerCondition.trim() || undefined },
    };
    if (!(await this.savePoints([...this.points.filter((p) => p !== previous), changed]))) {
      return;
    }
    this.closeCondition();
  }
  closeCondition(): void {
    delete this.conditionContext;
    this.conditionDialog.close();
  }
  consoleInput(event: Event): void {
    this.debuggerInput = (event.target as HTMLTextAreaElement).value;
  }
  async consoleKey(event: KeyboardEvent): Promise<void> {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      await this.evaluate();
    }
  }
  async evaluate(): Promise<void> {
    const draft = this.debuggerInput;
    const input = draft.trim();
    if (!input || !this.debuggerTarget || this.debuggerClosed || this.debuggerSubmitting) {
      return;
    }
    const session = this.debuggerTarget;
    const generation = this.generation;
    const id = ++this.serial;
    this.debuggerSubmitting = true;
    this.debuggerConsole = [
      ...this.debuggerConsole.slice(-99),
      { id, input, output: "Running…", thrown: false },
    ];
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
      if (this.debuggerInput === draft) {
        this.debuggerInput = "";
      }
    } finally {
      this.evaluationSubmission = null;
      this.debuggerSubmitting = false;
    }
  }
  async place(placement: string): Promise<void> {
    try {
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
        groups: [...this.sourceGroups],
        split: this.debuggerConsoleSplit,
        file: this.selected?.id,
        artifact: this.selected?.artifact,
        target: this.debuggerTarget,
        focus: this.lastFocus,
        requests: [...this.requests],
        frame: this.debuggerFrame,
        input: this.debuggerInput,
        console: this.debuggerConsole,
        states: [...this.states].slice(-64),
        tabs: this.debuggerTabs.map((tab) => tab.key),
        history: this.history,
        historyIndex: this.historyIndex,
        diagnostics: this.diagnostics?.saveView(),
        scroll: this.consoleOutput.scrollTop,
      };
      await invoke("place_debugger", { placement, viewState });
    } catch (error) {
      this.debuggerError = describeError(error);
    }
  }
  keyboard(event: KeyboardEvent): void {
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
        : event.key === "F10"
          ? "over"
          : event.key === "F11"
            ? event.shiftKey
              ? "out"
              : "into"
            : null;
    if (action) {
      event.preventDefault();
      event.stopPropagation();
      void this.resume(action);
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
    if (!this.editor || !this.monaco || !this.selected) {
      return;
    }
    const bindings = [...this.views.values()]
      .filter((view) => !view.stream.closed && view.info.artifact === this.selected?.artifact)
      .flatMap((view) => view.stream.inventory?.breakpoints ?? []);
    const decorations: Monaco.editor.IModelDeltaDecoration[] = [];
    for (const point of this.points) {
      const bound = bindings.find((binding) => binding.id === point.breakpoint.id)?.resolved;
      const position = this.positions.get(point.breakpoint.id);
      if (!position) {
        continue;
      }
      decorations.push({
        range: new this.monaco.Range(position.line, 1, position.line, 1),
        options: {
          glyphMarginClassName: !point.enabled
            ? "debug-breakpoint-disabled"
            : bound
              ? "debug-breakpoint"
              : "debug-breakpoint-pending",
          glyphMarginHoverMessage: {
            value: point.breakpoint.condition
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
    }
    this.decorations?.set(decorations);
  }
  disconnectedCallback(): void {
    this.resizeObserver?.disconnect();
    window.clearTimeout(this.navigationTimer);
    this.navigationGeneration++;
    document.removeEventListener("keydown", this.toggleShortcut, true);
    this.removeEventListener("keydown", this.shortcutListener, true);
    this.focusUnlisten?.();
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
