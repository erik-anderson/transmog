/** Optional authoring types for API 1. Runtime packages contain JavaScript ES2022 modules.
 * Use import type (or JSDoc import types); this declaration module has no runtime implementation.
 * Visualizers are read-only, synchronous and executed in an OS-isolated worker.
 */
declare module "transmog:extensions/v1" {
  export type Direction = "request" | "response";
  export type BodyAccess = "none" | "optional" | "required";
  export type Presentation = "tree" | "table" | "text" | "hex";
  export type ParseStatus = "complete" | "partial" | "malformed" | "unavailable" | "limited";
  export interface Span { sourceId: string; offset: string; length: string; }
  export interface Format { protocol?: string; charset?: string; byteOrder?: string; radix?: 2 | 10 | 16; encoding?: string; units?: string; }
  export type Value =
    | {kind: "string" | "integer" | "timestamp" | "guid"; value: string}
    | {kind: "float"; value: number}
    | {kind: "boolean"; value: boolean}
    | {kind: "enum"; value: {name: string; raw: string}}
    | {kind: "flags"; value: {names: string[]; raw: string}}
    | {kind: "bytes" | "object" | "array"};
  export interface Node { id: string; name: string; value?: Value; format?: Format; spans?: Span[]; children?: Node[]; childrenToken?: string; presentation?: Presentation; }
  export interface DerivedSource { id: string; name: string; bytesBase64: string; parents: Span[]; transform: string; exactMapping: boolean; }
  export interface Diagnostic { code: string; message: string; nodeId?: string; }
  export interface VisualizationDocument { title: string; status: ParseStatus; presentation: Presentation; summary?: string; roots: Node[]; sources?: DerivedSource[]; diagnostics?: Diagnostic[]; }
  export interface Header { sourceId?: string; index: number; name: string; value?: readonly number[] | null; redacted: boolean; originalBytes?: string | null; }
  export interface BodyReference { sourceId: string; length: string; availability: string; }
  export interface Message { evidenceId: string; headers: readonly Header[]; body?: BodyReference | null; method?: string | null; path?: string | null; status?: number | null; }
  export interface Input { instanceId: string; exchangeId: string; requestNumber: string; generation: string; direction: Direction; request?: Message | null; response?: Message | null; }
  export interface ReadRequest { sourceId: string; offset: string; maxBytes: number; }
  /** Related reads cannot select an origin, capture or exchange ID. Cursors are instance-scoped and single-use. */
  export interface ContextRequest {position: "before" | "after"; cursor?: string | null; limit: number;}
  export interface ContextExchange {order: string; input: Input;}
  export interface ContextPage {anchorOrder: string; exchanges: readonly ContextExchange[]; nextCursor?: string | null; status: "complete" | "more" | "unavailable" | "limited";}
  export type Step = {kind: "document"; document: VisualizationDocument} | {kind: "needInput"; reads: ReadRequest[]; token: string} | {kind: "needContext"; request: ContextRequest; token: string};
  export interface BytePage { sourceId: string; offset: string; bytes: readonly number[]; }
  export interface TrafficEvent { sequence: string; exchangeId: string; direction: Direction; phase: "head" | "body" | "bodyCompleted" | "ended"; revision: string; input: Input; }
  export interface TrafficBatchInfo {missedHints: string;}
  export function visualize(input: Input): Step;
  export function resume(token: string, pages: readonly BytePage[]): Step;
  /** Required when returning needContext; receives only host-filtered, declared input. */
  export function resumeContext(token: string, page: ContextPage): Step;
  export function expand(token: string): Step;
  export function onTraffic(batch: readonly TrafficEvent[], info: TrafficBatchInfo): Step;
}
