import { WebUIElement, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import "../script-diagnostics/script-diagnostics.js";
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
  expandable: boolean;
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
  @observable nextProperties = false;
  @observable canBack = false;
  @observable valuesLabel = "Select a lexical scope to inspect its own values.";
  @observable frameLimit = false;
  @observable breakpointCount = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private generation = 0;
  private sourceGeneration = 0;
  private cursor = 0;
  private sourcePage: SourcePage | null = null;
  private statement: SourceLocation | null = null;
  private selectedFrame = 0;
  private selectedHandle = 0;
  private nextOffset: number | null = null;
  private parents: number[] = [];
  private exception: ExceptionDiagnostic | null = null;
  private breakpoints: SourceBreakpoint[] = [];
  private bound = new Map<number, BreakpointResolution>();
  private outcomeKey = "";
  private ended = false;
  connectedCallback(): void {
    super.connectedCallback();
    this.start();
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
      this.sourceName = this.sessionInfo.source;
      void this.loadSource(this.sourceName, 1, 1);
      void this.poll(this.generation);
    }
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
        this.issue = "";
        this.status = `Paused · ${event.reason} · operation ${this.invocation}`;
        this.frames = event.frames.map((frame) => ({
          ...frame,
          label: `${frame.function || "(anonymous)"} · ${position(frame.original ?? frame.location)}`,
          selected: frame.index === 0,
        }));
        this.frameLimit = event.truncated;
        this.exception = event.exception;
        void this.selectFrame(0);
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
        }
        break;
      case "properties":
        if (event.pause === this.pause && this.paused && event.handle === this.selectedHandle) {
          this.properties = event.properties.map((property) => ({
            ...property,
            expandable: property.value.handle !== null,
          }));
          this.nextOffset = event.next;
          this.nextProperties = event.next !== null;
          this.valuesLabel = `Own properties ${event.offset + 1}–${event.offset + event.properties.length} · getters and proxies are not evaluated`;
        }
        break;
      case "rejected":
        this.issue = event.message;
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
    this.valuesLabel = "Select a lexical scope to inspect its own values.";
    this.scopes = [];
    this.properties = [];
    this.selectedHandle = 0;
    this.parents = [];
    this.nextOffset = null;
    this.nextProperties = false;
    this.canBack = false;
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
  async close(): Promise<void> {
    if (!this.sessionInfo) {
      return;
    }
    const id = this.sessionInfo.session;
    ++this.generation;
    ++this.sourceGeneration;
    this.stopPolling();
    try {
      await invoke("close_debug_session", { session: id });
      this.dispatchEvent(
        new CustomEvent("debug-closed", {
          detail: { detached: this.detached },
          bubbles: true,
          composed: true,
        }),
      );
    } catch (error: unknown) {
      this.issue = describeError(error);
      void this.poll(this.generation);
    }
  }
  async selectFrame(index: number): Promise<void> {
    if (!this.paused) {
      return;
    }
    this.selectedFrame = index;
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
      this.sourceStatus =
        "Source unavailable for this frame. Internal invocation sources are excluded.";
    }
    await this.action({ kind: "scopes", pause: this.pause, frame: index });
  }
  async selectScope(scope: ScopeView): Promise<void> {
    if (scope.handle === null) {
      return;
    }
    this.parents = [];
    this.canBack = false;
    this.scopes = this.scopes.map((value) => ({
      ...value,
      selected: value.handle === scope.handle,
    }));
    await this.readProperties(scope.handle, 0);
  }
  private async readProperties(handle: number, offset: number): Promise<void> {
    this.selectedHandle = handle;
    this.properties = [];
    this.nextProperties = false;
    await this.action({ kind: "properties", pause: this.pause, handle, offset });
  }
  async expand(value: DebugProperty): Promise<void> {
    if (value.value.handle === null || this.parents.length >= 16) {
      return;
    }
    this.parents.push(this.selectedHandle);
    this.canBack = true;
    await this.readProperties(value.value.handle, 0);
  }
  async back(): Promise<void> {
    const handle = this.parents.pop();
    this.canBack = this.parents.length > 0;
    if (handle !== undefined) {
      await this.readProperties(handle, 0);
    }
  }
  async moreProperties(): Promise<void> {
    if (this.nextOffset !== null) {
      await this.readProperties(this.selectedHandle, this.nextOffset);
    }
  }
  async policy(event: Event): Promise<void> {
    const value = (event.target as HTMLSelectElement).value;
    if (value !== "none" && value !== "uncaught" && value !== "all") {
      return;
    }
    this.exceptions = value;
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
    if (
      event
        .composedPath()
        .some(
          (target) =>
            target instanceof HTMLElement &&
            (target.matches("input,select,textarea") || target.isContentEditable),
        )
    ) {
      return;
    }
    if (event.key === "F5") {
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
