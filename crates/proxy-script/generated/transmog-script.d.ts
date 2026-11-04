// Bundled script contract. Runtime helpers are supplied only by this module.

/** Native bounded console. Values use primitive text or inert object labels;
 * getters, proxies and custom conversion are never evaluated. No format substitution.
 */
interface Console {
  log(...values: unknown[]): void;
  debug(...values: unknown[]): void;
  info(...values: unknown[]): void;
  warn(...values: unknown[]): void;
  error(...values: unknown[]): void;
  trace(...values: unknown[]): void;
}
// eslint-disable-next-line no-var -- Ambient var merges with optional editor standard libraries.
declare var console: Console;
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
  /** Complete decoded bytes. Oversized bodies fail before the callback runs. */
  export type Body = Readonly<{ bytes: Bytes }>;
  export type Handler =
    | "onRequestHead"
    | "onRequestBody"
    | "onResponseHead"
    | "onResponseBody"
    | "onResponseChunk"
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
  /** Pending decoded input, including any suffix retained from the previous call. */
  export type ResponseChunkEvent = EventBase &
    Readonly<{
      response: Response;
      body: null;
      failure: null;
      bytes: Bytes;
      /** No more input will arrive. Stays true while retained bytes are consumed. */
      end: boolean;
      /** Whether this invocation may edit response headers or status. */
      canEditHeaders: boolean;
    }>;
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
  type NeedMore = Readonly<{ action: "need-more" }>;
  type Forward = Readonly<{ action: "forward"; consumed: number }>;
  type Emit = Readonly<{
    action: "emit";
    bytes: Bytes;
    consumed?: number;
    headers?: readonly HeaderOperation[];
    status?: number;
  }>;
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
      /** Defaults to buffer if onResponseBody is exported, stream if onResponseChunk, otherwise skip. */
      body?: "skip" | "buffer" | "stream";
      /** Hold headers for initial body inspection; requires streaming mode. */
      deferHeaders?: boolean;
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
  /** pass() and edit() consume the whole window; forward() and emit() can consume a prefix. */
  export type ResponseChunkResult = Evidence &
    Readonly<{
      decision:
        | Continue
        | Abort
        | NeedMore
        | Forward
        | Edit<HeaderEdits & { bytes?: Bytes; status?: number; method?: never; url?: never }>
        | Emit;
    }>;
  export type Result =
    | RequestHeadResult
    | RequestBodyResult
    | ResponseHeadResult
    | ResponseBodyResult
    | ResponseChunkResult
    | TerminalResult;
  export type RequestHeadHandler = (event: RequestHeadEvent) => RequestHeadResult;
  export type RequestBodyHandler = (event: RequestBodyEvent) => RequestBodyResult;
  export type ResponseHeadHandler = (event: ResponseHeadEvent) => ResponseHeadResult;
  export type ResponseBodyHandler = (event: ResponseBodyEvent) => ResponseBodyResult;
  export type ResponseChunkHandler = (event: ResponseChunkEvent) => ResponseChunkResult;
  export type CompletedHandler = (event: CompletedEvent) => TerminalResult;
  export type FailedHandler = (event: FailedEvent) => TerminalResult;
  export function pass(evidence?: Evidence): TerminalResult;
  export function edit<T extends MessageEdits>(
    edits: T,
    evidence?: Evidence,
  ): Evidence & Readonly<{ decision: Readonly<{ action: "edit"; edits: Readonly<T> }> }>;
  /** Wait for additional input or EOF, retaining all pending bytes. Invalid at EOF/capacity. */
  export function needMore(evidence?: Evidence): Evidence & Readonly<{ decision: NeedMore }>;
  /** Forward and consume an unchanged input prefix, retaining the rest. Needs no body-write
   * permission. consumed must be an integer within the window, positive for nonempty input.
   */
  export function forward(
    consumed: number,
    evidence?: Evidence,
  ): Evidence & Readonly<{ decision: Forward }>;
  /** Replace pending input. consumed counts input bytes (positive for nonempty input);
   * omitting it consumes the entire window. Replacement length may differ.
   */
  export function emit(
    bytes: Bytes,
    options?: {
      consumed?: number;
      headers?: readonly HeaderOperation[];
      status?: number;
    },
    evidence?: Evidence,
  ): Evidence & Readonly<{ decision: Emit }>;
  /** Select streaming and hold headers until a result forwards or replaces input. */
  export function deferResponseHeaders(result?: ResponseHeadResult): ResponseHeadResult;
  /** Literal UTF-8 replacement across windows, retaining incomplete matches automatically.
   * Search must be nonempty. Preserves Content-Type.
   */
  export function replaceTextChunk(
    event: ResponseChunkEvent,
    search: string,
    replacement: string,
  ): Evidence & Readonly<{ decision: Continue | NeedMore | Emit }>;
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
