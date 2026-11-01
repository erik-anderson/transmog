import { WebUIElement, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import "../script-diagnostics/script-diagnostics.js";
import "../pane-divider/pane-divider.js";
import type {
  DeveloperEvidence,
  ExceptionDiagnostic,
  SourceLocation,
  SourcePage,
} from "../script-diagnostics/models.js";
import { describeError } from "../utilities.js";
import {
  evidence,
  type DebugAction,
  type DebugEvent,
  type DebugFrame,
  type DebugProperty,
  type DebugScope,
  type DebugSessionInfo,
  type DebugSessionView,
  type SourceBreakpoint,
  type BreakpointResolution,
  type DebugConfiguration,
} from "./models.js";
interface FrameView extends DebugFrame {
  label: string;
  selected: boolean;
}
interface ScopeView extends DebugScope {
  available: boolean;
  label: string;
  selected: boolean;
}
interface PropertyView extends DebugProperty {
  key: string;
  depth: number;
  expandable: boolean;
  expanded: boolean;
  more: boolean;
  pageOffset?: number;
  branch: string;
}
interface ValueBranch {
  handle: number;
  path: string[];
  depth: number;
  properties: DebugProperty[];
  next: number | null;
  offset: number;
  pages: number[];
}
interface SourceLine {
  number: number;
  text: string;
  shortened: boolean;
  className: string;
  breakpoint: boolean;
  bound: boolean;
  pending: boolean;
  breakpointStatus: string;
}
const position = (location: SourceLocation | null | undefined): string =>
  location ? `${location.source}:${location.line}:${location.column}` : "Source unavailable";

/** One explicitly owned local controller. Templates render all authored text inertly. */
export class ScriptDebugger extends WebUIElement {
  @observable sessionInfo: DebugSessionInfo | null = null;
  @observable outdated = false;
  @observable status = "Attaching debugger…";
  @observable issue = "";
  @observable paused = false;
  @observable closed = false;
  @observable closeLabel = "Stop and close debugger";
  closedChanged(): void {
    this.closeLabel = this.closed ? "Close debugger" : "Stop and close debugger";
  }
  @observable attached = false;
  @observable idle = false;
  @observable detached = false;
  @observable pause = 0;
  @observable invocation = 0;
  @observable lease = "";
  @observable frames: FrameView[] = [];
  @observable scopes: ScopeView[] = [];
  @observable properties: PropertyView[] = [];
  @observable sourceLines: SourceLine[] = [];
  @observable sourceName = "";
  @observable sourceStatus = "";
  @observable sources: Array<{ source: string }> = [];
  @observable resolutions: Array<{ id: number; label: string; bound: boolean }> = [];
  @observable details: DeveloperEvidence[] = [];
  @observable result = "";
  @observable exceptions = "none";
  @observable requested: SourceBreakpoint[] = [];
  requestedChanged(): void {
    this.configure(this.requested, this.exceptions);
  }
  @observable previousSource = false;
  @observable nextSource = false;
  @observable valuesLabel = "Select a lexical scope to inspect its own values.";
  @observable frameLimit = false;
  @observable breakpointCount = 0;
  @observable restartable = false;
  @observable restartLabel = "Restart with current draft and fixture";
  @observable busy = false;
  @observable detaching = false;
  @observable narrowTab = "source";
  @observable sourceSplit = "60";
  @observable sourceShare = "60%";
  surface!: HTMLDialogElement;
  sourceViewport!: HTMLDivElement;
  private returnFocus: HTMLElement | null = null;
  private readonly branches = new Map<string, ValueBranch>();
  private readonly pendingProperties = new Map<string, Set<string>>();
  private queuedProperties: string[] = [];
  private readingProperties: string | null = null;
  private readonly expandedPaths = new Set<string>();
  private preferredScope = "";
  private scopePath: string[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;
  private generation = 0;
  private sourceGeneration = 0;
  private cursor = 0;
  private sourcePage: SourcePage | null = null;
  private statement: SourceLocation | null = null;
  private selectedFrame = 0;
  private exception: ExceptionDiagnostic | null = null;
  private breakpoints: SourceBreakpoint[] = [];
  private bound = new Map<number, BreakpointResolution>();
  private outcomeKey = "";
  private ended = false;
  private readonly sourceResize = new ResizeObserver(() => {
    if (!this.surface?.open || !this.paused || !this.sourceViewport.clientHeight) {
      return;
    }
    const line = this.sourceViewport.querySelector<HTMLElement>(".current");
    if (!line) {
      return;
    }
    const viewport = this.sourceViewport.getBoundingClientRect();
    const current = line.getBoundingClientRect();
    const top = viewport.top + this.sourceViewport.clientTop;
    if (current.top < top || current.bottom > top + this.sourceViewport.clientHeight) {
      this.revealStatement();
    }
  });
  connectedCallback(): void {
    super.connectedCallback();
    this.start();
    this.sourceResize.observe(this.sourceViewport);
  }
  sessionInfoChanged(): void {
    this.start();
  }
  private start(): void {
    this.attached = false;
    this.outcomeKey = "";
    this.idle = false;
    this.detached = false;
    this.ended = false;
    this.detaching = false;
    this.busy = false;
    ++this.generation;
    this.stopPolling();
    this.cursor = 0;
    this.closed = false;
    this.paused = false;
    this.pause = 0;
    this.frames = [];
    this.scopes = [];
    this.properties = [];
    this.sources = [];
    this.details = [];
    this.result = "";
    this.issue = "";
    this.bound.clear();
    this.resolutions = [];
    this.statement = null;
    this.exception = null;
    this.status = "Attaching debugger…";
    if (this.sessionInfo) {
      this.restartable = this.sessionInfo.mode !== "Live exchange";
      this.restartLabel = this.sessionInfo.mode.startsWith("Sandbox")
        ? "Restart with current draft and fixture"
        : "Restart fresh fixture";
      this.sourceName = this.sessionInfo.source;
      void this.loadSource(this.sourceName, 1, 1);
      void this.poll(this.generation);
      queueMicrotask(() => this.show());
    }
  }
  show(): void {
    if (!this.isConnected || !this.surface || this.surface.open) {
      return;
    }
    this.returnFocus =
      this.getRootNode() instanceof ShadowRoot
        ? ((this.getRootNode() as ShadowRoot).activeElement as HTMLElement | null)
        : (document.activeElement as HTMLElement | null);
    this.surface.showModal();
    this.surface.focus();
    this.revealStatement();
  }
  hide(event?: Event): void {
    event?.preventDefault();
    this.surface.close();
    this.returnFocus?.focus();
    this.$emit("debug-hidden");
  }
  private revealStatement(): void {
    this.$flushUpdates();
    const line = this.sourceViewport?.querySelector<HTMLElement>(".current");
    if (line) {
      this.sourceViewport.scrollTop +=
        line.getBoundingClientRect().top -
        this.sourceViewport.getBoundingClientRect().top -
        this.sourceViewport.clientHeight / 2;
    }
  }
  selectTab(tab: string): void {
    this.narrowTab = tab;
    if (tab === "source") {
      this.revealStatement();
    }
  }
  private configurationChanged(): void {
    this.dispatchEvent(
      new CustomEvent<DebugConfiguration>("debug-configuration", {
        detail: {
          exceptions: this.exceptions,
          breakpoints: this.breakpoints.map((value) => ({
            ...value,
            location: { ...value.location },
          })),
        },
        bubbles: true,
        composed: true,
      }),
    );
  }
  resizeSource(event: CustomEvent<{ value: number }>): void {
    const value = Math.max(15, Math.min(85, event.detail.value));
    this.sourceSplit = String(value);
    this.sourceShare = `${value}%`;
    this.revealStatement();
  }
  /** Sets requested source positions before initialization; native resolution remains authoritative. */
  configure(values: SourceBreakpoint[], exceptions = "none"): void {
    this.breakpoints = values.map((value) => ({ id: value.id, location: { ...value.location } }));
    this.breakpointCount = values.length;
    this.exceptions = exceptions;
    this.redrawSource();
  }
  private async poll(generation: number): Promise<void> {
    if (!this.sessionInfo || !this.isConnected || generation !== this.generation) {
      return;
    }
    try {
      const view = await invoke<DebugSessionView>("debug_session", {
        session: this.sessionInfo.session,
        after: this.cursor,
      });
      if (!this.isConnected || generation !== this.generation) {
        return;
      }
      this.cursor = view.stream.cursor;
      if (view.stream.inventory) {
        this.accept({ kind: "sources", ...view.stream.inventory });
      }
      if (view.stream.omitted) {
        this.issue =
          "Earlier debugger events expired from the bounded history. Select a frame or source again.";
        this.invalidateValues();
      }
      for (const item of view.stream.events) {
        if (item.session === this.sessionInfo.session) {
          this.invocation = Math.max(this.invocation, item.invocation);
          this.accept(item.event);
        }
      }
      this.lease = `Session: ${Math.ceil(view.stream.sessionRemainingMs / 1000)}s remaining${view.stream.pauseRemainingMs === null ? "" : ` · Pause: ${Math.ceil(view.stream.pauseRemainingMs / 1000)}s remaining`}`;
      if (view.stream.armRemainingMs != null) {
        this.lease += ` · Arming: ${Math.ceil(view.stream.armRemainingMs / 1000)}s remaining`;
        if (!this.paused) {
          this.status = "Armed · waiting for the next matching exchange";
        }
      }
      if (view.stream.exchangeId) {
        this.lease += ` · Exchange ${view.stream.exchangeId}`;
      }
      this.idle = view.stream.active === false;
      if (this.detaching && this.idle && this.attached && !view.stream.closed) {
        this.detaching = false;
        await this.detach();
      }
      this.details = evidence(view, this.exception);
      if (view.outcome) {
        const key = `${view.outcome.status}:${view.outcome.summary}`;
        if (key !== this.outcomeKey) {
          this.outcomeKey = key;
          this.status = view.outcome.summary;
          this.result =
            view.outcome.result === null ? "" : JSON.stringify(view.outcome.result, null, 2);
          if (view.outcome.truncated) {
            this.result =
              "Result exceeded the 64 KiB debug view limit. Run an ordinary test for full evidence.";
          }
        }
      }
      this.closed = view.stream.closed;
      if (this.closed) {
        this.lease = "Session ended";
        this.paused = false;
        this.invalidateValues();
        if (!this.ended) {
          this.ended = true;
          this.dispatchEvent(
            new CustomEvent("debug-ended", {
              detail: { detached: this.detached },
              bubbles: true,
              composed: true,
            }),
          );
        }
      }
      if (this.closed && view.outcome) {
        return;
      }
    } catch (error: unknown) {
      if (generation === this.generation) {
        this.issue = describeError(error);
      }
    }
    // Keep polling a closed stream until its background completion result arrives.
    if (this.isConnected && generation === this.generation) {
      this.timer = setTimeout(
        () => {
          void this.poll(generation);
        },
        this.closed ? 500 : 150,
      );
    }
  }
  private accept(event: DebugEvent): void {
    switch (event.kind) {
      case "attached":
        this.attached = true;
        this.status = "Running · debugger attached";
        break;
      case "detached":
        this.detached = true;
        this.status = "Debugger detached";
        this.closed = true;
        break;
      case "sources": {
        const sources = new Map(this.sources.map((source) => [source.source, source]));
        for (const source of event.sources) {
          sources.set(source.source, { source: source.source });
        }
        this.sources = [...sources.values()]
          .filter((source) => source.source !== this.sessionInfo?.source)
          .slice(0, 128);
        for (const resolution of event.breakpoints) {
          this.bound.set(resolution.id, resolution);
        }
        this.resolutions = [...this.bound.values()].map((value) => ({
          id: value.id,
          label: value.resolved
            ? `Breakpoint ${value.id}: ${position(value.resolved)}`
            : `Breakpoint ${value.id}: ${value.issue ?? "pending"}`,
          bound: value.resolved !== null,
        }));
        this.redrawSource();
        break;
      }
      case "paused":
        this.invalidateValues();
        this.pause = event.pause;
        this.paused = true;
        if (this.detaching) {
          // Authored `debugger` statements can still pause after source breakpoints
          // are cleared. Finish the operation before the native idle detach barrier.
          void this.resume("continue");
          break;
        }
        this.issue = "";
        this.status = `Paused · ${event.reason} · operation ${this.invocation}`;
        this.frames = event.frames.map((frame) => ({
          ...frame,
          label: `${frame.function || "(anonymous)"} · ${position(frame.original ?? frame.location)}`,
          selected: frame.index === 0,
        }));
        this.frameLimit = event.truncated;
        this.exception = event.exception;
        void this.selectFrame(0, false);
        break;
      case "resumed":
        this.paused = false;
        this.status = "Running";
        this.invalidateValues();
        this.statement = null;
        this.redrawSource();
        break;
      case "scopes":
        if (event.pause === this.pause && this.paused && event.frame === this.selectedFrame) {
          this.scopes = event.scopes.map((scope) => ({
            ...scope,
            available: scope.handle !== null,
            label: `${scope.kind}${scope.name ? ` · ${scope.name}` : ""}${scope.handle === null ? " · unavailable or handle limit" : ""}`,
            selected: false,
          }));
          const scope =
            this.scopes.find(
              (value) => value.available && `${value.kind}:${value.name}` === this.preferredScope,
            ) ??
            this.scopes.find((value) => value.available && value.kind === "local") ??
            this.scopes.find((value) => value.available);
          if (scope) {
            void this.selectScope(scope, false);
          }
        }
        break;
      case "properties":
        if (event.pause === this.pause && this.paused) {
          const request = `${event.handle}:${event.offset}`;
          const keys = this.pendingProperties.get(request);
          this.pendingProperties.delete(request);
          if (this.readingProperties === request) {
            this.readingProperties = null;
          }
          for (const key of keys ?? []) {
            const branch = this.branches.get(key);
            if (!branch || branch.handle !== event.handle || branch.offset !== event.offset) {
              continue;
            }
            branch.properties = event.properties;
            branch.next = event.next;
            for (const property of event.properties) {
              const path = [...branch.path, property.name];
              if (property.value.handle !== null && this.expandedPaths.has(JSON.stringify(path))) {
                void this.openBranch(property.value.handle, path, branch.depth + 1);
              }
            }
          }
          this.redrawProperties();
          void this.readNextProperties();
        }
        break;
      case "rejected":
        this.issue = event.message;
        if (this.readingProperties) {
          this.pendingProperties.delete(this.readingProperties);
          this.readingProperties = null;
          void this.readNextProperties();
        }
        break;
      case "stopped":
        this.status = event.reason;
        this.closed = true;
        this.paused = false;
        this.invalidateValues();
        this.statement = null;
        this.redrawSource();
        break;
      case "acknowledged":
        break;
    }
  }
  private invalidateValues(): void {
    this.valuesLabel = "Variables appear automatically when execution pauses.";
    this.scopes = [];
    this.properties = [];
    this.branches.clear();
    this.pendingProperties.clear();
    this.queuedProperties = [];
    this.readingProperties = null;
  }
  async action(action: DebugAction): Promise<boolean> {
    if (!this.sessionInfo || this.closed) {
      return false;
    }
    const generation = this.generation;
    try {
      await invoke<number>("debug_command", { session: this.sessionInfo.session, action });
      return true;
    } catch (error: unknown) {
      if (generation === this.generation) {
        this.issue = describeError(error);
      }
      return false;
    }
  }
  async resume(action: "continue" | "into" | "over" | "out"): Promise<void> {
    if (!this.paused) {
      return;
    }
    const pause = this.pause;
    this.paused = false;
    this.invalidateValues();
    if (
      !(await this.action({ kind: "resume", pause, action })) &&
      pause === this.pause &&
      !this.closed
    ) {
      this.paused = true;
      await this.selectFrame(this.selectedFrame);
    }
  }
  async stop(): Promise<void> {
    await this.action({ kind: "stop" });
  }
  async detach(): Promise<void> {
    this.idle = false;
    await this.action({ kind: "detach" });
  }
  async continueAndDetach(): Promise<void> {
    if (this.closed || !this.attached || this.detaching) {
      return;
    }
    if (this.idle) {
      await this.detach();
      return;
    }
    this.busy = true;
    // Suppress further pauses for this operation without changing the next-run configuration.
    const cleared =
      (await this.action({ kind: "breakpoints", values: [] })) &&
      (await this.action({ kind: "exceptions", policy: "none" }));
    if (cleared) {
      this.detaching = true;
      await this.resume("continue");
    } else if (!this.closed) {
      await this.action({ kind: "breakpoints", values: this.breakpoints });
      if (
        this.exceptions === "none" ||
        this.exceptions === "all" ||
        this.exceptions === "uncaught"
      ) {
        await this.action({ kind: "exceptions", policy: this.exceptions });
      }
    }
    this.busy = false;
  }
  async restart(): Promise<void> {
    if (!this.restartable || this.busy) {
      return;
    }
    this.busy = true;
    const closed = await this.closeSession();
    this.busy = false;
    if (closed) {
      this.surface.close();
      this.$emit("debug-restart");
    }
  }
  async close(): Promise<void> {
    if (await this.closeSession()) {
      this.surface.close();
      this.$emit("debug-closed", { detached: this.detached });
      this.returnFocus?.focus();
    }
  }
  private async closeSession(): Promise<boolean> {
    if (!this.sessionInfo) {
      return false;
    }
    const id = this.sessionInfo.session;
    ++this.generation;
    ++this.sourceGeneration;
    this.stopPolling();
    try {
      await invoke("close_debug_session", { session: id });
      return true;
    } catch (error: unknown) {
      this.issue = describeError(error);
      void this.poll(this.generation);
      return false;
    }
  }
  async selectFrame(index: number, reveal = true): Promise<void> {
    if (!this.paused) {
      return;
    }
    this.selectedFrame = index;
    if (reveal) {
      this.narrowTab = "source";
    }
    this.invalidateValues();
    this.frames = this.frames.map((frame) => ({ ...frame, selected: frame.index === index }));
    const frame = this.frames.find((frame) => frame.index === index);
    const location = frame?.original ?? frame?.location;
    this.statement = location ?? null;
    if (location) {
      void this.loadSource(location.source, location.line, location.column);
    } else {
      this.sourcePage = null;
      this.sourceLines = [];
      this.previousSource = false;
      this.nextSource = false;
      this.sourceStatus =
        "Source unavailable for this frame. Internal invocation sources are excluded.";
    }
    await this.action({ kind: "scopes", pause: this.pause, frame: index });
  }
  async selectScope(scope: ScopeView, remember = true): Promise<void> {
    if (scope.handle === null) {
      return;
    }
    this.branches.clear();
    this.pendingProperties.clear();
    this.queuedProperties = [];
    this.readingProperties = null;
    const identity = `${scope.kind}:${scope.name}`;
    if (remember) {
      this.preferredScope = identity;
    }
    this.scopePath = [identity];
    this.scopes = this.scopes.map((value) => ({
      ...value,
      selected: value.handle === scope.handle,
    }));
    this.valuesLabel = scope.label;
    await this.openBranch(scope.handle, this.scopePath, 0);
  }
  private async openBranch(handle: number, path: string[], depth: number): Promise<void> {
    const key = JSON.stringify(path);
    if (depth > 16 || this.branches.size >= 64 || this.branches.has(key)) {
      return;
    }
    this.branches.set(key, {
      handle,
      path,
      depth,
      properties: [],
      next: null,
      offset: 0,
      pages: [0],
    });
    await this.readBranch(key, 0);
  }
  private async readBranch(key: string, offset: number): Promise<void> {
    const branch = this.branches.get(key);
    if (!branch) {
      return;
    }
    branch.offset = offset;
    if (!branch.pages.includes(offset)) {
      branch.pages.push(offset);
      branch.pages.sort((left, right) => left - right);
    }
    const request = `${branch.handle}:${offset}`;
    const pending = this.pendingProperties.get(request);
    if (pending) {
      pending.add(key);
      return;
    }
    this.pendingProperties.set(request, new Set([key]));
    this.queuedProperties.push(request);
    await this.readNextProperties();
  }
  private async readNextProperties(): Promise<void> {
    if (this.readingProperties || !this.paused) {
      return;
    }
    const request = this.queuedProperties.shift();
    if (!request) {
      return;
    }
    const key = this.pendingProperties.get(request)?.values().next().value;
    const branch = key ? this.branches.get(key) : null;
    if (!branch) {
      this.pendingProperties.delete(request);
      await this.readNextProperties();
      return;
    }
    const pause = this.pause;
    this.readingProperties = request;
    if (
      !(await this.action({
        kind: "properties",
        pause,
        handle: branch.handle,
        offset: branch.offset,
      })) &&
      pause === this.pause
    ) {
      this.pendingProperties.delete(request);
      this.readingProperties = null;
      await this.readNextProperties();
    }
  }
  async expand(value: PropertyView): Promise<void> {
    if (value.more) {
      const branch = this.branches.get(value.branch);
      if (branch && value.pageOffset !== undefined) {
        await this.readBranch(value.branch, value.pageOffset);
      }
    } else if (value.value.handle !== null) {
      if (this.expandedPaths.has(value.key)) {
        this.expandedPaths.delete(value.key);
      } else {
        if (this.expandedPaths.size >= 256) {
          this.issue = "Collapse an expanded value before opening another.";
          return;
        }
        this.expandedPaths.add(value.key);
        const path = JSON.parse(value.key) as string[];
        await this.openBranch(value.value.handle, path, value.depth + 1);
      }
    }
    this.redrawProperties();
  }
  private redrawProperties(): void {
    const rows: PropertyView[] = [];
    const visit = (key: string): void => {
      const branch = this.branches.get(key);
      if (!branch) {
        return;
      }
      if (branch.offset > 0) {
        rows.push({
          name: "Previous properties",
          value: { kind: "", display: "", handle: null, truncated: false },
          key: `${key}:previous`,
          depth: branch.depth,
          expandable: false,
          expanded: false,
          more: true,
          branch: key,
          pageOffset: branch.pages.filter((offset) => offset < branch.offset).at(-1) ?? 0,
        });
      }
      for (const property of branch.properties) {
        const path = JSON.stringify([...branch.path, property.name]);
        const expanded = property.value.handle !== null && this.expandedPaths.has(path);
        rows.push({
          ...property,
          key: path,
          depth: branch.depth,
          expandable: property.value.handle !== null && branch.depth < 16,
          expanded,
          more: false,
          branch: key,
        });
        if (expanded) {
          visit(path);
        }
      }
      if (branch.next !== null) {
        rows.push({
          name: "Next properties",
          value: { kind: "", display: "", handle: null, truncated: false },
          key: `${key}:more`,
          depth: branch.depth,
          expandable: false,
          expanded: false,
          more: true,
          branch: key,
          pageOffset: branch.next,
        });
      }
    };
    visit(JSON.stringify(this.scopePath));
    this.properties = rows;
  }
  async policy(event: Event): Promise<void> {
    const value = (event.target as HTMLSelectElement).value;
    if (value !== "none" && value !== "uncaught" && value !== "all") {
      return;
    }
    this.exceptions = value;
    this.configurationChanged();
    await this.action({ kind: "exceptions", policy: value });
  }
  async changeSource(event: Event): Promise<void> {
    await this.loadSource((event.target as HTMLSelectElement).value, 1, 1);
  }
  async loadSource(source: string, line: number, column: number): Promise<void> {
    if (!this.sessionInfo) {
      return;
    }
    const version = ++this.sourceGeneration;
    this.sourceStatus = "Loading exact source snapshot…";
    try {
      const page = await invoke<SourcePage>("script_source", {
        artifact: this.sessionInfo.artifact,
        source,
        line,
        column,
      });
      if (!this.isConnected || version !== this.sourceGeneration) {
        return;
      }
      this.sourcePage = page;
      this.sourceName = source;
      this.previousSource = page.lines[0]!.number > 1;
      this.nextSource = page.lines.at(-1)!.number < page.totalLines;
      this.sourceStatus = `Read-only executing snapshot · ${source}:${line}:${column}`;
      this.redrawSource();
      this.sourceViewport.scrollTop = 0;
      this.revealStatement();
    } catch (error: unknown) {
      if (version === this.sourceGeneration) {
        this.sourcePage = null;
        this.sourceLines = [];
        this.sourceStatus = describeError(error);
      }
    }
  }
  async sourcePrevious(): Promise<void> {
    if (this.sourcePage) {
      await this.loadSource(this.sourceName, Math.max(1, this.sourcePage.line - 40), 1);
    }
  }
  async sourceNext(): Promise<void> {
    if (this.sourcePage) {
      await this.loadSource(
        this.sourceName,
        Math.min(this.sourcePage.totalLines, this.sourcePage.line + 40),
        1,
      );
    }
  }
  async toggleBreakpoint(line: number): Promise<void> {
    const existing = this.breakpoints.find(
      (value) => value.location.source === this.sourceName && value.location.line === line,
    );
    if (existing) {
      this.breakpoints = this.breakpoints.filter((value) => value !== existing);
    } else if (this.breakpoints.length < 128) {
      this.breakpoints.push({
        id: Math.max(0, ...this.breakpoints.map((value) => value.id)) + 1,
        location: { source: this.sourceName, line, column: 1 },
      });
    }
    this.breakpointCount = this.breakpoints.length;
    this.bound.clear();
    this.resolutions = [];
    this.redrawSource();
    this.configurationChanged();
    await this.action({ kind: "breakpoints", values: this.breakpoints });
  }
  private redrawSource(): void {
    this.sourceLines = (this.sourcePage?.lines ?? []).map((line) => {
      const breakpoint = this.breakpoints.find(
        (value) => value.location.source === this.sourceName && value.location.line === line.number,
      );
      const resolution = breakpoint ? this.bound.get(breakpoint.id) : undefined;
      return {
        ...line,
        className:
          this.paused &&
          this.statement?.source === this.sourceName &&
          this.statement.line === line.number
            ? "code-line current"
            : "code-line",
        breakpoint: Boolean(breakpoint),
        bound: Boolean(resolution?.resolved),
        pending: Boolean(breakpoint && !resolution?.resolved),
        breakpointStatus: breakpoint
          ? resolution?.resolved
            ? `Bound at ${position(resolution.resolved)}`
            : (resolution?.issue ?? "Pending resolution")
          : "No breakpoint requested",
      };
    });
  }
  keyboard(event: KeyboardEvent): void {
    event.stopPropagation();
    if (event.isComposing || event.repeat || event.altKey || event.metaKey) {
      return;
    }
    if (event.key === "F8" || (event.ctrlKey && event.key === "\\")) {
      event.preventDefault();
      void this.resume("continue");
    }
    if (event.key === "F5" && !event.ctrlKey) {
      event.preventDefault();
      if (event.shiftKey) {
        void this.stop();
      } else {
        void this.resume("continue");
      }
    }
    if (event.key === "F10") {
      event.preventDefault();
      void this.resume("over");
    }
    if (event.key === "F11") {
      event.preventDefault();
      void this.resume(event.shiftKey ? "out" : "into");
    }
  }
  private stopPolling(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
  disconnectedCallback(): void {
    this.sourceResize.disconnect();
    ++this.generation;
    ++this.sourceGeneration;
    this.stopPolling();
    if (this.sessionInfo) {
      void invoke("close_debug_session", { session: this.sessionInfo.session }).catch(() => {});
    }
    super.disconnectedCallback();
  }
}
ScriptDebugger.define("script-debugger");
