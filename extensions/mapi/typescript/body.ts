import { decode } from "./decode.js";
import type { Direction, Exchange, ParsedField } from "./model.js";
import { ParseError } from "./model.js";
import type {
  BodyReference,
  BytePage,
  ContextPage,
  DecodeContinuation,
  Diagnostic,
  ParseStatus,
  TrafficEvent,
  VisualizationInput,
  VisualizationStep,
} from "./host.js";
import { header, notice } from "./shared.js";
import { present } from "./view.js";
import type { Presentation } from "./view.js";

const maximumBytes = 8 * 1024 * 1024;
const maximumContext = 64;
const directions = ["request", "response"] as const;

interface RetainedBody {
  readonly original: BodyReference;
  reference: BodyReference;
  decoded: boolean;
  offset: number;
  chunks: number[][];
  bytes: number[] | null;
  skipped: boolean;
  issue?: Diagnostic;
}

interface RetainedExchange {
  input: VisualizationInput;
  readonly sides: Record<Direction, RetainedBody | null>;
}

interface ReadTask {
  readonly record: RetainedExchange;
  readonly side: Direction;
  readonly source: RetainedBody;
}

type Pending =
  | { token: string; kind: "bytes" | "decoded"; task: ReadTask }
  | { token: string; kind: "context" };

interface VisualizerState {
  readonly selected: string;
  readonly direction: Direction;
  operation: string | null;
  readonly records: Map<string, RetainedExchange>;
  readonly revisions: Map<string, bigint>;
  readonly diagnostics: Diagnostic[];
  sequence: number;
  pending: Pending | null;
  bytes: number;
  historyRequested: boolean;
  historyDone: boolean;
  cursor: string | null;
  presentation?: Presentation;
}

let active: VisualizerState | undefined;
let epoch = 0;

function state(): VisualizerState {
  if (!active) throw new Error("No active MAPI visualizer");
  return active;
}

function order(input: VisualizationInput): bigint {
  return BigInt(input.requestNumber);
}

function title(): string {
  return `MAPI/HTTP ${(state().operation ?? "body").slice(0, 64)}`;
}

function warning(code: string, message: string): void {
  if (!state().diagnostics.some((item) => item.code === code))
    state().diagnostics.push({ code, message });
}

function problem(source: RetainedBody, code: string, message: string): void {
  source.skipped = true;
  source.issue = { code, message };
}

function showNotice(status: ParseStatus, message: string): VisualizationStep {
  delete state().presentation;
  return notice(title(), status, message);
}

function diagnostics(): Diagnostic[] {
  const result = [...state().diagnostics];
  for (const record of state().records.values()) {
    for (const source of Object.values(record.sides)) {
      const issue =
        source?.issue ??
        (source?.bytes && source.reference.availability !== "complete"
          ? { code: "body-incomplete", message: "Some retained body data is incomplete." }
          : undefined);
      if (issue && !result.some((item) => item.code === issue.code)) result.push(issue);
    }
  }
  return result;
}

function upsert(input: VisualizationInput): void {
  const current = state();
  let record = current.records.get(input.exchangeId);
  if (!record) {
    if (current.records.size >= maximumContext + 1) return;
    record = { input, sides: { request: null, response: null } };
    current.records.set(input.exchangeId, record);
  }
  for (const side of directions) {
    const body = input[side]?.body;
    const old = record.sides[side];
    const metadata = (value: VisualizationInput): string =>
      JSON.stringify(
        (value[side]?.headers ?? []).filter((field) =>
          ["content-type", "content-encoding", "x-responsecode"].includes(field.name.toLowerCase()),
        ),
      );
    if (
      old &&
      body &&
      old.original.sourceId === body.sourceId &&
      old.original.length === body.length &&
      old.original.availability === body.availability &&
      metadata(record.input) === metadata(input)
    )
      continue;
    if (old) current.bytes -= old.offset;
    record.sides[side] = body
      ? {
          original: body,
          reference: body,
          decoded: false,
          offset: 0,
          chunks: [],
          bytes: null,
          skipped: false,
        }
      : null;
  }
  record.input = input;
  if (input.exchangeId === current.selected)
    current.operation = header(input.request, "x-requesttype");
}

function orderedRecords(): RetainedExchange[] {
  return [...state().records.values()].sort((left, right) =>
    order(left.input) < order(right.input) ? -1 : 1,
  );
}

function nextTask(): ReadTask | undefined {
  const current = state();
  const selected = current.records.get(current.selected)!;
  const others = orderedRecords().filter((record) => record !== selected);
  for (const record of [selected, ...others]) {
    const sides: readonly Direction[] = [
      current.direction,
      current.direction === "request" ? "response" : "request",
    ];
    for (const side of sides) {
      if (
        record === selected &&
        side !== current.direction &&
        (current.operation !== "Execute" || current.direction === "request")
      )
        continue;
      const source = record.sides[side];
      if (source && !source.skipped && !source.bytes) return { record, side, source };
    }
  }
  return undefined;
}

function beginRead(kind: "bytes" | "decoded", task: ReadTask): string {
  const current = state();
  const token = `m.${epoch}.${++current.sequence}`;
  current.pending = { kind, token, task };
  return token;
}

function takeRead(token: string, kind: "bytes" | "decoded"): ReadTask {
  const current = state();
  const pending = current.pending;
  if (!pending || pending.token !== token || pending.kind === "context" || pending.kind !== kind) {
    throw new Error("Expired MAPI continuation");
  }
  current.pending = null;
  return pending.task;
}

function pump(): VisualizationStep {
  const current = state();
  if (!current.operation)
    return showNotice("unavailable", "The request operation is unavailable or redacted.");
  if (current.direction === "response") {
    const code = header(current.records.get(current.selected)!.input.response, "x-responsecode");
    if (!code)
      return showNotice(
        "unavailable",
        "The MAPI response status is unavailable or redacted. Inspect the retained headers and raw body.",
      );
    if (code.trim() !== "0")
      return showNotice(
        "partial",
        `The server returned X-ResponseCode ${code.slice(0, 64)}. Inspect the retained headers and raw body for details.`,
      );
  }
  for (let task = nextTask(); task; task = nextTask()) {
    const { record, side, source } = task;
    if (
      header(record.input[side], "content-type")?.split(";", 1)[0]?.trim().toLowerCase() !==
      "application/mapi-http"
    ) {
      problem(
        source,
        "content-type-unavailable",
        "Some paired or related content is not declared as MAPI/HTTP.",
      );
      continue;
    }
    if (side === "response" && header(record.input.response, "x-responsecode")?.trim() !== "0") {
      problem(
        source,
        "response-status-unavailable",
        "Some related MAPI response data has an unavailable or unsuccessful protocol status.",
      );
      continue;
    }
    if (!source.decoded) {
      const encoding = header(record.input[side], "content-encoding");
      if (encoding && encoding.trim().toLowerCase() !== "identity") {
        return {
          kind: "needDecoded",
          sourceId: source.reference.sourceId,
          token: beginRead("decoded", task),
        };
      }
      if (
        (record.input[side]?.headers ?? []).some(
          (field) =>
            field.name.toLowerCase() === "content-encoding" && (field.redacted || !field.value),
        )
      ) {
        problem(
          source,
          "http-coding-unavailable",
          "A content-encoding header is unavailable or redacted. Encoded bytes were not interpreted as MAPI.",
        );
        continue;
      }
      source.decoded = true;
    }
    const length = BigInt(source.reference.length);
    if (length - BigInt(source.offset) > BigInt(maximumBytes - current.bytes)) {
      problem(source, "input-limit", "Retained input exceeds the 8 MiB MAPI snapshot limit.");
      continue;
    }
    if (length === 0n) {
      source.bytes = [];
      continue;
    }
    if (!["complete", "capturing", "truncated"].includes(source.reference.availability)) {
      problem(source, "body-unavailable", "Some requested body data is unavailable.");
      continue;
    }
    return {
      kind: "needInput",
      token: beginRead("bytes", task),
      reads: [
        {
          sourceId: source.reference.sourceId,
          offset: String(source.offset),
          maxBytes: Math.min(65_536, Number(length) - source.offset),
        },
      ],
    };
  }
  if (current.historyRequested && !current.historyDone) {
    const token = `m.${epoch}.${++current.sequence}`;
    current.pending = { kind: "context", token };
    return {
      kind: "needContext",
      token,
      request: {
        position: "before",
        cursor: current.cursor,
        limit: Math.min(32, maximumContext - (current.records.size - 1)),
      },
    };
  }
  return render();
}

function needsContext(field: ParsedField): boolean {
  return (
    field.issue?.kind === "missing-context" ||
    /^Rop(?:FastTransfer|WritePerUserInformation|SetMessageReadFlag)/.test(field.type) ||
    field.children.some(needsContext)
  );
}

function render(): VisualizationStep {
  const current = state();
  const selected = current.records.get(current.selected)!;
  const body = selected.sides[current.direction];
  if (!body?.bytes)
    return showNotice(
      body?.issue?.code === "input-limit" ? "limited" : "unavailable",
      body?.issue?.message ?? "The selected body is unavailable within the visualizer input limit.",
    );
  if (body.bytes.length === 0)
    return showNotice(
      body.reference.availability === "complete" ? "partial" : "unavailable",
      "The selected body has no retained bytes yet. Live updates can follow it while this visualizer remains active.",
    );
  const ordered = orderedRecords();
  const messages: Exchange[] = ordered.map((record, index) => {
    const message: {
      id: number;
      requestType: string;
      request?: { sourceId: string; bytes: readonly number[] };
      response?: { sourceId: string; bytes: readonly number[] };
    } = {
      id: index + 1,
      requestType: header(record.input.request, "x-requesttype") ?? "",
    };
    for (const side of directions) {
      const source = record.sides[side];
      if (source?.bytes)
        message[side] = { sourceId: source.reference.sourceId, bytes: source.bytes };
    }
    return message;
  });
  let decoded;
  try {
    decoded = decode(messages, ordered.indexOf(selected) + 1, current.direction);
  } catch (error) {
    if (error instanceof ParseError && error.kind === "limit")
      return showNotice("limited", error.message);
    if (current.operation === "Execute" && !current.historyRequested) {
      current.historyRequested = true;
      return pump();
    }
    return showNotice(
      "partial",
      "MAPI decoding could not complete with the bounded retained input. Inspect the raw body or open a fresh visualizer after more traffic is retained.",
    );
  }
  if (current.operation === "Execute" && !current.historyRequested && needsContext(decoded.root)) {
    current.historyRequested = true;
    return pump();
  }
  const origins: [string, number][] = [];
  for (const record of ordered)
    for (const side of directions) {
      const source = record.sides[side];
      if (source?.bytes) origins.push([source.reference.sourceId, source.bytes.length]);
    }
  current.presentation = present(
    decoded,
    origins,
    title(),
    diagnostics(),
    current.presentation,
    body.reference.availability === "complete",
  );
  return current.presentation.step;
}

export function visualize(input: VisualizationInput): VisualizationStep {
  epoch++;
  active = {
    selected: input.exchangeId,
    direction: input.direction,
    operation: header(input.request, "x-requesttype"),
    records: new Map(),
    revisions: new Map(),
    diagnostics: [],
    sequence: 0,
    pending: null,
    bytes: 0,
    historyRequested: false,
    historyDone: false,
    cursor: null,
  };
  upsert(input);
  return pump();
}

export function resume(token: string, pages: readonly BytePage[]): VisualizationStep {
  const { source } = takeRead(token, "bytes");
  const page = pages[0];
  if (
    pages.length !== 1 ||
    !page ||
    page.sourceId !== source.reference.sourceId ||
    page.offset !== String(source.offset) ||
    page.bytes.length === 0
  )
    throw new Error("Incomplete MAPI byte page");
  if (page.bytes.length > Math.min(65_536, Number(source.reference.length) - source.offset))
    throw new Error("Invalid MAPI byte page");
  source.chunks.push(Array.from(page.bytes));
  source.offset += page.bytes.length;
  state().bytes += page.bytes.length;
  if (source.offset === Number(source.reference.length)) {
    source.bytes = [];
    for (const chunk of source.chunks) for (const byte of chunk) source.bytes.push(byte);
    source.chunks = [];
  }
  return pump();
}

export function resumeDecoded(token: string, result: DecodeContinuation): VisualizationStep {
  const { source } = takeRead(token, "decoded");
  source.decoded = true;
  if (result.kind === "ready") source.reference = result.decoded.body;
  else
    problem(
      source,
      `http-decoding-${result.reason}`,
      `HTTP content decoding is ${result.reason}. Encoded bytes were not interpreted as MAPI.`,
    );
  return pump();
}

export function resumeContext(token: string, page: ContextPage): VisualizationStep {
  const current = state();
  if (current.pending?.kind !== "context" || current.pending.token !== token)
    throw new Error("Expired MAPI continuation");
  current.pending = null;
  const anchor = order(current.records.get(current.selected)!.input);
  for (const exchange of page.exchanges) {
    if (
      order(exchange.input) < anchor &&
      header(exchange.input.request, "x-requesttype") === "Execute"
    )
      upsert(exchange.input);
  }
  current.cursor = page.nextCursor ?? null;
  current.historyDone =
    page.status !== "more" || !current.cursor || current.records.size >= maximumContext + 1;
  if (
    page.status === "unavailable" ||
    page.status === "limited" ||
    (page.status === "more" && current.records.size >= maximumContext + 1)
  ) {
    warning(
      `context-${page.status}`,
      "Some earlier conversation data is unavailable within the retained context limit.",
    );
  }
  return pump();
}

export function expand(token: string): VisualizationStep {
  if (!state().presentation) throw new Error("No MAPI structure");
  return state().presentation!.expand(token);
}

export function onTraffic(
  batch: readonly TrafficEvent[],
  info: { readonly missedHints: string },
): VisualizationStep {
  const current = state();
  let changed = info.missedHints !== "0";
  if (changed)
    warning(
      "missed-live-data",
      "Some live updates were coalesced or missed. Open a fresh visualizer to rebuild retained context.",
    );
  const anchor = order(current.records.get(current.selected)!.input);
  for (const event of batch) {
    if (
      order(event.input) > anchor ||
      (header(event.input.request, "x-requesttype") !== "Execute" &&
        event.exchangeId !== current.selected)
    )
      continue;
    if (!current.records.has(event.exchangeId) && current.records.size >= maximumContext + 1) {
      if (!current.diagnostics.some((item) => item.code === "live-context-limit")) {
        warning(
          "live-context-limit",
          "The live context window is full. Open a fresh visualizer to rebuild retained context.",
        );
        changed = true;
      }
      continue;
    }
    const revision = BigInt(event.revision);
    const previous = current.revisions.get(event.exchangeId);
    if (previous !== undefined && revision <= previous) continue;
    current.revisions.set(event.exchangeId, revision);
    upsert(event.input);
    changed = true;
  }
  return changed ? pump() : (current.presentation?.step ?? pump());
}
