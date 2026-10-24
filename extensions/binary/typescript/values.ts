import type { Node, Value, VisualizationDocument } from "transmog:extensions/v1";
import { Budget, Reader, ParseError, document, failure, floating, text } from "./reader.js";

export function unsigned(reader: Reader, width: number, bigEndian = false): bigint {
  reader.require(width);
  let result = 0n;
  for (let index = 0; index < width; index++) {
    const shift = (bigEndian ? width - index - 1 : index) * 8;
    result |= BigInt(reader.byte()) << BigInt(shift);
  }
  return result;
}

export function float(reader: Reader, width: 4 | 8, bigEndian = false): Value {
  const bytes = reader.take(width);
  reader.budget.step(bytes.offset, width);
  const view = new DataView(reader.bytes.buffer, reader.bytes.byteOffset + bytes.offset, width);
  return floating(width === 4 ? view.getFloat32(0, !bigEndian) : view.getFloat64(0, !bigEndian));
}

export function string(reader: Reader, length: number): Value {
  const decoded = text(reader.take(length));
  return { kind: "string", value: decoded.value + (decoded.clipped ? "… [preview]" : "") };
}

export function literal(name: string, raw = 0): Value {
  return { kind: "enum", value: { name, raw: String(raw) } };
}

export function hex(reader: Reader, length: number): string {
  reader.require(length);
  let result = "";
  for (let index = 0; index < length; index++) {
    result += reader.byte().toString(16).padStart(2, "0");
  }
  return result;
}

export function count(value: bigint, reader: Reader): number {
  if (value > BigInt(reader.remaining)) {
    throw new ParseError(
      "malformed",
      "Declared length or count exceeds retained bytes",
      reader.offset,
    );
  }
  return Number(value);
}

/** Names are presentation only; complete keys remain separate source-linked nodes. */
export function entryName(key: Node, index: number): string {
  const value = key.value;
  return value && "value" in value && ["string", "integer", "boolean"].includes(value.kind)
    ? `Key ${JSON.stringify(value.value).slice(0, 100)}`
    : `Entry ${index + 1}`;
}

export function parseValues(
  bytes: Uint8Array,
  sourceId: string,
  title: string,
  summary: string,
  read: (reader: Reader, name: string, depth: number) => Node,
): VisualizationDocument {
  const doc = document(title, summary);
  const reader = new Reader(bytes, sourceId, new Budget(title.toLowerCase()));
  let start = 0;
  try {
    if (!reader.remaining) {
      throw new ParseError("malformed", `Missing ${title} value`, 0);
    }
    while (reader.remaining) {
      start = reader.offset;
      doc.roots.push(read(reader, `Value ${doc.roots.length + 1}`, 0));
    }
  } catch (error) {
    failure(doc, error, reader, start);
  }
  return doc;
}
