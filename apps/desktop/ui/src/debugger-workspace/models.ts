import type {
  DeveloperEvidence,
  ExceptionDiagnostic,
  LogRun,
  LogValue,
  StackFrame,
  SourceLocation,
} from "../script-diagnostics/models.js";
export interface DebugSessionInfo {
  session: string;
  mode: string;
  label: string;
  artifact: string;
  source: string;
}
export interface DebugFrame {
  index: number;
  function: string;
  location: SourceLocation | null;
  original?: SourceLocation | null;
  unavailable: boolean;
}
export interface DebugScope {
  kind: string;
  name: string;
  handle: number | null;
}
export interface DebugValue {
  kind: string;
  display: string;
  handle: number | null;
  truncated: boolean;
}
export interface DebugProperty {
  name: string;
  value: DebugValue;
}
export interface SourceBreakpoint {
  logExpression?: string | undefined;
  id: number;
  location: SourceLocation;
  condition?: string | undefined;
}
export interface DebugConfiguration {
  exceptions: string;
  breakpoints: SourceBreakpoint[];
}
export interface BreakpointResolution {
  id: number;
  requested: SourceLocation;
  resolved: SourceLocation | null;
  issue: string | null;
}
export type DebugAction =
  | { kind: "skip-pauses"; skip: boolean }
  | { kind: "pause-next"; enabled: boolean }
  | { kind: "continue-to"; pause: number; location: SourceLocation }
  | {
      kind: "evaluate" | "inspect" | "complete";
      expression: string;
      pause: number;
      frame: number | null;
    }
  | { kind: "resume"; pause: number; action: "continue" | "into" | "over" | "out" }
  | { kind: "scopes"; pause: number; frame: number }
  | { kind: "properties"; pause: number; handle: number; offset: number }
  | {
      kind: "preview-properties";
      pause: number;
      handle: number;
      offset: number;
      limit: number;
      refresh: boolean;
    }
  | { kind: "property"; pause: number; handle: number; index: number }
  | { kind: "release"; pause: number; handles: number[] }
  | { kind: "exceptions"; policy: "none" | "uncaught" | "all" }
  | { kind: "breakpoints"; values: SourceBreakpoint[] }
  | { kind: "stop" | "detach" };
export type DebugEvent =
  | { kind: "control"; skipPauses: boolean; pauseNext: boolean }
  | { kind: "logpoint-started" }
  | {
      kind: "logpoint";
      location: SourceLocation | null;
      original?: SourceLocation | null;
      text: string;
      thrown: boolean;
      omitted: number;
      timeUnixMs: number;
    }
  | { kind: "completions"; pause: number; matches: string[]; omitted: boolean }
  | { kind: "output"; level: string; text: string; values?: LogValue[]; frames?: StackFrame[] }
  | { kind: "evaluating" }
  | { kind: "evaluated"; pause: number; value: DebugValue; thrown: boolean }
  | { kind: "attached" | "detached" | "resumed" | "acknowledged" }
  | {
      kind: "sources";
      sources: Array<{ source: string; bytes: number | null }>;
      breakpoints: BreakpointResolution[];
    }
  | {
      kind: "paused";
      pause: number;
      reason: string;
      frames: DebugFrame[];
      exception: ExceptionDiagnostic | null;
      truncated: boolean;
    }
  | { kind: "scopes"; pause: number; frame: number; scopes: DebugScope[] }
  | {
      kind: "properties" | "property-preview";
      pause: number;
      handle: number;
      offset: number;
      properties: DebugProperty[];
      next: number | null;
      total?: number;
    }
  | { kind: "rejected"; message: string }
  | { kind: "stopped"; reason: string };
export interface DebugSessionView {
  request?: { id: string; method: string; url: string } | null;
  info: DebugSessionInfo;
  stream: {
    operation?: { callback: string; invocation: number; exchangeId: string | null } | null;
    valueGeneration?: number;
    skipPauses?: boolean;
    pauseNext?: boolean;
    exceptions?: "none" | "uncaught" | "all";
    paused?: {
      session: string;
      invocation: number;
      request: number;
      event: DebugEvent;
      valueGeneration?: number;
    } | null;
    inventory?: {
      sources: Array<{ source: string; bytes: number | null }>;
      breakpoints: BreakpointResolution[];
    };
    active: boolean;
    exchangeId: string | null;
    armRemainingMs: number | null;
    cursor: number;
    omitted: boolean;
    closed: boolean;
    pauseRemainingMs: number | null;
    sessionRemainingMs: number;
    logs: LogRun[];
    events: Array<{
      session: string;
      invocation: number;
      request: number;
      event: DebugEvent;
      valueGeneration?: number;
    }>;
  };
  outcome: {
    status: string;
    summary: string;
    result: unknown;
    truncated: boolean;
    exception: ExceptionDiagnostic | null;
    logs: LogRun[];
  } | null;
}
export function evidence(
  view: DebugSessionView,
  exception: ExceptionDiagnostic | null,
): DeveloperEvidence[] {
  return [
    {
      label: view.info.label,
      running: view.stream.active && !view.stream.closed,
      exception: view.outcome?.exception ?? exception,
      logs: view.stream.logs.length ? view.stream.logs : (view.outcome?.logs ?? []),
    },
  ];
}
