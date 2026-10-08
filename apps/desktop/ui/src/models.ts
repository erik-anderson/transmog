export type Lifecycle = 'stopped' | 'running' | 'stopping' | 'failed';

export interface AppStatus {
  lifecycle: Lifecycle;
  listener: string | null;
  summary: string;
  hostRestorePending: boolean;
}
export interface CaIdentity { sha256: string; certificatePath: string; }
export interface DesktopBootstrap {
  caCertificatePath: string;
  caPrivateKeyPath: string;
  caFilesPresent: boolean;
  ownedCaSha256: string | null;
  ownedCaTrusted: boolean;
  hostRestorePending: boolean;
  diagnosticsPath: string;
}

export interface ClientIdentity {
  kind: 'local-process' | 'local-unknown' | 'remote';
  processName: string | null;
  processId: number | null;
}

export interface SessionSummary {
  id: string;
  caller: ClientIdentity;
  method: string;
  host: string;
  path: string;
  protocol: string;
  status: number | null;
  durationMs: number;
  requestBytes: number;
  responseBytes: number;
  terminal: 'active' | 'completed' | 'failed';
  loss: boolean;
  capturing: boolean;
  autoResponse: AutoResponseMatch | null;
  url: string;
  startedAt: number;
  contentType: string | null;
}

export interface SessionPage {
  sessions: SessionSummary[];
  nextCursor: string | null;
  evicted: number;
  sequenceGaps: number;
  subscriberLag: number;
  totalMatched: number;
  retainedCount: number;
  focusOffset: number | null;
}

export interface SessionHint { exchangeId: string | null; sequence: number; lagged: boolean; }
export interface SessionDetail {
  id: string;
  caller: ClientIdentity;
  requests: HeadView[];
  responses: HeadView[];
  bodies: unknown[];
  storedBodies: StoredBodyMetadata[];
  diagnostics: string[];
  hookEffects: string[];
  routeSelection: string | null;
  routeAttempts: string[];
  terminal: string;
  websocket: string | null;
  sequenceLoss: number;
  autoResponse: AutoResponseMatch | null;
}
export interface HeaderView { name: string; value: string; binary: boolean; sensitive: boolean; }
export interface HeadView {
  boundary: string;
  method: string | null;
  target: string | null;
  status: number | null;
  protocol: string;
  headers: HeaderView[];
}
export interface AutoResponseMatch {
  ruleId: string;
  ruleName: string;
  ruleRevision: number;
  position: number;
  assetReference: string;
  status: number;
  bodyBytes: number;
}
export interface StoredBodyMetadata {
  exchangeId: string;
  boundary: string;
  observedBytes: number;
  retainedBytes: number;
  availability: string;
  mediaType: string | null;
  charset: string | null;
  contentCodings: string[];
  sha256: string | null;
  reason: string | null;
}
export interface BodyInspection {
  metadata: StoredBodyMetadata;
  representation: string;
  decoded: boolean;
  textEncoding: string | null;
  display: string;
  displayBytes: number;
  truncated: boolean;
  nextOffset: number | null;
  warning: string | null;
  previewHandle: string | null;
  previewMimeType: string | null;
}
export type BreakpointPhase = 'request-head' | 'request-body' | 'response-head' | 'response-body';
export interface PausedExchange {
  decisionId: number;
  exchangeId: string;
  phase: BreakpointPhase;
  requestHead: Record<string, unknown> | null;
  responseHead: Record<string, unknown> | null;
  bodyHex: string | null;
  hookId: string;
}
export interface BreakpointStatus { enabled: boolean; paused: PausedExchange[]; }
export interface ComposerResult {
  id: number;
  status: number;
  headers: Array<{name: string; value: string}>;
  body: string;
  bodyIsHex: boolean;
  truncated: boolean;
  attribution: string;
}
export type CaptureReadModel = Record<string, unknown>;
export interface ProductState {
  schemaVersion: number;
  preferences: { theme: 'system' | 'light' | 'dark'; sessionPageSize: number; configureSystemProxy: boolean };
  privacy: { retainResponseBodies: boolean; retainBodySamples: boolean; rememberRecentArtifacts: boolean; includePathsInSupportBundles: boolean };
  window: { width: number; height: number; x: number | null; y: number | null; maximized: boolean };
  recentArtifacts: Array<{path: string; kind: string}>;
  workspace: WorkspacePreferences;
}

export type ColumnId = 'method' | 'status' | 'process' | 'pid' | 'host' | 'path' | 'url' | 'protocol' | 'duration' | 'response-bytes' | 'request-bytes' | 'state' | 'content-type' | 'started-at';
export interface ColumnPreference { id: ColumnId; width: number; visible: boolean; pinned: boolean; }
export interface WorkspacePreferences {
  sidebarCollapsed: boolean;
  layout: 'stacked' | 'side-by-side';
  listSplit: number;
  requestSplit: number;
  requestBodySplit: number;
  responseBodySplit: number;
  wrapCells: boolean;
  compactRows: boolean;
  columns: ColumnPreference[];
}
export interface TrafficSort { column: ColumnId; direction: 'ascending' | 'descending'; }
export interface TrafficFilter { column: ColumnId; operator: 'contains' | 'equals' | 'minimum' | 'maximum'; value: string; label: string; }
export interface AutomationCandidate { candidateId: string; ruleCount: number; registrationCount: number; }
export interface AutomationRule {
  id: string;
  displayName?: string | null;
  enabled?: boolean;
  revision: number;
  priority: number;
  matcher: {
    method: string | null;
    url?: {kind: 'exact'; value: string} | null;
    scheme: string | null;
    host: string | null;
    port: number | null;
    pathPrefix: string | null;
    query: string | null;
    requestHeaders: Array<{name: string; condition: {kind: string; value?: number[]}}>;
    responseHeaders: Array<unknown>;
    responseStatus: number | null;
    responseStatusClass: number | null;
  };
  request: {
    headers: unknown[];
    replaceBody: number[] | null;
    discardBody: boolean;
    abortReason: string | null;
    responseAsset: string | null;
    allowNonIdempotentBodyReplacement: boolean;
  };
  response: {headers: unknown[]; replaceBody: number[] | null; discardBody: boolean; abortReason: string | null};
}
export interface AutomationStatus {
  generation: number;
  autoresponsesEnabled: boolean;
  diagnostics: Array<{ruleId:string; supersededBy:string; duplicateResponse:boolean}>;
  rules: AutomationRule[];
  candidateCount: number;
  historyCount: number;
}
export interface ResponseAsset {
  id: string; revision: number; status: number; bodyBytes: number; sha256: string; mediaType: string | null;
  provenance: {kind: 'authored' | 'imported'} | {kind: 'session'; exchange_id: string; boundary: string};
}
export interface CapturedAutoResponseSource {
  kind: 'captured';
  sessionId: string;
  boundary: 'client-response';
  contentCodings: string[];
  bodyEditable: boolean;
  textEncoding: string | null;
}
export interface ScratchAutoResponseSource { kind: 'scratch'; }
export interface ExistingAutoResponseSource { kind: 'existing'; assetReference: string; }
export type AutoResponseSource = CapturedAutoResponseSource | ScratchAutoResponseSource | ExistingAutoResponseSource;
export type ScriptHandler = 'onRequestHead' | 'onRequestBody' | 'onResponseHead' | 'onResponseBody';
export interface ScriptDraft {
  id: string;
  revision: number;
  source: string;
  handlers: ScriptHandler[];
  prefilter: { method: string | null; host: string | null; pathPrefix: string | null };
  capabilities: {
    readSensitiveHeaders: boolean;
    readBodies: boolean;
    writeHeaders: string[];
    writeBody: boolean;
    respond: boolean;
    abort: boolean;
  };
  limits: {
    maxInputBodyBytes: number;
    maxOutputBytes: number;
    maxLogBytes: number;
    maxHeapBytes: number;
    maxDurationMs: number;
  };
  priority: number;
}
export interface ScriptRevision { manifest: { id: string; revision: number; sourceHash: string }; source: string; }
export interface ScriptStatus {
  generation: number;
  active: ScriptRevision[];
  saved: ScriptDraft[];
  candidateCount: number;
  historyCount: number;
}
export interface ScriptCandidate { candidateId: string; scriptId: string; revision: number; sourceHash: string; }


export interface SelectedResponse { sessionId: string; detail: SessionDetail; reusable: boolean; }
export type ViewName = 'traffic' | 'breakpoints' | 'automation' | 'composer' | 'captures' | 'settings';
export type NoticeAction = 'setup-ca' | 'start-proxy' | 'settings' | 'recover-proxy' | null;
export interface Notice { title: string; message: string; actionLabel: string | null; action: NoticeAction; }
