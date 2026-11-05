import type { DebugSessionView } from "./models.js";
import type { LogRun } from "../script-diagnostics/models.js";

export interface ClearedLog {
  key: string;
  sequence: number;
  dropped: number;
  incomplete: boolean;
}
export function consoleRuns(view: DebugSessionView): LogRun[] {
  return view.stream.logs.length ? view.stream.logs : (view.outcome?.logs ?? []);
}
export function consoleRunKey(view: DebugSessionView, run: LogRun): string {
  const context = run.context;
  return `${view.info.session}/${context.worker}/${context.invocation}/${context.callback}`;
}
function incomplete(view: DebugSessionView, run: LogRun): boolean {
  return (
    run.incomplete &&
    !(view.stream.active && view.stream.operation?.invocation === run.context.invocation)
  );
}
/** Clear received output while allowing later records and loss reports from the same run. */
export class ConsoleOutput {
  private cleared = new Map<string, ClearedLog>();
  snapshot(): ClearedLog[] {
    return [...this.cleared.values()].map((entry) => ({ ...entry }));
  }
  restore(entries: ClearedLog[]): void {
    this.cleared = new Map(entries.map((entry) => [entry.key, { ...entry }]));
  }
  clear(views: Iterable<DebugSessionView>): void {
    for (const view of views) {
      for (const run of consoleRuns(view)) {
        const key = consoleRunKey(view, run);
        this.cleared.set(key, {
          key,
          sequence: run.records.at(-1)?.sequence ?? 0,
          dropped: run.dropped,
          incomplete: incomplete(view, run),
        });
      }
    }
  }
  records(view: DebugSessionView, run: LogRun): LogRun["records"] {
    const sequence = this.cleared.get(consoleRunKey(view, run))?.sequence ?? 0;
    return run.records.filter((record) => record.sequence > sequence);
  }
  loss(view: DebugSessionView, run: LogRun): { dropped: number; incomplete: boolean } {
    const cleared = this.cleared.get(consoleRunKey(view, run));
    return {
      dropped: Math.max(0, run.dropped - (cleared?.dropped ?? 0)),
      incomplete: incomplete(view, run) && !cleared?.incomplete,
    };
  }
  retain(views: Iterable<DebugSessionView>): void {
    const keys = new Set(
      [...views].flatMap((view) => consoleRuns(view).map((run) => consoleRunKey(view, run))),
    );
    for (const key of this.cleared.keys()) {
      if (!keys.has(key)) {
        this.cleared.delete(key);
      }
    }
  }
}
