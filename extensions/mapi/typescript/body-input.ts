import type { BodyReference, Input, Diagnostic, Message } from "transmog:extensions/v1";
import type { Direction, Exchange } from "./model.js";
import { inputLimits } from "./limits.js";
import { header } from "./shared.js";

const directions = ["request", "response"] as const;
const bodyHeaderNames: readonly string[] = ["content-type", "content-encoding", "x-responsecode"];

/** Metadata changes can require decoding the same retained bytes again. */
function bodyMetadata(message: Message | null | undefined): string {
  return JSON.stringify(
    (message?.headers ?? []).filter((field) => bodyHeaderNames.includes(field.name.toLowerCase())),
  );
}

export interface RetainedBody {
  readonly original: BodyReference;
  reference: BodyReference;
  httpDecoded: boolean;
  offset: number;
  chunks: number[][];
  bytes: number[] | null;
  skipped: boolean;
  issue?: Diagnostic;
}

export interface RetainedExchange {
  input: Input;
  readonly sides: Record<Direction, RetainedBody | null>;
}

export interface ReadTask {
  readonly record: RetainedExchange;
  readonly side: Direction;
  readonly source: RetainedBody;
}

export function order(input: Input): bigint {
  return BigInt(input.requestNumber);
}

/** Retains granted body pages and prioritizes the selected message before context. */
export class RetainedInputs {
  readonly selected: string;
  readonly direction: Direction;
  readonly records = new Map<string, RetainedExchange>();
  operation: string | null = null;
  bytes = 0;

  constructor(input: Input) {
    this.selected = input.exchangeId;
    this.direction = input.direction;
    this.upsert(input);
  }

  upsert(input: Input): void {
    let record = this.records.get(input.exchangeId);
    if (!record) {
      if (this.records.size >= inputLimits.relatedExchanges + 1) {
        return;
      }
      record = { input, sides: { request: null, response: null } };
      this.records.set(input.exchangeId, record);
    }
    for (const side of directions) {
      const body = input[side]?.body;
      const old = record.sides[side];
      if (
        old &&
        body &&
        old.original.sourceId === body.sourceId &&
        old.original.length === body.length &&
        old.original.availability === body.availability &&
        bodyMetadata(record.input[side]) === bodyMetadata(input[side])
      ) {
        continue;
      }
      if (old) {
        this.bytes -= old.offset;
      }
      record.sides[side] = body
        ? {
            original: body,
            reference: body,
            httpDecoded: false,
            offset: 0,
            chunks: [],
            bytes: null,
            skipped: false,
          }
        : null;
    }
    record.input = input;
    if (input.exchangeId === this.selected) {
      this.operation = header(input.request, "x-requesttype");
    }
  }

  orderedRecords(): RetainedExchange[] {
    return [...this.records.values()].sort((left, right) =>
      order(left.input) < order(right.input) ? -1 : 1,
    );
  }

  nextTask(): ReadTask | undefined {
    const selected = this.records.get(this.selected)!;
    const others = this.orderedRecords().filter((record) => record !== selected);
    for (const record of [selected, ...others]) {
      const sides: readonly Direction[] = [
        this.direction,
        this.direction === "request" ? "response" : "request",
      ];
      for (const side of sides) {
        if (
          record === selected &&
          side !== this.direction &&
          (this.operation !== "Execute" || this.direction === "request")
        ) {
          continue;
        }
        const source = record.sides[side];
        if (source && !source.skipped && !source.bytes) {
          return { record, side, source };
        }
      }
    }
    return undefined;
  }

  snapshot(): { messages: Exchange[]; selectedId: number; origins: [string, number][] } {
    const selected = this.records.get(this.selected)!;
    const ordered = this.orderedRecords();
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
        if (source?.bytes) {
          message[side] = { sourceId: source.reference.sourceId, bytes: source.bytes };
        }
      }
      return message;
    });
    const origins: [string, number][] = [];
    for (const record of ordered) {
      for (const side of directions) {
        const source = record.sides[side];
        if (source?.bytes) {
          origins.push([source.reference.sourceId, source.bytes.length]);
        }
      }
    }
    return { messages, selectedId: ordered.indexOf(selected) + 1, origins };
  }
}
