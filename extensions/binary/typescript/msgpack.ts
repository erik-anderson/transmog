import type { Node, Value } from "transmog:extensions/v1";
import type { Reader } from "./reader.js";
import { ParseError, integer } from "./reader.js";
import {
  count,
  entryName,
  finishNode,
  float,
  literal,
  mapEntry,
  parseValues,
  retainPartial,
  string,
  unsigned,
} from "./values.js";

function read(reader: Reader, name: string, depth: number): Node {
  reader.budget.depth(depth, reader.offset);
  const start = reader.offset;
  const tag = reader.byte();
  let value: Value;
  const children: Node[] = [];
  let length = 0;
  let kind = "";
  if (tag < 0x80) {
    value = integer(tag);
  } else if (tag >= 0xe0) {
    value = integer(tag - 256);
  } else if (tag >= 0xa0 && tag <= 0xbf) {
    value = string(reader, tag & 31);
  } else if (tag >= 0x90 && tag <= 0x9f) {
    kind = "array";
    length = tag & 15;
    value = { kind: "array" };
  } else if (tag >= 0x80 && tag <= 0x8f) {
    kind = "map";
    length = tag & 15;
    value = { kind: "object" };
  } else if (tag === 0xc0) {
    value = literal("null");
  } else if (tag === 0xc2 || tag === 0xc3) {
    value = { kind: "boolean", value: tag === 0xc3 };
  } else if (tag === 0xca || tag === 0xcb) {
    value = float(reader, tag === 0xca ? 4 : 8, true);
  } else if (tag >= 0xcc && tag <= 0xd3) {
    const width = 2 ** ((tag - 0xcc) & 3);
    const raw = unsigned(reader, width, true);
    value = integer(tag >= 0xd0 ? BigInt.asIntN(width * 8, raw) : raw);
  } else if (tag >= 0xd9 && tag <= 0xdb) {
    value = string(reader, count(unsigned(reader, 2 ** (tag - 0xd9), true), reader));
  } else if (tag >= 0xc4 && tag <= 0xc6) {
    length = count(unsigned(reader, 2 ** (tag - 0xc4), true), reader);
    const payloadStart = reader.offset;
    reader.take(length);
    children.push(reader.node("Binary payload", payloadStart, { kind: "bytes" }));
    value = literal("Binary", 0);
  } else if (tag >= 0xdc && tag <= 0xdf) {
    kind = tag <= 0xdd ? "array" : "map";
    length = Number(unsigned(reader, tag % 2 === 0 ? 2 : 4, true));
    value = { kind: kind === "array" ? "array" : "object" };
  } else if ((tag >= 0xc7 && tag <= 0xc9) || (tag >= 0xd4 && tag <= 0xd8)) {
    length =
      tag >= 0xd4 ? 2 ** (tag - 0xd4) : count(unsigned(reader, 2 ** (tag - 0xc7), true), reader);
    const type = Number(BigInt.asIntN(8, BigInt(reader.byte())));
    const payload = reader.take(length);
    const payloadStart = payload.offset;
    if (type === -1) {
      let seconds: bigint,
        nanos = 0n;
      if (length === 4) {
        seconds = unsigned(payload, 4, true);
      } else if (length === 8) {
        const bits = unsigned(payload, 8, true);
        nanos = bits >> 34n;
        seconds = bits & ((1n << 34n) - 1n);
      } else if (length === 12) {
        nanos = unsigned(payload, 4, true);
        seconds = BigInt.asIntN(64, unsigned(payload, 8, true));
      } else {
        throw new ParseError("malformed", "Invalid MessagePack timestamp length", start);
      }
      if (nanos >= 1_000_000_000n) {
        throw new ParseError("malformed", "Invalid timestamp nanoseconds", start);
      }
      value = { kind: "timestamp", value: `${seconds} seconds + ${nanos} ns since Unix epoch` };
    } else {
      value = literal(`Extension ${type} (opaque)`, type);
      payload.offset = payload.end;
      children.push(payload.node("Extension payload", payloadStart, { kind: "bytes" }));
    }
  } else {
    throw new ParseError("malformed", "Reserved or invalid MessagePack tag", start);
  }
  if (kind) {
    try {
      for (let index = 0; index < length; index++) {
        if (kind === "array") {
          children.push(read(reader, `Item ${index + 1}`, depth + 1));
        } else {
          const entryStart = reader.offset;
          const key = read(reader, "Key", depth + 1);
          children.push(
            mapEntry(reader, entryName(key, index), entryStart, key, () =>
              read(reader, "Value", depth + 1),
            ),
          );
        }
      }
    } catch (error) {
      retainPartial(error, reader, name, start, children, kind === "array");
    }
  }
  return finishNode(reader, name, start, value, children);
}

export function parse(bytes: Uint8Array, sourceId: string) {
  return parseValues(
    bytes,
    sourceId,
    "MessagePack",
    "Encoded values and map keys; array positions do not establish application field names. Unknown extension types remain opaque. Consecutive values are shown in wire order.",
    read,
  );
}
