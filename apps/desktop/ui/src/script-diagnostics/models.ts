/** Native snapshots are inert local evidence; none of these strings are markup or URLs. */
export interface SourceLocation {
  source: string;
  line: number;
  column: number;
}
export interface StackFrame {
  function: string;
  generated: SourceLocation | null;
  original?: SourceLocation | null;
  mappingIssue?: "missing" | "invalid" | "unmapped" | "source" | "budget";
}
export interface ExceptionValue {
  name: string | null;
  message: string;
  frames: StackFrame[];
  unavailable: boolean;
}
export interface ExceptionDiagnostic {
  artifact?: string | null;
  exception: ExceptionValue;
  causes: ExceptionValue[];
  truncated: boolean;
}
export interface LogRecord {
  sequence: number;
  timeUnixMs: number;
  level: "debug" | "info" | "warn" | "error" | "trace";
  text: string;
  frames: StackFrame[];
  truncated: boolean;
}
export interface LogRun {
  context: {
    artifact?: string | null;
    worker: string;
    revision: string;
    callback: string;
    invocation: number;
    exchangeId: string | null;
  };
  records: LogRecord[];
  dropped: number;
  incomplete: boolean;
}
export interface DeveloperEvidence {
  running?: boolean | undefined;
  label: string;
  exception?: ExceptionDiagnostic | null | undefined;
  logs?: LogRun[] | undefined;
  diagnostics?: string[] | undefined;
  detailsOmitted?: boolean | undefined;
  logRunsOmitted?: number | undefined;
}
export interface SourcePage {
  artifact: string;
  source: string;
  line: number;
  column: number;
  totalLines: number;
  lines: Array<{ number: number; startColumn: number; text: string; shortened: boolean }>;
}
/** Tauri command errors retain authored evidence only on explicit local commands. */
export function errorEvidence(error: unknown, label: string): DeveloperEvidence {
  if (typeof error !== "object" || error === null) {
    return { label };
  }
  const native = error as { exception?: ExceptionDiagnostic; logs?: LogRun[] };
  return { label, exception: native.exception, logs: native.logs };
}
