import { Conversation } from "./conversation.js";
import { FastTransferDecoder } from "./fast-transfer.js";
import { readHttpMessage } from "./http.js";
import { ParseBudget, ParseError } from "./model.js";
import type { DecodeResult, Direction, Exchange, MessageBody, ParsedField } from "./model.js";
import { BinaryReader } from "./reader.js";
import { RecordReader } from "./record.js";
import { RpcDecoder } from "./rpc.js";

/** One bounded snapshot, including state replay. This never owns a host handle. */
export class DecodeSession {
  readonly conversation = new Conversation();
  readonly transfer = new FastTransferDecoder();
  readonly budget: ParseBudget;
  private readonly historyBudget = new ParseBudget(50_000, 48, 1_000_000, "Related context");
  private readonly requestBudget = new ParseBudget(50_000, 48, 1_000_000, "Paired request");
  private readonly preparationIssues: ParseError[] = [];
  readonly selected: Exchange;
  readonly body: MessageBody;
  readonly payloadReaders = {
    transfer: (record: RecordReader, length: number, operation: string, handle: number): void => {
      this.transfer.read(record, length, operation, handle);
    },
    release: (handle: number): void => {
      this.transfer.release(handle);
    },
  };
  readonly rpc = new RpcDecoder(this.conversation, this.payloadReaders);

  constructor(
    private readonly exchanges: readonly Exchange[],
    readonly selectedId: number,
    readonly direction: Direction,
  ) {
    if (direction !== "request" && direction !== "response") {
      throw new Error("Invalid message direction");
    }
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
    this.selected = selected;
    this.body = body;
    this.budget = new ParseBudget(50_000, 48, 1_000_000, `Selected ${direction}`);
    this.budget.input(body.bytes.length);
  }

  prepare(includeSelectedRequest: boolean): void {
    for (const exchange of [...this.exchanges].sort((left, right) => left.id - right.id)) {
      if (exchange.id >= this.selectedId) break;
      this.conversation.beginExchange();
      if (!this.replay(exchange, "request", this.historyBudget)) break;
      if (!this.replay(exchange, "response", this.historyBudget)) break;
    }
    this.conversation.beginExchange();
    if (includeSelectedRequest) this.replay(this.selected, "request", this.requestBudget);
  }

  record(name: string, offset = 0, count?: number): RecordReader {
    const end = count === undefined ? this.body.bytes.length : offset + count;
    const reader = new BinaryReader(this.body.bytes, this.body.sourceId, this.budget, offset, end);
    return new RecordReader(reader, name);
  }

  readMessage(record: RecordReader, operation: string, direction: Direction): void {
    record.reader.budget.track(`${operation} ${direction}`, record.reader.range(), () => {
      readHttpMessage(record, operation, direction, {
        auxiliary: (record, length) => this.rpc.read(record, length, true, direction),
        rops: (record, length, side) => this.rpc.read(record, length, false, side),
      });
    });
  }

  capture(record: RecordReader, parse: () => void): DecodeResult {
    try {
      parse();
      if (record.reader.remaining !== 0) {
        throw new ParseError(
          "malformed",
          "Unexpected trailing message bytes",
          record.reader.range(record.reader.offset, record.reader.remaining),
        );
      }
    } catch (error) {
      if (!(error instanceof ParseError)) throw error;
      record.add({
        name: error.message,
        type: "Parse issue",
        range: error.range,
        children: [],
        issue: { kind: error.kind, message: error.message },
      });
      record.field.issue = { kind: error.kind, message: error.message };
    }
    for (const error of this.preparationIssues) {
      record.add({
        name: error.message,
        type: "Context parse issue",
        range: error.range,
        children: [],
        issue: { kind: error.kind, message: error.message },
      });
    }
    return this.result(record);
  }

  private replay(exchange: Exchange, side: Direction, budget: ParseBudget): boolean {
    const body = exchange[side];
    if (!body?.bytes.length) return true;
    budget.input(body.bytes.length);
    const reader = new BinaryReader(body.bytes, body.sourceId, budget);
    try {
      const record = new RecordReader(reader, `${exchange.requestType} ${side}`);
      this.readMessage(record, exchange.requestType, side);
    } catch (error) {
      if (!(error instanceof ParseError)) throw error;
      if (error.kind === "limit") {
        this.preparationIssues.push(error);
        return false;
      }
      // Keep facts read before an earlier error. Any dependency it could not
      // produce is reported at its point of use in the selected message.
    }
    return true;
  }

  private result(record: RecordReader): DecodeResult {
    const root = record.finish();
    const sources = [...this.rpc.derivedSources, ...this.transfer.derivedSources];
    const needed = new Set<string>();
    const visit = (field: ParsedField): void => {
      needed.add(field.range.sourceId);
      field.children.forEach(visit);
    };
    visit(root);
    const byId = new Map(sources.map((source) => [source.id, source]));
    const pending = [...needed];
    for (let index = 0; index < pending.length; index++) {
      for (const parent of byId.get(pending[index]!)?.parents ?? []) {
        if (!needed.has(parent.sourceId)) {
          needed.add(parent.sourceId);
          pending.push(parent.sourceId);
        }
      }
    }
    return {
      root,
      derivedSources: sources.filter((source) => needed.has(source.id)),
      budgets: [this.historyBudget, this.requestBudget, this.budget]
        .map((budget) => budget.report())
        .filter((report) => report.inputBytes > 0),
    };
  }
}
