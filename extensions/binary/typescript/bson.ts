import type { Node, Value } from "transmog:extensions/v1";
import { Reader, ParseError, integer, text } from "./reader.js";
import {
  float,
  finishNode,
  hex,
  literal,
  mapEntry,
  parseValues,
  retainPartial,
  string,
  unsigned,
} from "./values.js";

function length(reader: Reader): number {
  const start = reader.offset,
    size = Number(unsigned(reader, 4));
  if (size > 0x7fffffff) {
    throw new ParseError("malformed", "Negative BSON length", start);
  }
  return size;
}

function cstring(reader: Reader): { value: string; clipped: boolean } {
  const start = reader.offset;
  while (reader.byte() !== 0) {
    /* The bounded reader enforces the terminator. */
  }
  return text(new Reader(reader.bytes, reader.sourceId, reader.budget, start, reader.offset - 1));
}

function bsonString(reader: Reader): Value {
  const start = reader.offset,
    size = length(reader);
  if (!size) {
    throw new ParseError("malformed", "BSON string must include a terminator", start);
  }
  const result = string(reader, size - 1);
  if (reader.byte() !== 0) {
    throw new ParseError("malformed", "Missing BSON string terminator", reader.offset - 1);
  }
  return result;
}

const types: Record<number, string> = {
  1: "double",
  2: "string",
  3: "document",
  4: "array",
  5: "binary",
  6: "undefined (deprecated)",
  7: "ObjectId",
  8: "Boolean",
  9: "UTC datetime",
  10: "null",
  11: "regular expression",
  12: "DBPointer (deprecated)",
  13: "JavaScript source (inert)",
  14: "symbol (deprecated)",
  15: "JavaScript with scope (inert, deprecated)",
  16: "int32",
  17: "timestamp",
  18: "int64",
  19: "Decimal128 (raw bits)",
  0x7f: "MaxKey",
  0xff: "MinKey",
};

function field(reader: Reader, type: number, name: string, depth: number): Node {
  const start = reader.offset,
    children: Node[] = [];
  let value: Value;
  if (type === 1) {
    value = float(reader, 8);
  } else if ([2, 13, 14].includes(type)) {
    value = bsonString(reader);
  } else if (type === 3 || type === 4) {
    return readDocument(reader, name, depth, type === 4);
  } else if (type === 5) {
    let size = length(reader);
    const subtype = reader.byte();
    if (subtype === 2) {
      const inner = length(reader);
      if (size < 4 || inner !== size - 4) {
        throw new ParseError("malformed", "Invalid old BSON binary length", start);
      }
      size = inner;
    }
    const payloadStart = reader.offset;
    reader.take(size);
    children.push(
      reader.node(`Binary subtype ${subtype} payload`, payloadStart, { kind: "bytes" }),
    );
    value = literal(`Binary subtype ${subtype}`, subtype);
  } else if (type === 7) {
    value = { kind: "string", value: hex(reader, 12) };
  } else if (type === 8) {
    const raw = reader.byte();
    if (raw > 1) {
      throw new ParseError("malformed", "Invalid BSON Boolean", start);
    }
    value = { kind: "boolean", value: raw === 1 };
  } else if (type === 9) {
    value = {
      kind: "timestamp",
      value: `${BigInt.asIntN(64, unsigned(reader, 8))} ms since Unix epoch`,
    };
  } else if (type === 11) {
    const pattern = cstring(reader);
    children.push(
      reader.node("Pattern (inert)", start, {
        kind: "string",
        value: pattern.value + (pattern.clipped ? "… [preview]" : ""),
      }),
    );
    const optionsStart = reader.offset;
    const options = cstring(reader);
    children.push(
      reader.node("Options", optionsStart, {
        kind: "string",
        value: options.value + (options.clipped ? "… [preview]" : ""),
      }),
    );
    value = { kind: "object" };
  } else if (type === 12) {
    const namespace = bsonString(reader);
    children.push(reader.node("Namespace", start, namespace));
    const objectStart = reader.offset;
    children.push(reader.node("ObjectId", objectStart, { kind: "string", value: hex(reader, 12) }));
    value = { kind: "object" };
  } else if (type === 15) {
    const size = length(reader);
    if (size < 14) {
      throw new ParseError("malformed", "Invalid code-with-scope size", start);
    }
    const payload = reader.take(size - 4),
      codeStart = payload.offset;
    const code = bsonString(payload);
    children.push(payload.node("JavaScript source (inert)", codeStart, code));
    children.push(readDocument(payload, "Scope", depth + 1));
    if (payload.remaining) {
      throw new ParseError("malformed", "Trailing code-with-scope bytes", payload.offset);
    }
    value = { kind: "object" };
  } else if (type === 16 || type === 18) {
    const width = type === 16 ? 4 : 8;
    value = integer(BigInt.asIntN(width * 8, unsigned(reader, width)));
  } else if (type === 17) {
    const increment = unsigned(reader, 4),
      secondsStart = reader.offset,
      seconds = unsigned(reader, 4);
    const incrementReader = new Reader(
      reader.bytes,
      reader.sourceId,
      reader.budget,
      start,
      start + 4,
    );
    incrementReader.offset = start + 4;
    children.push(incrementReader.node("Increment", start, integer(increment)));
    children.push(reader.node("Seconds since Unix epoch", secondsStart, integer(seconds)));
    value = { kind: "object" };
  } else if (type === 19) {
    value = { kind: "string", value: `0x${hex(reader, 16)} (little-endian Decimal128 bits)` };
  } else if ([6, 10, 0x7f, 0xff].includes(type)) {
    value = literal(types[type]!, type);
  } else {
    throw new ParseError("malformed", "Unknown BSON element type", start);
  }
  const node = finishNode(reader, name, start, value, children);
  node.format = { protocol: `BSON ${types[type]}` };
  return node;
}

function readDocument(reader: Reader, name: string, depth: number, array = false): Node {
  reader.budget.depth(depth, reader.offset);
  const start = reader.offset,
    size = length(reader);
  if (size < 5) {
    throw new ParseError("malformed", "Invalid BSON document size", start);
  }
  const truncated = size - 4 > reader.remaining;
  const payload = reader.take(Math.min(size - 4, reader.remaining)),
    children: Node[] = [];
  try {
    while (payload.remaining > 1) {
      const entryStart = payload.offset,
        type = payload.byte();
      if (!types[type]) {
        throw new ParseError("malformed", "Invalid BSON element type", entryStart);
      }
      const keyStart = payload.offset,
        key = cstring(payload);
      if (array && (key.clipped || key.value !== String(children.length))) {
        throw new ParseError(
          "malformed",
          "BSON array keys must be consecutive decimal indexes",
          keyStart,
        );
      }
      const keyNode = payload.node("Key", keyStart, {
        kind: "string",
        value: key.value + (key.clipped ? "… [preview]" : ""),
      });
      children.push(
        mapEntry(
          payload,
          `${Array.from(key.value).slice(0, 72).join("")}${key.clipped || Array.from(key.value).length > 72 ? "…" : ""} · ${types[type]}`,
          entryStart,
          keyNode,
          () => field(payload, type, "Value", depth + 1),
        ),
      );
    }
    if (truncated) {
      throw new ParseError("malformed", "Truncated BSON document", payload.offset);
    }
    if (payload.byte() !== 0 || payload.remaining) {
      throw new ParseError("malformed", "Missing BSON document terminator", payload.offset - 1);
    }
  } catch (error) {
    retainPartial(error, payload, name, start, children, array);
  }
  return finishNode(reader, name, start, { kind: array ? "array" : "object" }, children);
}

export function parse(bytes: Uint8Array, sourceId: string) {
  return parseValues(
    bytes,
    sourceId,
    "BSON",
    "Encoded field names, types and ordered occurrences. JavaScript and regular expressions are inert text. Decimal128 is shown as exact raw bits. Consecutive documents are shown in wire order.",
    readDocument,
  );
}
