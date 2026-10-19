/** Version 1: read-only, synchronous visualizers executed in an OS-isolated worker. */
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
  export interface Header { index: number; name: string; value?: readonly number[]; redacted: boolean; originalBytes?: string; }
  export interface BodyReference { sourceId: string; length: string; availability: string; }
  export interface Message { headers: readonly Header[]; body?: BodyReference; method?: string; path?: string; status?: number; }
  export interface Input { instanceId: string; generation: string; direction: Direction; request?: Message; response?: Message; }
  export interface ReadRequest { sourceId: string; offset: string; maxBytes: number; }
  export type Step = {kind: "document"; document: VisualizationDocument} | {kind: "needInput"; reads: ReadRequest[]; token: string};
  export interface BytePage { sourceId: string; offset: string; bytes: readonly number[]; }
  export interface TrafficEvent { sequence: string; exchangeId: string; direction: Direction; revision: string; input: Input; }
  export function visualize(input: Input): Step;
  export function resume(token: string, pages: readonly BytePage[]): Step;
  export function expand(token: string): Step;
  export function onTraffic(batch: readonly TrafficEvent[]): Step;
}
