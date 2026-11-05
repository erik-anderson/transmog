import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { describeError } from "../utilities.js";
import type {
  DeveloperEvidence,
  ExceptionValue,
  LogRun,
  SourceLocation,
  SourcePage,
  StackFrame,
} from "./models.js";

interface FrameView {
  function: string;
  label: string;
  generatedLabel: string;
  mapping: string;
  artifact: string;
  location: SourceLocation | null;
  generated: SourceLocation | null;
}
interface ExceptionView {
  label: string;
  title: string;
  unavailable: boolean;
  truncated: boolean;
  frames: FrameView[];
}
interface LogView {
  key: string;
  context: string;
  level: string;
  time: string;
  text: string;
  truncated: boolean;
  frames: FrameView[];
}
const locationText = (location: SourceLocation | null | undefined): string =>
  location ? `${location.source}:${location.line}:${location.column}` : "Location unavailable";
const runKey = (run: LogRun): string =>
  JSON.stringify([run.context.worker, run.context.revision, run.context.invocation]);
const timestamp = (milliseconds: number): string => {
  const value = new Date(milliseconds);
  return Number.isNaN(value.getTime()) ? "Timestamp unavailable" : value.toISOString();
};
const mappingText = {
  missing: "Declared source map is missing",
  invalid: "Source map is invalid or unsupported",
  unmapped: "No original position covers this location",
  source: "Original source is outside the verified package",
  budget: "Original location omitted at the evidence limit",
};
function frames(value: StackFrame[], artifact?: string | null): FrameView[] {
  return value.map((frame) => ({
    function: frame.function || "(anonymous)",
    label: locationText(frame.original ?? frame.generated),
    generatedLabel: frame.original && frame.generated ? locationText(frame.generated) : "",
    mapping: frame.mappingIssue ? mappingText[frame.mappingIssue] : "",
    artifact: artifact ?? "",
    location: frame.original ?? frame.generated,
    generated: frame.generated,
  }));
}

export interface DiagnosticViewState {
  level: string;
  page: number;
  cleared: Array<[string, { sequence: number; dropped: number }]>;
}

/** Explicit developer disclosure shared by traffic tests, live findings and visualizers. */
export class ScriptDiagnostics extends WebUIElement {
  @attr({ mode: "boolean" }) expanded = false;
  @attr({ mode: "boolean" }) navigateSources = false;
  disclosureChanged(event: Event): void {
    this.expanded = (event.target as HTMLDetailsElement).open;
  }
  saveView(): DiagnosticViewState {
    return { level: this.level, page: this.page, cleared: [...this.cleared] };
  }
  restoreView(view: DiagnosticViewState): void {
    this.level = view.level;
    this.page = view.page;
    this.cleared = new Map(view.cleared);
    this.rebuild();
  }
  @observable evidence: DeveloperEvidence[] = [];
  @observable exceptions: ExceptionView[] = [];
  @observable messages: Array<{ label: string; text: string }> = [];
  @observable logRows: LogView[] = [];
  @observable losses: string[] = [];
  @observable hasEvidence = false;
  @observable level = "all";
  @observable logCount = 0;
  @observable page = 0;
  @observable previousPage = false;
  @observable nextPage = false;
  @observable copyStatus = "";
  @observable sourceStatus = "";
  @observable sourcePage: SourcePage | null = null;
  sourceDialog!: HTMLDialogElement;
  private sourceTrigger: HTMLElement | null = null;
  private sourceVersion = 0;
  private filtered: LogView[] = [];
  private cleared = new Map<string, { sequence: number; dropped: number }>();

  evidenceChanged(): void {
    this.rebuild();
  }
  private runs(): LogRun[] {
    const runs = new Map<string, LogRun>();
    for (const item of this.evidence) {
      for (const run of item.logs ?? []) {
        runs.set(runKey(run), run);
      }
    }
    return [...runs.values()];
  }
  private rebuild(): void {
    this.exceptions = this.evidence.flatMap((item) => {
      const detail = item.exception;
      if (!detail) {
        return [];
      }
      return [detail.exception, ...detail.causes].map((value: ExceptionValue, index) => ({
        label: `${item.label}${index ? ` · Cause ${index}` : ""}`,
        title: [value.name, value.message].filter(Boolean).join(": ") || "Thrown value unavailable",
        unavailable: value.unavailable,
        truncated: index === 0 && detail.truncated,
        frames: frames(value.frames, detail.artifact),
      }));
    });
    this.messages = this.evidence.flatMap((item) =>
      (item.diagnostics ?? []).map((text) => ({ label: item.label, text })),
    );
    this.losses = this.evidence.flatMap((item) => [
      ...(item.detailsOmitted
        ? [`${item.label}: some developer details were omitted at the evidence limit.`]
        : []),
      ...(item.logRunsOmitted
        ? [`${item.label}: ${item.logRunsOmitted} older operation streams were evicted.`]
        : []),
    ]);
    const rows: LogView[] = [];
    const running = new Set(
      this.evidence.filter((item) => item.running).flatMap((item) => (item.logs ?? []).map(runKey)),
    );
    for (const run of this.runs()) {
      const key = runKey(run),
        cleared = this.cleared.get(key);
      const context = `${run.context.callback} · invocation ${run.context.invocation}${run.context.exchangeId ? ` · exchange ${run.context.exchangeId}` : ""} · revision ${run.context.revision} · worker ${run.context.worker}`;
      const dropped = Math.max(0, run.dropped - (cleared?.dropped ?? 0));
      if (dropped) {
        this.losses.push(`${context}: ${dropped} messages omitted.`);
      }
      if (run.incomplete) {
        this.losses.push(
          running.has(key)
            ? `${context}: stream in progress; received records are shown.`
            : `${context}: stream incomplete; records still inside a stopped worker cannot be recovered.`,
        );
      }
      for (const record of run.records) {
        if (
          record.sequence <= (cleared?.sequence ?? 0) ||
          (this.level !== "all" && record.level !== this.level)
        ) {
          continue;
        }
        rows.push({
          key: `${key}:${record.sequence}`,
          context,
          level: record.level,
          time: timestamp(record.timeUnixMs),
          text: record.text,
          truncated: record.truncated,
          frames: frames(record.frames, run.context.artifact),
        });
      }
    }
    this.filtered = rows;
    this.logCount = rows.length;
    this.page = Math.min(this.page, Math.max(0, Math.ceil(rows.length / 100) - 1));
    this.showPage();
    this.hasEvidence = !!(
      this.exceptions.length ||
      this.messages.length ||
      rows.length ||
      this.losses.length ||
      this.cleared.size
    );
  }
  changeLevel(event: Event): void {
    this.level = (event.target as HTMLSelectElement).value;
    this.page = 0;
    this.rebuild();
  }
  movePage(delta: number): void {
    this.page += delta;
    this.showPage();
  }
  private showPage(): void {
    this.logRows = this.filtered.slice(this.page * 100, (this.page + 1) * 100);
    this.previousPage = this.page > 0;
    this.nextPage = (this.page + 1) * 100 < this.logCount;
  }
  clearLogs(): void {
    for (const run of this.runs()) {
      this.cleared.set(runKey(run), {
        sequence: run.records.at(-1)?.sequence ?? 0,
        dropped: run.dropped,
      });
    }
    while (this.cleared.size > 64) {
      this.cleared.delete(this.cleared.keys().next().value!);
    }
    this.copyStatus = "Displayed console records cleared. New records remain visible.";
    this.page = 0;
    this.rebuild();
  }
  async copyEvidence(): Promise<void> {
    // Copy includes every page/level, but honors explicitly cleared records.
    const text = JSON.stringify(
      this.evidence.map((item) => ({
        ...item,
        logs: item.logs?.map((run) => {
          const cleared = this.cleared.get(runKey(run));
          return {
            ...run,
            records: run.records.filter((record) => record.sequence > (cleared?.sequence ?? 0)),
            dropped: Math.max(0, run.dropped - (cleared?.dropped ?? 0)),
          };
        }),
      })),
      null,
      2,
    );
    try {
      await navigator.clipboard.writeText(text);
      if (this.isConnected) {
        this.copyStatus = "Developer details copied.";
      }
    } catch {
      if (this.isConnected) {
        this.copyStatus = "Copy failed. Select the visible details to copy them.";
      }
    }
  }
  async openSource(frame: FrameView, generated: boolean, event: Event): Promise<void> {
    const location = generated ? frame.generated : frame.location;
    if (!location || !frame.artifact) {
      return;
    }
    if (this.navigateSources) {
      this.$emit("diagnostic-source", { artifact: frame.artifact, location });
      return;
    }
    const version = ++this.sourceVersion;
    this.sourceTrigger = event.currentTarget as HTMLElement;
    this.sourceStatus = "Loading the executing source snapshot…";
    this.sourcePage = null;
    this.sourceDialog.showModal();
    try {
      const source = await invoke<SourcePage>("script_source", {
        artifact: frame.artifact,
        ...location,
      });
      if (this.isConnected && version === this.sourceVersion && this.sourceDialog.open) {
        this.sourcePage = source;
        this.sourceStatus = "Read-only executing snapshot. Your draft is unchanged.";
      }
    } catch (error: unknown) {
      if (this.isConnected && version === this.sourceVersion && this.sourceDialog.open) {
        this.sourceStatus = describeError(error);
      }
    }
  }
  closeSource(): void {
    this.sourceDialog.close();
  }
  sourceClosed(): void {
    ++this.sourceVersion;
    if (this.sourceTrigger?.isConnected) {
      this.sourceTrigger.focus({ preventScroll: true });
    }
  }
  sourceKey(event: KeyboardEvent): void {
    event.stopPropagation();
  }
  disconnectedCallback(): void {
    ++this.sourceVersion;
    this.sourceDialog?.close();
    super.disconnectedCallback();
  }
}
ScriptDiagnostics.define("script-diagnostics");
