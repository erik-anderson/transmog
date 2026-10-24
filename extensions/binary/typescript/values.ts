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
    ? `Key ${Array.from(JSON.stringify(value.value)).slice(0, 72).join("")}`
    : `Entry ${index + 1}`;
}

/** Parent display allocation must not discard already completed children. */
export function finishNode(
  reader: Reader,
  name: string,
  start: number,
  value: Value,
  children: Node[],
): Node {
  try {
    return reader.node(name, start, value, children);
  } catch (error) {
    retainPartial(error, reader, name, start, children, value.kind === "array");
  }
}

/** Keep a decoded key attached to its value, including incomplete nested values. */
export function mapEntry(
  reader: Reader,
  name: string,
  start: number,
  key: Node,
  read: () => Node,
): Node {
  const children = [key];
  try {
    children.push(read());
  } catch (error) {
    retainPartial(error, reader, name, start, children);
  }
  return finishNode(reader, name, start, { kind: "object" }, children);
}

/** Retain only completed children; the enclosing value is explicitly partial. */
export function retainPartial(
  error: unknown,
  reader: Reader,
  name: string,
  start: number,
  children: Node[],
  array = false,
): never {
  if (error instanceof ParseError) {
    if (error.partial) {
      children.push(error.partial);
    }
    if (children.length) {
      error.partial = {
        id: `${reader.budget.prefix}.partial.${start}`,
        name: `Partial ${name}`,
        value: { kind: array ? "array" : "object" },
        spans: [
          {
            sourceId: reader.sourceId,
            offset: String(start),
            length: String(reader.offset - start),
          },
        ],
        children,
      };
    }
  }
  throw error;
}

export function parseValues(
  bytes: Uint8Array,
  sourceId: string,
  title: string,
  summary: string,
  read: (reader: Reader, name: string, depth: number) => Node,
  allowEmpty = false,
): VisualizationDocument {
  const doc = document(title, summary);
  const reader = new Reader(bytes, sourceId, new Budget(title.toLowerCase()));
  let start = 0;
  try {
    if (!reader.remaining && !allowEmpty) {
      throw new ParseError("malformed", `Missing ${title} value`, 0);
    }
    while (reader.remaining) {
      start = reader.offset;
      doc.roots.push(read(reader, `Value ${doc.roots.length + 1}`, 0));
    }
  } catch (error) {
    if (error instanceof ParseError && error.partial) {
      doc.roots.push(error.partial);
    }
    failure(doc, error, reader, start);
    if (error instanceof ParseError && error.partial) {
      doc.roots.at(-1)!.name = "Incomplete value bytes";
    }
  }
  return doc;
}
