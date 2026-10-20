import { readHttpMessage } from "./http.js";
import { ParseBudget, ParseError } from "./model.js";
import type { DecodeResult, Direction, Exchange, ParsedField } from "./model.js";
import { parsePropertyFixture } from "./properties.js";
import { BinaryReader } from "./reader.js";
import { RecordReader } from "./record.js";

export interface DecodeOptions {
  readonly type?: string;
  readonly arguments?: readonly unknown[];
  readonly offset?: number;
  readonly count?: number;
  readonly prepareRequest?: boolean;
}

function unavailable(record: RecordReader, feature: string): never {
  throw new ParseError("unsupported", `${feature} parser is not registered`, record.reader.range());
}

/** A decode owns its readers and context. No state survives this call. */
export function decode(
  exchanges: readonly Exchange[],
  selectedId: number,
  direction: Direction,
  options: DecodeOptions = {},
): DecodeResult {
  if (exchanges.length === 0 || exchanges.length > 257) {
    throw new Error("Context limit");
  }
  const identities = new Set<number>();
  let inputBytes = 0;
  for (const exchange of exchanges) {
    if (!Number.isSafeInteger(exchange.id) || exchange.id < 1 || identities.has(exchange.id)) {
      throw new Error("Invalid message identity");
    }
    identities.add(exchange.id);
    inputBytes += (exchange.request?.bytes.length ?? 0) + (exchange.response?.bytes.length ?? 0);
    if (inputBytes > 8 * 1024 * 1024) {
      throw new Error("Input limit");
    }
  }
  const selected = exchanges.find((exchange) => exchange.id === selectedId);
  const body = selected?.[direction];
  if (!selected || !body) {
    throw new Error("Missing selected message");
  }
  const budget = new ParseBudget();
  const offset = options.offset ?? 0;
  const end = options.count ? offset + options.count : body.bytes.length;
  const reader = new BinaryReader(body.bytes, body.sourceId, budget, offset, end);
  const type = options.type?.split(".").at(-1);
  const record = new RecordReader(reader, type ?? `${selected.requestType} ${direction}`);
  try {
    if (type) {
      if (!parsePropertyFixture(record, type, options.arguments ?? [])) {
        unavailable(record, type);
      }
    } else {
      readHttpMessage(record, selected.requestType, direction, {
        auxiliary: (record) => unavailable(record, "Auxiliary buffer"),
        rops: (record) => unavailable(record, "ROP buffer"),
      });
    }
    if (reader.remaining !== 0) {
      throw new ParseError("malformed", "Unexpected trailing message bytes", reader.range(reader.offset, reader.remaining));
    }
  } catch (error) {
    if (!(error instanceof ParseError)) {
      throw error;
    }
    const issue: ParsedField = {
      name: error.message,
      type: "Parse issue",
      range: error.range,
      children: [],
      issue: { kind: error.kind, message: error.message },
    };
    record.add(issue);
    record.field.issue = { kind: error.kind, message: error.message };
  }
  return { root: record.finish(), derivedSources: [] };
}
