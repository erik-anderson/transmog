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
  requestNumber: string;
  id: string;
  caller: ClientIdentity;
  method: string;
  host: string;
  path: string;
  protocol: string;
  streamProtocol?: 'web-socket' | 'sse' | null;
  status: number | null;
  durationMs: number | null;
  httpDurationMs?: number | null;
  responseHeadersMs?: number | null;
  streamLifetime?: StreamLifetime | null;
  traceId?: string | null;
  streamSent?: number | null;
  streamReceived?: number | null;
  requestBytes: number;
  responseBytes: number;
  terminal: 'active' | 'completed' | 'failed' | 'cancelled' | 'interrupted' | 'open-at-capture' | 'incomplete';
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
export interface TrafficTimelineRow {
  id:string;requestNumber:string;method:string;url:string;traceId:string|null;
  startMillis:number|null;durationMicros:number|null;responseMicros:number|null;basis:string;
  terminal:SessionSummary['terminal'];
}

export interface SessionHint { exchangeId: string | null; sequence: number; lagged: boolean; }
export interface SessionDetail {
  stream?:StreamSummary|null;
  composerOrigin?:ComposerOrigin|null;
  performance?: { work?:Array<{kind:string;label:string;beganOffsetMicros:number;endedOffsetMicros:number;busyNanos:number;calls:number}>; points:Array<{milestone:string;unixMillis:number;offsetMicros:number}>; protocols:Array<{boundary:string;version:string;reason:string|null}>; transports:Array<{leg:string;connectionId:string;shared:boolean;setupTimings?:Array<{phase:string;beganOffsetMicros:number;endedOffsetMicros:number;requestWaitMicros:number}>;outcome:string;tunnelDestination?:string|null;sampledOffsetMicros:number;peer:string|null;local:string|null;dnsMicros:number|null;tcpMicros:number|null;tlsMicros:number|null;tlsVersion:string|null;tls?:unknown;tlsResumed:boolean|null;cipher:string|null;alpn:string|null;bytesRead:number|null;bytesWritten:number|null;tcp?:{rttMicros:number|null;minRttMicros:number|null;congestionWindow:number|null;sendWindow:number|null;receiveWindow:number|null;unacknowledgedBytes:number|null;retransmittedBytes:number|null;retransmittedSegments:number|null;fastRetransmissions:number|null;duplicateAcks:number|null;timeoutEpisodes:number|null;mss:number|null;connectionAgeMillis:number|null}|null;tcpSampledOffsetMicros?:number|null;socketIo?:{writeWaitMicros:number;writeWaits:number;lastWriteOffsetMicros:number|null;lastFlushOffsetMicros:number|null}|null;quic?:{rttMicros:number|null;congestionWindow:number|null;packetsSent:number;packetsReceived:number;packetsLost:number;retransmittedBytes:number}|null}> };
  traceId?:string|null; originalId?:string|null;
  requestNumber:string; originalRequestNumber?:string|null; originalRequestNumberProvenance?:string|null;
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
  headerEffects?:HeaderEffect[];
  bodyEdits?:Array<{sequence:number;hookId:string;name:string;report:BodyEditReport}>;
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
  snapshot?: {status:number;bodyBytes:number;sha256:string;contentCodings:string[]} | null;
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
export interface BodyBytePage {metadata:StoredBodyMetadata;totalBytes:string;offset:string;bytesBase64:string;readLimit:number;}
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
  detail:SessionDetail;
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
export interface PreviewRuleSnapshot {id:string;name:string;revision:number;asset:string;sha256:string;bytes:number;shadowedBy:string|null;}
export interface CapturedPageReport {caseOrigin?:string|null;variant?:ExperimentSelection|null;url:string;scope:string;source:string;selectedEntryId:string;runtime:string;bytesAvailable:boolean;rules:PreviewRuleSnapshot[];scriptsEnabled?:boolean|null;available:number;skipped:number;bytes:number;hits:number;misses:number;failures:number;resources:Array<{entryId:string;source:string;sourceAvailable?:boolean;method:string;url:string;unixMillis:number|null;bytes:number|null;sha256:string|null;decision:string}>;requests:Array<{id:number;method:string;url:string;outcome:string;entryId:string|null;ruleId:string|null;sha256:string|null;reason:string}>;}
export interface TrafficSearchMatch {boundary:string;field:string;startUtf16:number;endUtf16:number;before:string;matched:string;after:string;shortened:boolean;}
export interface TrafficSearchEntry {entryId:string;matches:TrafficSearchMatch[];moreMatches:boolean;}
export interface TrafficSearchResult {id:string;operationId:string;ids:string[];examined:number;binaryBodies:number;unavailableBodies:number;}
export interface TrafficSearchProgress {operationId:string;completed:number;total:number;}
export interface ProductState {
  schemaVersion: number;
  preferences: { networkProfiles:NetworkProfiles; clientCertificatePolicy?:ClientCertificatePolicy; theme: 'system' | 'light' | 'dark'; sessionPageSize: number; configureSystemProxy: boolean; decryptHttps:boolean; allowRemoteClients:boolean; proxyPort:number|null; reuseServerConnections:boolean; isolateOutboundConnections: boolean; allowedOutboundProtocols: {http1:boolean;http2:boolean;http3:boolean} };
  privacy: { maxLiveEntries: number | null; bufferLimit: {mode: 'automatic'} | {mode: 'custom'; bytes: number} | {mode: 'unlimited'}; retainRequestBodies: boolean; requestBodyLimit: number | null; customRequestBodyLimit?: boolean; redactSensitiveHeaders: boolean; retainResponseBodies: boolean; retainBodySamples: boolean; rememberRecentArtifacts: boolean; includePathsInSupportBundles: boolean };
  window: { width: number; height: number; x: number | null; y: number | null; maximized: boolean };
  recentArtifacts: Array<{path: string; kind: string}>;
  workspace: WorkspacePreferences;
}

export interface NetworkProfileSettings {
  protocols:{http1:boolean;http2:boolean;http3:boolean}; reuseConnections:boolean;
  tlsVersions:'tls12-and13'|'tls12-only'|'tls13-only'; compatibilityCiphers:boolean;
  hybridKeyExchange:boolean; grease:'chromium'|'no-signature'|'disabled';
  randomizeExtensions:boolean; echGrease:boolean; encryptedClientHello:boolean; certificateCompression:boolean; http2Alps:boolean; http3Alps:boolean;
  trustAnchorIds:boolean; pkiMetadataFile:string|null;
  rootStore:'os'|'chromium-bundled'|'chrome-installed'|'webview2-installed';
  localCertificates:boolean;
}
export interface NetworkProfileSnapshot {
  id:string; name:string; revision:number; baseProfile:string; settings:NetworkProfileSettings;
}
export interface NetworkProfiles {
  revision:number; activeProfile:NetworkProfileSnapshot; customProfiles:NetworkProfileSnapshot[];
}

export interface ClientCertificateIdentity {id:string;fingerprint:string;subject:string;issuer:string;validUntil:string}
export type ClientCertificateAction = {action:'noCertificate'}|{action:'prompt'}|{action:'automatic';selector:{identity:string|null;subject:Record<string,string>;issuer:Record<string,string>}};
export interface ClientCertificateRule {id:string;enabled:boolean;host:string;port:number;mode:ClientCertificateAction}
export interface ClientCertificatePolicy {default:'noCertificate'|'prompt';rules:ClientCertificateRule[]}
export interface ClientCertificatePending {id:string;host:string;port:number;caller:string;connections:number;choiceCount:number;warning:string|null}
export interface ClientCertificateStatus {supported:boolean;pending:ClientCertificatePending[]}
export interface ClientCertificateRequestDetail {pending:ClientCertificatePending;choices:ClientCertificateIdentity[];tlsVersion:string;acceptedIssuers:number;signatureAlgorithms:string[]}

export type ColumnId = 'request-number' | 'method' | 'status' | 'process' | 'pid' | 'host' | 'path' | 'url' | 'protocol' | 'duration' | 'http-duration' | 'stream-sent' | 'stream-received' | 'response-bytes' | 'request-bytes' | 'state' | 'content-type' | 'started-at';
export interface ColumnPreference { id: ColumnId; width: number; visible: boolean; pinned: boolean; }
export interface WorkspacePreferences {
  responseSetGuide: ResponseSetGuideHistory;
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
export type ResponseSetGuideHistory = 'not-offered' | 'offered' | 'started';
export interface ResponseSetGuideUpdate {history:ResponseSetGuideHistory;offered:boolean;}
export interface TrafficSort { column: ColumnId; direction: 'ascending' | 'descending'; }
export type TrafficDimension = ColumnId | 'traffic-type' | 'fetch-destination';
export type TrafficFilterValue = {kind:'text';value:string}|{kind:'number';value:string}|{kind:'status-family';value:number}|{kind:'unavailable'};
export interface TrafficFilterGroup {dimension:TrafficDimension;values:TrafficFilterValue[];}
export interface TrafficFacetOption {key:string;label:string;value:TrafficFilterValue;count:number;}
export interface TrafficFacetPage {dimension:TrafficDimension;options:TrafficFacetOption[];includedOptions:TrafficFacetOption[];matchingEntries:number;omittedEntries:number;totalOptions:number;nextOffset:number|null;}
export interface TrafficQuery {incidentWindow:unknown;search:string;searchResultId:string|null;sort:TrafficSort;fetchDestinations:string[];filters:Array<Pick<TrafficFilter,'column'|'operator'|'value'>>;filterGroups:TrafficFilterGroup[];}
export interface TrafficFilter { column: ColumnId; operator: 'contains' | 'equals' | 'minimum' | 'maximum'; value: string; label: string; }
export interface AutomationCandidate { candidateId: string; ruleCount: number; registrationCount: number; }
export interface RawHeaderField {name:number[];value:number[];}
export type HeaderOperation = {operation:'set'|'append';field:RawHeaderField}|{operation:'remove';field:string};
export interface HeaderPredicate {name:string;condition:{kind:'exists'|'exact'|'prefix'|'suffix'|'regex';value?:number[]|string};}
export interface HeaderRuleTest {test:MatchTestResult['test'];before:RawHeaderField[];after:RawHeaderField[];operations:Array<{index:number;operation:string;name:string;changed:boolean}>;}
export interface HeaderEffect {sequence:number;hookId:string;name:string;phase:string;changed:boolean;headers:{added:string[];removed:string[];changed:string[]};}
export type QueryCondition = {kind:'exact';value:string|null} | {kind:'parameters';value:Array<{name:string;value:string}>} | {kind:'ignore'};
export type UrlCondition = {kind:'exact';value:string} | {kind:'pattern';value:{address:string;query:QueryCondition;caseSensitive:boolean}} | {kind:'regex';value:{pattern:string;scope:'url'|'path';whole:boolean;caseSensitive:boolean;query:QueryCondition|null}};
export interface MatchExample {method:string;url:string;expected:boolean;headers?:Array<{name:string;value:string}>;}
export interface MatchTestResult {
  test:{matched:boolean;normalizedUrl:string;checks:Array<{label:string;matched:boolean;detail:string}>;captures:Array<{label:string;value:string}>};
  wouldServe:boolean;explanation:string;winningRule:string|null;examples:Array<{index:number;method:string;url:string;expected:boolean;matched:boolean;passed:boolean}>;
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
    responseHeaders: HeaderPredicate[];
    responseStatus: number | null;
    responseStatusClass: number | null;
  };
  request: {
    userAgent?:UserAgentOverride|null;
    headers: HeaderOperation[];
    replaceBody: number[] | null;
    discardBody: boolean;
    abortReason: string | null;
    responseAsset: string | null;
    allowNonIdempotentBodyReplacement: boolean;
  };
  response: {
    textTransform?:LiteralTransform|null;headers: HeaderOperation[]; replaceBody: number[] | null; discardBody: boolean; abortReason: string | null};
}
export interface UserAgentOverride { template:string|null; versionOverride:string|null; clientHints:Record<string,string>; stripClientHints:boolean; }
export interface AutomationStatus {
  experiments?:ResponseExperiment[];
  scriptGeneration?:number;
  activeScripts?:number;
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
  linkedBody?:{path:string;originalContentCodings:string[];applyOriginalCompression?:boolean}|null;
  copiedFrom?:string|null;
}
export type ResponseBodyFileMode='copy'|'linked';
export interface ResponseBodyFileInfo {path:string;bytes:number;error:string|null;}
export interface ResponseAssetInspection {asset:ResponseAsset;display:string;textEncoding:string|null;bytesBase64:string|null;contentCodings:string[];explanation:string;}
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
export interface ScriptRevision { manifest: Omit<ScriptDraft, 'source'> & { sourceHash: string; apiRevision: number }; source: string; }
export interface ScriptStatus {
  generation: number;
  active: ScriptRevision[];
  saved: ScriptDraft[];
  candidateCount: number;
  historyCount: number;
}
export interface ScriptCandidate { candidateId: string; scriptId: string; revision: number; sourceHash: string; }


export interface SelectedResponse { sessionId: string; detail: SessionDetail; reusable: boolean; }
export type TrafficRuleKind='request-headers'|'browser-headers'|'response-headers'|'response-text';
export interface TrafficRuleRequest {kind:TrafficRuleKind;source:SelectedResponse;}
export type ViewName = 'traffic' | 'breakpoints' | 'automation' | 'composer' | 'captures' | 'settings';
export type NoticeAction = 'setup-ca' | 'reset-ca' | 'start-proxy' | 'settings' | 'recover-proxy' | null;
export type RequestCommandFormat = 'curl' | 'powershell';
export interface RequestCommand {text:string;notices:string[];bodyFileRequired:boolean;bodyFileAvailable:boolean;}
export interface ComposerSource {method:string;url:string;headers:Array<{name:string;value:string}>;body:string;bodyAvailable:boolean;bodyStreamed?:boolean;bodyBytes?:number|null;notices:string[];}
export interface Notice { title: string; message: string; actionLabel: string | null; action: NoticeAction; }

export interface TracePasswordPrompt {title:string;confirm:boolean;message:string;resolve:(password:string|null)=>void;}
export interface OperationError {title:string;message:string;resolve:()=>void;}

export interface StreamMessage { atMillis:number; sender:string; kind:string; byteCount:number; payload:number[]; truncated:boolean; compressed:boolean; event:string|null; eventId:string|null; retry:number|null; raw:string|null; closeCode:number|null; }
export interface StreamLifetime { durationMs:number|null; establishedAt:number|null; endedAt:number|null; observedUntil:number|null; live:boolean; boundary:'live'|'final'|'open-at-capture'|'incomplete'|'never-established'; }
export interface StreamSummary { lifetime?:StreamLifetime; httpDurationMs?:number|null; responseHeadersMs?:number|null; state:{protocol:'web-socket'|'sse';status:string;atMillis:number;reason:string|null}; imported:boolean; revision:number; observed:number; evicted:number; sequenceLoss:number; retained:number; clientBytes:number|null; serverBytes:number|null; openedAt:number|null; lastActivity:number|null; }
export interface StreamRow { sequence:number; message:StreamMessage; }
export interface StreamPage { total:number; offset:number; rows:StreamRow[]; summary:StreamSummary|null; }

export interface ExperimentSelection {experimentId:string;variantId:string;revision:number;}
export interface ExperimentRevision {revision:number;rules:AutomationRule[];}
export interface ExperimentVariant {id:string;name:string;archived:boolean;revisions:ExperimentRevision[];}
export interface ResponseExperiment {id:string;name:string;archived:boolean;paused:boolean;active:ExperimentSelection|null;variants:ExperimentVariant[];undo?:{generation:number}|null;}
export interface VariantEdit {selection:ExperimentSelection;ruleId:string|null;}
export interface ResponsePreviewIntent { ruleId:string|null; sourceSessionId:string; mode:'captured'|'modified'; variant?:ExperimentSelection; }

export interface LiteralTransform {find:string;replacement:string;occurrences:'first'|'all';expectedMatches:number|null;mediaType:string;encoding:'utf-8';maxBytes:number;}
export interface BodyEditReport {outcome:string;matches:number|null;replacements:number;beforeBytes:number|null;afterBytes:number|null;beforeSha256:string|null;afterSha256:string|null;}
export interface TextTransformTest {test:MatchTestResult['test'];report:BodyEditReport|null;beforeText:string;afterText:string;}
