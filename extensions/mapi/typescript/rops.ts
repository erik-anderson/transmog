import { invalidHandle } from "./wire.js";
import { readRequestTail } from "./rop-requests.js";
import { readResponseTail } from "./rop-responses.js";
import { sizedBytes, readLayout } from "./rop-values.js";
import { nested, readEnum, readFields, repeat } from "./common.js";
import type { RopRequest, Conversation } from "./conversation.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import { readNotification } from "./notifications.js";
import type { Direction } from "./model.js";

import type { RecordReader } from "./record.js";
import { requestLayouts } from "./rop-layouts.js";

export interface RopPayloadReaders {
  readonly transfer: (record: RecordReader, length: number, stream: string, handle: number) => void;
  readonly release?: (handle: number) => void;
}

const opcodes = new Map(
  Object.entries(enums.RopIdType!.values).map(([name, value]) => [value, name]),
);

/** Parses one ROP; the enclosing ROP buffer enforces forward progress. */
export function readRop(
  record: RecordReader,
  direction: Direction,
  handles: readonly number[],
  conversation: Conversation,
  payload: RopPayloadReaders,
): void {
  const opcode = readEnum(record, "RopId", "RopIdType");
  const operation = opcodes.get(opcode);
  if (!operation) {
    throw new ParseError(
      opcode === 0 ? "malformed" : "unsupported",
      `Unknown ROP 0x${opcode.toString(16)}`,
      record.reader.range(),
    );
  }
  if (direction === "request") {
    const layout = requestLayouts[operation];
    if (!layout) {
      throw new ParseError(
        "unsupported",
        `Unsupported request ROP ${operation}`,
        record.reader.range(),
      );
    }
    const logonId = record.integer("LogonId", "u8");
    const parameters = readLayout(record, layout);
    const indexName =
      "InputHandleIndex" in layout
        ? "InputHandleIndex"
        : "SourceHandleIndex" in layout
          ? "SourceHandleIndex"
          : "OutputHandleIndex";
    const handleIndex = parameters.get(indexName);
    const request: RopRequest = {
      operation,
      logonId,
      parameters,
      handle: handleIndex === undefined ? undefined : handles[handleIndex],
    };
    readRequestTail(record, operation, request, conversation, payload);
    conversation.rememberRequest(request);
    return;
  }
  if (operation === "RopPending") {
    record.integer("SessionIndex", "u16");
    return;
  }
  if (operation === "RopBackoff") {
    readFields(record, { LogonId: "u8", Duration: "u32" });
    const count = record.integer("BackoffRopCount", "u8");
    repeat(
      record,
      count,
      () =>
        nested(record, "BackoffRop", (child) =>
          readFields(child, { RopIdBackoff: "u8", Duration: "u32" }),
        ),
      5,
    );
    sizedBytes(record, "AdditionalData");
    return;
  }
  if (operation === "RopBufferTooSmall") {
    record.integer("SizeNeeded", "u16");
    record.bytes("RequestBuffers", record.reader.remaining);
    return;
  }
  if (operation === "RopNotify") {
    readNotification(record, conversation);
    return;
  }
  const index = record.integer("HandleIndex", "u8");
  const status = readEnum(record, "ReturnValue", "ErrorCodes");
  const request = conversation.requestFor(operation);
  readResponseTail(
    record,
    operation,
    status,
    request,
    conversation,
    payload,
    handles[index] ?? request?.handle ?? invalidHandle,
  );
}

export function ropName(opcode: number, direction: Direction): string {
  return `${opcodes.get(opcode) ?? "Unknown ROP"} ${direction}`;
}
