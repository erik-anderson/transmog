import { displayLimits } from "./view-limits.js";
import { RetainedInputs, order } from "./body-input.js";
import type { ReadTask, RetainedBody } from "./body-input.js";
import { decode } from "./decode.js";
import { fastTransferStreamType } from "./fast-transfer.js";
import type { Direction, ParsedField } from "./model.js";
import type {
  BytePage,
  ContextPage,
  HttpDecodeResult,
  Diagnostic,
  ParseStatus,
  TrafficEvent,
  Input,
  Step,
} from "transmog:extensions/v1";
import { header, notice } from "./shared.js";
import { present } from "./view.js";
import type { Presentation } from "./view.js";

import { inputLimits } from "./limits.js";
import { isInputLimit } from "./input-diagnostics.js";

type Pending =
  { token: string; kind: "bytes" | "decoded"; task: ReadTask } | { token: string; kind: "context" };

interface VisualizerState {
  readonly selected: string;
  readonly direction: Direction;
  readonly inputs: RetainedInputs;
  readonly revisions: Map<string, bigint>;
  readonly diagnostics: Diagnostic[];
  sequence: number;
  pending: Pending | null;
  historyRequested: boolean;
  historyDone: boolean;
  cursor: string | null;
  presentation?: Presentation;
}

/** A complete FastTransfer chunk can still continue an earlier stream element. */
function needsContext(field: ParsedField): boolean {
  return (
    field.issue?.kind === "missing-context" ||
    field.type === fastTransferStreamType ||
    field.children.some(needsContext)
  );
}

/** One host worker owns one session; callbacks resume its explicitly retained input. */
export class BodyVisualizer {
  private readonly current: VisualizerState;

  constructor(
    input: Input,
    private readonly epoch: number,
  ) {
    this.current = {
      selected: input.exchangeId,
      direction: input.direction,
      inputs: new RetainedInputs(input),
      revisions: new Map(),
      diagnostics: [],
      sequence: 0,
      pending: null,
      historyRequested: false,
      historyDone: false,
      cursor: null,
    };
  }

  start(): Step {
    return this.pump();
  }

  private title(): string {
    return `MAPI/HTTP ${(this.current.inputs.operation ?? "body").slice(0, displayLimits.operationName)}`;
  }

  private warning(code: string, message: string): void {
    if (!this.current.diagnostics.some((item) => item.code === code)) {
      this.current.diagnostics.push({ code, message });
    }
  }

  private problem(source: RetainedBody, code: string, message: string): void {
    source.skipped = true;
    source.issue = { code, message };
  }

  private showNotice(status: ParseStatus, message: string): Step {
    delete this.current.presentation;
    return notice(this.title(), status, message);
  }

  private diagnostics(): Diagnostic[] {
    const result = [...this.current.diagnostics];
    for (const record of this.current.inputs.records.values()) {
      for (const source of Object.values(record.sides)) {
        const issue =
          source?.issue ??
          (source?.bytes && source.reference.availability !== "complete"
            ? { code: "body-incomplete", message: "Some retained body data is incomplete." }
            : undefined);
        if (issue && !result.some((item) => item.code === issue.code)) {
          result.push(issue);
        }
      }
    }
    return result;
  }

  private beginRead(kind: "bytes" | "decoded", task: ReadTask): string {
    const current = this.current;
    const token = `m.${this.epoch}.${++current.sequence}`;
    current.pending = { kind, token, task };
    return token;
  }

  private takeRead(token: string, kind: "bytes" | "decoded"): ReadTask {
    const current = this.current;
    const pending = current.pending;
    if (
      !pending ||
      pending.token !== token ||
      pending.kind === "context" ||
      pending.kind !== kind
    ) {
      throw new Error("Expired MAPI continuation");
    }
    current.pending = null;
    return pending.task;
  }

  private pump(): Step {
    const current = this.current;
    if (!current.inputs.operation) {
      return this.showNotice("unavailable", "The request operation is unavailable or redacted.");
    }
    if (current.direction === "response") {
      const code = header(
        current.inputs.records.get(current.selected)!.input.response,
        "x-responsecode",
      );
      if (!code) {
        return this.showNotice(
          "unavailable",
          "The MAPI response status is unavailable or redacted. Inspect the retained headers and raw body.",
        );
      }
      if (code.trim() !== "0") {
        return this.showNotice(
          "partial",
          `The server returned X-ResponseCode ${code.slice(0, displayLimits.operationName)}. Inspect the retained headers and raw body for details.`,
        );
      }
    }
    for (let task = this.current.inputs.nextTask(); task; task = this.current.inputs.nextTask()) {
      const { record, side, source } = task;
      if (
        header(record.input[side], "content-type")?.split(";", 1)[0]?.trim().toLowerCase() !==
        "application/mapi-http"
      ) {
        this.problem(
          source,
          "content-type-unavailable",
          "Some paired or related content is not declared as MAPI/HTTP.",
        );
        continue;
      }
      if (side === "response" && header(record.input.response, "x-responsecode")?.trim() !== "0") {
        this.problem(
          source,
          "response-status-unavailable",
          "Some related MAPI response data has an unavailable or unsuccessful protocol status.",
        );
        continue;
      }
      if (!source.httpDecoded) {
        const encoding = header(record.input[side], "content-encoding");
        if (encoding && encoding.trim().toLowerCase() !== "identity") {
          return {
            kind: "needDecoded",
            sourceId: source.reference.sourceId,
            token: this.beginRead("decoded", task),
          };
        }
        if (
          (record.input[side]?.headers ?? []).some(
            (field) =>
              field.name.toLowerCase() === "content-encoding" && (field.redacted || !field.value),
          )
        ) {
          this.problem(
            source,
            "http-coding-unavailable",
            "A content-encoding header is unavailable or redacted. Encoded bytes were not interpreted as MAPI.",
          );
          continue;
        }
        source.httpDecoded = true;
      }
      const length = BigInt(source.reference.length);
      if (
        length - BigInt(source.offset) >
        BigInt(inputLimits.snapshotBytes - current.inputs.bytes)
      ) {
        this.problem(
          source,
          "input-limit",
          "Retained input exceeds the 8 MiB MAPI snapshot limit.",
        );
        continue;
      }
      if (length === 0n) {
        source.bytes = [];
        continue;
      }
      if (!["complete", "capturing", "truncated"].includes(source.reference.availability)) {
        this.problem(source, "body-unavailable", "Some requested body data is unavailable.");
        continue;
      }
      return {
        kind: "needInput",
        token: this.beginRead("bytes", task),
        reads: [
          {
            sourceId: source.reference.sourceId,
            offset: String(source.offset),
            maxBytes: Math.min(inputLimits.pageBytes, Number(length) - source.offset),
          },
        ],
      };
    }
    if (current.historyRequested && !current.historyDone) {
      const remaining = inputLimits.relatedExchanges - (current.inputs.records.size - 1);
      if (remaining <= 0) {
        current.historyDone = true;
        this.contextLimit();
      } else {
        const token = `m.${this.epoch}.${++current.sequence}`;
        current.pending = { kind: "context", token };
        return {
          kind: "needContext",
          token,
          request: {
            position: "before",
            cursor: current.cursor,
            limit: Math.min(inputLimits.contextPageExchanges, remaining),
          },
        };
      }
    }
    return this.render();
  }

  private render(): Step {
    const current = this.current;
    const selected = current.inputs.records.get(current.selected)!;
    const body = selected.sides[current.direction];
    if (!body?.bytes) {
      return this.showNotice(
        body?.issue && isInputLimit(body.issue) ? "limited" : "unavailable",
        body?.issue?.message ??
          "The selected body is unavailable within the visualizer input limit.",
      );
    }
    if (body.bytes.length === 0) {
      return this.showNotice(
        body.reference.availability === "complete" ? "partial" : "unavailable",
        "The selected body has no retained bytes yet. Live updates can follow it while this visualizer remains active.",
      );
    }
    const { messages, selectedId, origins } = current.inputs.snapshot();
    // Wire issues are returned as fields; thrown authoring errors reach host diagnostics.
    const decoded = decode(messages, selectedId, current.direction);
    if (
      current.inputs.operation === "Execute" &&
      !current.historyRequested &&
      needsContext(decoded.root)
    ) {
      current.historyRequested = true;
      return this.pump();
    }
    current.presentation = present(
      decoded,
      origins,
      this.title(),
      this.diagnostics(),
      current.presentation,
      body.reference.availability === "complete",
    );
    return current.presentation.step;
  }

  resume(token: string, pages: readonly BytePage[]): Step {
    const { source } = this.takeRead(token, "bytes");
    const page = pages[0];
    if (
      pages.length !== 1 ||
      !page ||
      page.sourceId !== source.reference.sourceId ||
      page.offset !== String(source.offset) ||
      page.bytes.length === 0
    ) {
      throw new Error("Incomplete MAPI byte page");
    }
    if (
      page.bytes.length >
      Math.min(inputLimits.pageBytes, Number(source.reference.length) - source.offset)
    ) {
      throw new Error("Invalid MAPI byte page");
    }
    source.chunks.push(Array.from(page.bytes));
    source.offset += page.bytes.length;
    this.current.inputs.bytes += page.bytes.length;
    if (source.offset === Number(source.reference.length)) {
      source.bytes = [];
      for (const chunk of source.chunks) {
        for (const byte of chunk) {
          source.bytes.push(byte);
        }
      }
      source.chunks = [];
    }
    return this.pump();
  }

  resumeDecoded(token: string, result: HttpDecodeResult): Step {
    const { source } = this.takeRead(token, "decoded");
    source.httpDecoded = true;
    if (result.kind === "ready") {
      source.reference = result.decoded.body;
    } else {
      this.problem(
        source,
        `http-decoding-${result.reason}`,
        `HTTP content decoding is ${result.reason}. Encoded bytes were not interpreted as MAPI.`,
      );
    }
    return this.pump();
  }

  resumeContext(token: string, page: ContextPage): Step {
    const current = this.current;
    if (current.pending?.kind !== "context" || current.pending.token !== token) {
      throw new Error("Expired MAPI continuation");
    }
    current.pending = null;
    const anchor = order(current.inputs.records.get(current.selected)!.input);
    for (const exchange of page.exchanges) {
      if (
        order(exchange.input) < anchor &&
        header(exchange.input.request, "x-requesttype") === "Execute"
      ) {
        this.current.inputs.upsert(exchange.input);
      }
    }
    current.cursor = page.nextCursor ?? null;
    current.historyDone =
      page.status !== "more" ||
      !current.cursor ||
      current.inputs.records.size >= inputLimits.relatedExchanges + 1;
    if (page.status === "unavailable") {
      this.warning("context-unavailable", "Some earlier conversation data is unavailable.");
    } else if (
      page.status === "limited" ||
      (page.status === "more" && current.inputs.records.size >= inputLimits.relatedExchanges + 1)
    ) {
      this.contextLimit();
    }
    return this.pump();
  }

  private contextLimit(): void {
    this.warning(
      "context-limit",
      "Some earlier conversation data exceeds the retained context limit. Available selected-message fields are still shown.",
    );
  }

  expand(token: string): Step {
    if (!this.current.presentation) {
      throw new Error("No MAPI structure");
    }
    return this.current.presentation!.expand(token);
  }

  onTraffic(batch: readonly TrafficEvent[], info: { readonly missedHints: string }): Step {
    const current = this.current;
    let changed = info.missedHints !== "0";
    if (changed) {
      this.warning(
        "missed-live-data",
        "Some live updates were coalesced or missed. Open a fresh visualizer to rebuild retained context.",
      );
    }
    const anchor = order(current.inputs.records.get(current.selected)!.input);
    for (const event of batch) {
      const isSelected = event.exchangeId === current.selected;
      if (
        order(event.input) > anchor ||
        (!isSelected &&
          (current.inputs.operation !== "Execute" ||
            header(event.input.request, "x-requesttype") !== "Execute"))
      ) {
        continue;
      }
      if (
        !current.inputs.records.has(event.exchangeId) &&
        current.inputs.records.size >= inputLimits.relatedExchanges + 1
      ) {
        if (!current.diagnostics.some((item) => item.code === "live-context-limit")) {
          this.warning(
            "live-context-limit",
            "The live context window is full. Open a fresh visualizer to rebuild retained context.",
          );
          changed = true;
        }
        continue;
      }
      const revision = BigInt(event.revision);
      const previous = current.revisions.get(event.exchangeId);
      if (previous !== undefined && revision <= previous) {
        continue;
      }
      current.revisions.set(event.exchangeId, revision);
      this.current.inputs.upsert(event.input);
      changed = true;
    }
    return changed ? this.pump() : (current.presentation?.step ?? this.pump());
  }
}
