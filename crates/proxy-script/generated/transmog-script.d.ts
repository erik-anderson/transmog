// Bundled script contract. Runtime helpers are supplied only by this module.
declare module "transmog:api" {
  export type Bytes = readonly number[];
  export type Header = Readonly<{
    name: string;
    value: Bytes;
    sensitive: boolean;
    redacted: boolean;
  }>;
  export type Request = Readonly<{
    method: string;
    scheme: string;
    host: string;
    port: number;
    path: string;
    query: string | null;
    headers: readonly Header[];
  }>;
  export type Response = Readonly<{ status: number; headers: readonly Header[] }>;
  /** Complete decoded bytes. Exceeded limits fail before this event is called. */
  export type Body = Readonly<{ bytes: Bytes }>;
  export type Handler =
    | "onRequestHead"
    | "onRequestBody"
    | "onResponseHead"
    | "onResponseBody"
    | "onCompleted"
    | "onFailed";
  export type Context = Readonly<{
    exchangeId: string;
    nowUnixMs: number;
    handler: Handler;
    source: "proxy" | "sandbox";
    httpVersion: "Http1" | "Http2" | "Http3";
    startedUnixMs: number;
    /** Monotonic milliseconds since script exchange admission. */
    elapsedMs: number;
  }>;
  export type Json =
    null | boolean | number | string | readonly Json[] | { readonly [key: string]: Json };
  type EventBase = Readonly<{
    context: Context;
    request: Request;
    parameters: Readonly<Record<string, string | boolean | number>>;
    state: Readonly<Record<string, Json>>;
    localResponse: boolean;
  }>;
  export type RequestHeadEvent = EventBase &
    Readonly<{ response: null; body: null; failure: null }>;
  export type RequestBodyEvent = EventBase &
    Readonly<{ response: null; body: Body; failure: null }>;
  export type ResponseHeadEvent = EventBase &
    Readonly<{ response: Response; body: null; failure: null }>;
  export type ResponseBodyEvent = EventBase &
    Readonly<{ response: Response; body: Body; failure: null }>;
  export type CompletedEvent = EventBase &
    Readonly<{ response: Response; body: null; failure: null }>;
  /** Stable, case-sensitive terminal failure names; native descriptions are withheld. */
  export type FailureCategory =
    | "HookInitialization"
    | "HookTimedOut"
    | "HookPanicked"
    | "HookAborted(Policy)"
    | "HookAborted(Rejected)"
    | "Cancelled"
    | "Body"
    | "RequestTranslation"
    | "ResponseTranslation"
    | "Route"
    | "Upstream"
    | "Shutdown";
  export type FailedEvent = EventBase &
    Readonly<{ response: null; body: null; failure: FailureCategory }>;
  export type HeaderOperation =
    | Readonly<{ operation: "set" | "append"; name: string; value: string }>
    | Readonly<{ operation: "remove"; name: string }>;
  export type Annotation = Readonly<{
    note: string;
    tags?: readonly string[];
    severity?: "info" | "warning" | "error";
  }>;
  type Evidence = Readonly<{
    annotations?: readonly Annotation[];
    diagnostics?: readonly string[];
    state?: Readonly<Record<string, Json>>;
  }>;
  type Continue = Readonly<{ action: "continue" }>;
  type Abort = Readonly<{ action: "abort"; reason: string }>;
  type InlineResponse = Readonly<{
    action: "respond";
    response: { status: number; headers: readonly (readonly [string, string])[]; bytes: Bytes };
  }>;
  type Edit<T> = Readonly<{ action: "edit"; edits: Readonly<T> }>;
  type HeaderEdits = { headers?: readonly HeaderOperation[] };
  export type MessageEdits = HeaderEdits & {
    method?: string;
    url?: string;
    status?: number;
    bytes?: Bytes;
  };
  export type RequestHeadResult = Evidence &
    Readonly<{
      decision:
        | Continue
        | Abort
        | InlineResponse
        | { action: "respond"; response: { assetRef: string } }
        | Edit<HeaderEdits & { method?: string; url?: string; status?: never; bytes?: never }>;
    }>;
  export type RequestBodyResult = Evidence &
    Readonly<{
      decision:
        | Continue
        | Abort
        | Edit<HeaderEdits & { bytes?: Bytes; method?: never; url?: never; status?: never }>;
    }>;
  export type ResponseHeadResult = Evidence &
    Readonly<{
      decision:
        | Continue
        | Abort
        | InlineResponse
        | Edit<HeaderEdits & { status?: number; method?: never; url?: never; bytes?: never }>;
    }>;
  export type ResponseBodyResult = Evidence &
    Readonly<{
      decision:
        | Continue
        | Abort
        | Edit<HeaderEdits & { status?: number; bytes?: Bytes; method?: never; url?: never }>;
    }>;
  export type TerminalResult = Evidence & Readonly<{ decision: Continue }>;
  export type Result =
    | RequestHeadResult
    | RequestBodyResult
    | ResponseHeadResult
    | ResponseBodyResult
    | TerminalResult;
  export type RequestHeadHandler = (event: RequestHeadEvent) => RequestHeadResult;
  export type RequestBodyHandler = (event: RequestBodyEvent) => RequestBodyResult;
  export type ResponseHeadHandler = (event: ResponseHeadEvent) => ResponseHeadResult;
  export type ResponseBodyHandler = (event: ResponseBodyEvent) => ResponseBodyResult;
  export type CompletedHandler = (event: CompletedEvent) => TerminalResult;
  export type FailedHandler = (event: FailedEvent) => TerminalResult;
  export function pass(evidence?: Evidence): TerminalResult;
  export function edit<T extends MessageEdits>(
    edits: T,
    evidence?: Evidence,
  ): Evidence & Readonly<{ decision: Readonly<{ action: "edit"; edits: Readonly<T> }> }>;
  export function utf8Encode(text: string): number[];
  export function utf8Decode(bytes: Bytes, options?: { fatal?: boolean }): string;
  export function base64Encode(bytes: Bytes): string;
  export function base64Decode(text: string): number[];
  export function headerValues(headers: readonly Header[], name: string): readonly Bytes[];
  /** Returns all values separately; withheld sensitive values throw. */
  export function headerText(headers: readonly Header[], name: string): readonly string[];
  export function setHeader(name: string, value: string): HeaderOperation;
  export function appendHeader(name: string, value: string): HeaderOperation;
  export function removeHeader(name: string): HeaderOperation;
  export function requestUrl(request: Request): string;
  /** URL-encoded pairs; duplicates and order are retained. */
  export function parseQuery(query: string | null): [string, string][];
  export function formatQuery(pairs: readonly (readonly [string, string])[]): string;
  export function setQuery(query: string | null, name: string, value: string): string;
  export type Pairs = readonly (readonly [string, string])[];
  /** Raw cookie values, with duplicate names and field order preserved. */
  export function readCookies(headers: readonly Header[]): [string, string][];
  export function replaceCookies(pairs: Pairs): HeaderOperation;
  export type SetCookie = Readonly<{
    name: string;
    value: string;
    attributes?: readonly (readonly [string, string | null])[];
  }>;
  /** Separate fields and ordered attributes; ignores empty attribute segments. */
  export function readSetCookies(headers: readonly Header[]): SetCookie[];
  export function replaceSetCookies(cookies: readonly SetCookie[]): HeaderOperation[];
  /** UTF-8 application/x-www-form-urlencoded, retaining duplicate fields and order. */
  export function readForm(body: Body, headers: readonly Header[]): [string, string][];
  export function replaceForm(pairs: Pairs): RequestBodyResult & ResponseBodyResult;
  /** Strict UTF-8; rejects an explicitly incompatible charset. */
  export function readText(body: Body, headers?: readonly Header[]): string;
  export function readJson(body: Body, headers?: readonly Header[]): Json;
  export function replaceText(
    text: string,
    mediaType?: string,
  ): RequestBodyResult & ResponseBodyResult;
  export function replaceJson(value: Json): RequestBodyResult & ResponseBodyResult;
  export function respondAsset(assetRef: string): RequestHeadResult;
  export function respondText(
    status: number,
    text: string,
    headers?: readonly (readonly [string, string])[],
  ): RequestHeadResult & ResponseHeadResult;
  export function respondJson(
    status: number,
    value: Json,
    headers?: readonly (readonly [string, string])[],
  ): RequestHeadResult & ResponseHeadResult;
}
