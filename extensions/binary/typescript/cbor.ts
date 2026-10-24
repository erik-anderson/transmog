import type { Input, Node, Value } from "transmog:extensions/v1";
import type { Reader } from "./reader.js";
import { ParseError, floating, integer, text } from "./reader.js";
import {
  count,
  entryName,
  float,
  literal,
  mapEntry,
  parseValues,
  retainPartial,
  string,
  unsigned,
} from "./values.js";

function argument(reader: Reader, info: number): bigint | null {
  if (info < 24) {
    return BigInt(info);
  }
  if (info <= 27) {
    return unsigned(reader, 2 ** (info - 24), true);
  }
  if (info === 31) {
    return null;
  }
  throw new ParseError("malformed", "Reserved CBOR additional information", reader.offset - 1);
}

function read(reader: Reader, name: string, depth: number): Node {
  reader.budget.depth(depth, reader.offset);
  const start = reader.offset,
    tag = reader.byte(),
    major = tag >> 5,
    info = tag & 31;
  let value: Value;
  const children: Node[] = [];
  if (major === 7 && info >= 25 && info <= 27) {
    if (info === 25) {
      const bits = Number(unsigned(reader, 2, true));
      const sign = bits & 0x8000 ? -1 : 1,
        exponent = (bits >> 10) & 31,
        fraction = bits & 1023;
      value = floating(
        exponent === 31
          ? fraction
            ? NaN
            : sign * Infinity
          : sign * (exponent ? (1 + fraction / 1024) * 2 ** (exponent - 15) : fraction * 2 ** -24),
      );
    } else {
      value = float(reader, info === 26 ? 4 : 8, true);
    }
    return reader.node(name, start, value);
  }
  const size = argument(reader, info);
  if (size === null && (major < 2 || major > 5)) {
    throw new ParseError("malformed", "Unexpected indefinite value or break", start);
  }
  if (major === 0) {
    value = integer(size!);
  } else if (major === 1) {
    value = integer(-1n - size!);
  } else if (major === 2 || major === 3) {
    if (size !== null) {
      const length = count(size, reader);
      if (major === 3) {
        value = string(reader, length);
      } else {
        const payloadStart = reader.offset;
        reader.take(length);
        children.push(reader.node("Byte string payload", payloadStart, { kind: "bytes" }));
        value = literal("Byte string", 2);
      }
    } else {
      // Each chunk must be definite and independently valid UTF-8.
      let combined = "",
        clipped = false,
        index = 0;
      try {
        while (reader.remaining && reader.bytes[reader.offset] !== 0xff) {
          const chunkStart = reader.offset,
            chunkTag = reader.byte();
          if (chunkTag >> 5 !== major || (chunkTag & 31) === 31) {
            throw new ParseError("malformed", "Invalid indefinite string chunk", chunkStart);
          }
          const length = count(argument(reader, chunkTag & 31)!, reader);
          const payload = reader.take(length);
          let chunkValue: Value = { kind: "bytes" };
          if (major === 3) {
            const decoded = text(payload);
            chunkValue = {
              kind: "string",
              value: decoded.value + (decoded.clipped ? "… [preview]" : ""),
            };
            if (combined.length + decoded.value.length <= 1024 && !clipped) {
              combined += decoded.value;
            } else {
              clipped = true;
            }
            clipped ||= decoded.clipped;
          }
          children.push(
            reader.node(`Chunk ${++index}`, major === 2 ? payload.start : chunkStart, chunkValue),
          );
        }
        if (reader.byte() !== 0xff) {
          throw new ParseError("malformed", "Missing string break", start);
        }
      } catch (error) {
        retainPartial(error, reader, name, start, children);
      }
      value =
        major === 3
          ? { kind: "string", value: combined + (clipped ? "… [preview]" : "") }
          : literal("Indefinite byte string", 2);
    }
  } else if (major === 4 || major === 5) {
    const length = size;
    let index = 0;
    try {
      while (
        length === null
          ? reader.remaining && reader.bytes[reader.offset] !== 0xff
          : BigInt(index) < length
      ) {
        if (major === 4) {
          children.push(read(reader, `Item ${index + 1}`, depth + 1));
        } else {
          const entryStart = reader.offset,
            key = read(reader, "Key", depth + 1);
          children.push(
            mapEntry(reader, entryName(key, index), entryStart, key, () =>
              read(reader, "Value", depth + 1),
            ),
          );
        }
        index++;
      }
      if (length === null && reader.byte() !== 0xff) {
        throw new ParseError("malformed", "Missing collection break", start);
      }
    } catch (error) {
      retainPartial(error, reader, name, start, children, major === 4);
    }
    value = { kind: major === 4 ? "array" : "object" };
  } else if (major === 6) {
    // Preserve the tag itself. A tag does not establish valid application semantics.
    children.push(read(reader, "Tagged value", depth + 1));
    value = literal(`Tag ${size}`, 0);
  } else {
    if (info === 24 && size! < 32n) {
      throw new ParseError("malformed", "Invalid extended simple value", start);
    }
    value =
      size === 20n || size === 21n
        ? { kind: "boolean", value: size === 21n }
        : literal(
            size === 22n ? "null" : size === 23n ? "undefined" : `Simple ${size}`,
            Number(size),
          );
  }
  return reader.node(name, start, value, children);
}

export function parse(bytes: Uint8Array, sourceId: string, input?: Input) {
  const types =
    input?.[input.direction]?.headers.filter(
      (field) => field.name.toLowerCase() === "content-type",
    ) ?? [];
  const type = types.length === 1 ? types[0] : undefined;
  const sequence = !!(
    type &&
    !type.redacted &&
    type.value &&
    type.value.length <= 256 &&
    /^[\t ]*application\/cbor-seq(?:[\t ]*;|[\t ]*$)/i.test(String.fromCharCode(...type.value))
  );
  return parseValues(
    bytes,
    sourceId,
    "CBOR",
    "Encoded types, map keys, tags and string chunks. Tags are preserved without assuming application semantics. Consecutive data items are shown in wire order.",
    read,
    sequence,
  );
}
