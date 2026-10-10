export type Lifecycle = 'stopped' | 'running' | 'draining' | 'stopping' | 'failed';

export interface AppStatus {
  lifecycle: Lifecycle;
  listener: string | null;
  summary: string;
  hostRestorePending: boolean;
}
export interface CaIdentity { sha256: string; certificatePath: string; }
export interface DesktopBootstrap {
  viewerMode?: boolean;
  caCertificatePath: string;
  caPrivateKeyPath: string;
  caFilesPresent: boolean;
  caFilesExist: boolean;
  caKeyError: string | null;
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
  durationMs: number | null;
  traceId?: string | null;
  requestBytes: number;
  responseBytes: number;
  terminal: 'active' | 'completed' | 'failed';
  loss: boolean;
  capturing: boolean;
  autoResponse: AutoResponseMatch | null;
  url: string;
  startedAt: number;
  contentType: string | null;
  topLevelNavigation: boolean;
  fetchDestination: string | null;
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

export interface TrafficView {id:string;ids:string[];page:SessionPage;}

export interface SessionHint { exchangeId: string | null; sequence: number; lagged: boolean; }
export interface SessionDetail {
  performance?: { work?:Array<{kind:string;label:string;beganOffsetMicros:number;endedOffsetMicros:number;busyNanos:number;calls:number}>; points:Array<{milestone:string;unixMillis:number;offsetMicros:number}>; protocols:Array<{boundary:string;version:string;reason:string|null}>; transports:Array<{leg:string;connectionId:string;shared:boolean;setupTimings?:Array<{phase:string;beganOffsetMicros:number;endedOffsetMicros:number;requestWaitMicros:number}>;outcome:string;sampledOffsetMicros:number;peer:string|null;local:string|null;dnsMicros:number|null;tcpMicros:number|null;tlsMicros:number|null;tlsVersion:string|null;tlsResumed:boolean|null;cipher:string|null;alpn:string|null;bytesRead:number|null;bytesWritten:number|null;tcp?:{rttMicros:number|null;minRttMicros:number|null;congestionWindow:number|null;sendWindow:number|null;receiveWindow:number|null;unacknowledgedBytes:number|null;retransmittedBytes:number|null;retransmittedSegments:number|null;fastRetransmissions:number|null;duplicateAcks:number|null;timeoutEpisodes:number|null;mss:number|null;connectionAgeMillis:number|null}|null;tcpSampledOffsetMicros?:number|null;socketIo?:{writeWaitMicros:number;writeWaits:number;lastWriteOffsetMicros:number|null;lastFlushOffsetMicros:number|null}|null;quic?:{rttMicros:number|null;congestionWindow:number|null;packetsSent:number;packetsReceived:number;packetsLost:number;retransmittedBytes:number}|null}> };
  traceId?:string|null; originalId?:string|null;
  savedEvidence?: Record<string, string>;
  sourceIp?: string;
  id: string;
  startedAt?:number;
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
  terminalState: SessionSummary['terminal'];
  websocket: string | null;
  sequenceLoss: number;
  autoResponse: AutoResponseMatch | null;
}
export interface HeaderSummary { totalFields:number;valueBytes:number|null;serializedBytes:number|null;authorization:string;proxyAuthorization:string; }
export interface HeaderPage {summary:HeaderSummary;headers:HeaderView[];offset:number;nextOffset:number|null;}
export interface HeaderView { index?:number; name: string; value: string; binary: boolean; sensitive: boolean; valueBytes?:number|null; fieldBytes?:number|null; }
export interface HeadView {
  summary?:HeaderSummary;
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
  lengthKnown?:boolean;
  exchangeId: string;
  boundary: string;
  observedBytes: number;
  wireBodyBytes?:number|null;
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
  bytesBase64: string | null;
  byteOffset: number;
  truncated: boolean;
  nextOffset: number | null;
  warning: string | null;
  previewHandle: string | null;
  previewMimeType: string | null;
}
export type BreakpointPhase = 'request-head' | 'request-body' | 'response-head' | 'response-body';
export interface PausedExchange {
  decisionId: number;
  expiresAtUnixMs?:number;
  exchangeId: string;
  phase: BreakpointPhase;
  requestHead: Record<string, unknown> | null;
  responseHead: Record<string, unknown> | null;
  bodyHex: string | null;
  hookId: string;
}
export interface BreakpointStatus { enabled: boolean; paused: PausedExchange[]; }
export interface ComposerSnapshot {id:number;method:string;target:string;status:number|null;source?:ComposerOrigin|null;}
export interface ComposerOrigin {entryId:string;traceId:string|null;traceName?:string|null;originalId:string|null;}
export interface ComposerResult {
  source?:ComposerOrigin|null;
  id: number;
  status: number;
  headers: Array<{name: string; value: string}>;
  body: string;
  bodyIsHex: boolean;
  truncated: boolean;
  attribution: string;
}
export interface CaptureReadModel {state:'idle'|'active'|'sealed'|'failed'|'shutdown';path?:string|null;bytesWritten?:number;message?:string;}
export interface CaptureSummaryView {records:number;exchanges:number;lossMarkers:number;retainedBodyBytes:number;sealed:boolean;truncatedTail:boolean;validBytes:number;}
export interface CaptureExportResult {destination:string;records:number;bytes:number;sourceSealed:boolean;sourceTruncatedTail:boolean;fidelity:string;}
export interface TraceMetadata {id:string;name:string;format:string;path:string;sessions:number;importedAt:number;context:unknown;notes:string[];}
export interface TraceImportResult {trace:TraceMetadata;issues:string[];}
export interface TraceImportIntent {paths:string[];}
export interface TraceInspection {path:string;format:string;sessions:number;bytes:number;context:unknown;sources:TraceMetadata[];notes:string[];}
export interface TraceImportProgress {operationId:string;completed:number;total:number;}
export interface CapturedPageReport {url:string;scope:string;source:string;scriptsEnabled?:boolean|null;available:number;skipped:number;bytes:number;hits:number;misses:number;resources:Array<{entryId:string;source:string;sourceAvailable?:boolean;method:string;url:string;unixMillis:number|null;bytes:number|null;decision:string}>;requests:Array<{id:number;method:string;url:string;outcome:string;entryId:string|null;reason:string}>;}
export interface TrafficSearchMatch {boundary:string;field:string;startUtf16:number;endUtf16:number;before:string;matched:string;after:string;shortened:boolean;}
export interface TrafficSearchEntry {entryId:string;matches:TrafficSearchMatch[];moreMatches:boolean;}
export interface TrafficSearchResult {id:string;operationId:string;ids:string[];examined:number;binaryBodies:number;unavailableBodies:number;}
export interface TrafficSearchProgress {operationId:string;completed:number;total:number;}
export interface ProductState {
  schemaVersion: number;
  preferences: { theme: 'system' | 'light' | 'dark'; sessionPageSize: number; configureSystemProxy: boolean };
  privacy: { maxLiveEntries: number | null; bufferLimit: {mode: 'automatic'} | {mode: 'custom'; bytes: number} | {mode: 'unlimited'}; retainRequestBodies: boolean; requestBodyLimit: number | null; redactSensitiveHeaders: boolean; retainResponseBodies: boolean; retainBodySamples: boolean; rememberRecentArtifacts: boolean; includePathsInSupportBundles: boolean };
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
  autoresponseSplit:number;
  wrapCells: boolean;
  compactRows: boolean;
  columns: ColumnPreference[];
}
export interface TrafficSort { column: ColumnId; direction: 'ascending' | 'descending'; }
export interface TrafficFilter { column: ColumnId; operator: 'contains' | 'equals' | 'minimum' | 'maximum'; value: string; label: string; }
export interface AutomationCandidate { candidateId: string; ruleCount: number; registrationCount: number; }
export type QueryCondition = {kind:'exact';value:string|null} | {kind:'parameters';value:Array<{name:string;value:string}>} | {kind:'ignore'};
export type UrlCondition = {kind:'exact';value:string} | {kind:'pattern';value:{address:string;query:QueryCondition;caseSensitive:boolean}} | {kind:'regex';value:{pattern:string;scope:'url'|'path';whole:boolean;caseSensitive:boolean;query:QueryCondition|null}};
export interface MatchExample {method:string;url:string;expected:boolean;headers?:Array<{name:string;value:string}>;}
export interface MatchTestResult {
  test:{matched:boolean;normalizedUrl:string;checks:Array<{label:string;matched:boolean;detail:string}>;captures:Array<{label:string;value:string}>};
  wouldServe:boolean;explanation:string;winningRule:string|null;examples:Array<{method:string;url:string;passed:boolean}>;
}
export interface AutomationRule {
  id: string;
  displayName?: string | null;
  enabled?: boolean;
  revision: number;
  priority: number;
  matcher: {
    method: string | null;
    url?: UrlCondition | null;
    examples?: MatchExample[];
    scheme: string | null;
    host: string | null;
    port: number | null;
    pathPrefix: string | null;
    query: string | null;
    requestHeaders: Array<{name: string; condition: {kind: string; value?: number[] | string}}>;
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
  usage?:Array<{ruleId:string;matches:number;lastMatchedAt:number}>;
  rules: AutomationRule[];
  candidateCount: number;
  historyCount: number;
}
export interface ResponseAsset {
  id: string; revision: number; status: number; bodyBytes: number; sha256: string; mediaType: string | null;
  provenance: {kind: 'authored' | 'imported'} | {kind: 'session'; exchange_id: string; boundary: string};
  headers:Array<{name:number[];value:number[]}>;
}
export interface ResponseAssetInspection {asset:ResponseAsset;display:string;textEncoding:string|null;contentCodings:string[];explanation:string;}
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
export type NoticeAction = 'setup-ca' | 'reset-ca' | 'start-proxy' | 'settings' | 'recover-proxy' | null;
export type RequestCommandFormat = 'curl' | 'powershell';
export interface RequestCommand {text:string;notices:string[];bodyFileRequired:boolean;bodyFileAvailable:boolean;}
export interface ComposerSource {method:string;url:string;headers:Array<{name:string;value:string}>;body:string;bodyAvailable:boolean;bodyStreamed?:boolean;bodyBytes?:number|null;notices:string[];}
export interface Notice { title: string; message: string; actionLabel: string | null; action: NoticeAction; }

export interface TracePasswordPrompt {title:string;confirm:boolean;message:string;resolve:(password:string|null)=>void;}
export interface OperationError {title:string;message:string;resolve:()=>void;}
