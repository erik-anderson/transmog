import type { ResponseAsset } from "./models.js";
export type ComparisonSubject =
  | { kind: "exchange"; id: string; requestBoundary?: string; responseBoundary?: string }
  | { kind: "asset"; reference: string }
  | {
      kind: "draft";
      label: string;
      status: string;
      headers: ResponseAsset["headers"];
      decodedBody: number[] | null;
      base: ComparisonSubject | null;
      bodyPath: string | null;
    }
  | { kind: "preview"; run: string; observation: number };
export type ComparisonOptions = { excludedHeaders: string[]; normalizeNewlines: boolean };
export type ComparisonRow = {
  index: number;
  section: string;
  field: string;
  state: string;
  left: string;
  right: string;
  note: string;
};
export type ComparisonSide = {
  label: string;
  source: string;
  entryId: string | null;
  acquiredAt: number;
  boundary: string;
  notes: string[];
};
export type ComparisonResult = {
  id: string;
  left: ComparisonSide;
  right: ComparisonSide;
  different: boolean;
  complete: boolean;
  changed: number;
  unknown: number;
  excluded: number;
  totalRows: number;
  moreChanges: boolean;
  rows: ComparisonRow[];
  bodySections: string[];
  options: ComparisonOptions;
};
export type ScopeChoice = { value: string; label: string; scope: unknown };
export type ComparisonOpen = {
  left: ComparisonSubject;
  right: ComparisonSubject;
  options?: ComparisonOptions;
};
export type CaptureOpen = { choices: ScopeChoice[] };
export type PairingOptions = {
  excludedQueryNames: string[];
  requestHeaders: string[];
  requestBodyDigest: boolean;
};
export type PairingRow = {
  index: number;
  left: string | null;
  right: string | null;
  leftLabel: string;
  rightLabel: string;
  reason: string;
  state: string;
  group: string;
  timingDelta: string | null;
};
export type CaptureComparison = {
  id: string;
  leftLabel: string;
  rightLabel: string;
  leftDetails: string;
  rightDetails: string;
  leftCount: number;
  rightCount: number;
  changed: number;
  equalMetadata: number;
  unresolved: number;
  leftOnly: number;
  rightOnly: number;
  unavailable: number;
  total: number;
  rows: PairingRow[];
  options: PairingOptions;
  note: string;
  canUndo: boolean;
};
