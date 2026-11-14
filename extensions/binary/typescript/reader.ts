import type { Node, ParseStatus, Value, VisualizationDocument } from "transmog:extensions/v1";
import { MAX_INLINE_DEPTH, MAX_INLINE_NODES, MAX_INLINE_TEXT_UNITS } from "./limits.js";

import { ParseError } from "./parse-error.js";
import { scanText } from "./unicode.js";
export { ParseError } from "./parse-error.js";

const VARINT_GROUP_BITS = 7;
const VARINT_DATA_MASK = 0x7f;
const VARINT_CONTINUATION_BIT = 0x80;
const BITS_PER_BYTE = 8;
const FLOAT32_BYTES = 4;

/** Bounds materialized output; byte limits and the host isolate bound parser execution. */
export class DocumentLimits {
  private nodes = 0;
  private characters = 0;

  constructor(readonly prefix: string) {}

  depth(depth: number, offset: number): void {
    if (depth > MAX_INLINE_DEPTH) {
      throw new ParseError("limited", "Nesting limit reached", offset);
    }
  }

  node(name: string, reader: Reader, start: number, value?: Value, children: Node[] = []): Node {
    this.nodes++;
    if (this.nodes > MAX_INLINE_NODES) {
      throw new ParseError("limited", "Field display limit reached", start);
    }
    if (value?.kind === "string") {
      this.characters += value.value.length;
      if (this.characters > MAX_INLINE_TEXT_UNITS) {
        throw new ParseError("limited", "Text display limit reached", start);
      }
    }
    return {
      id: `${this.prefix}.${this.nodes}`,
      name,
      ...(value ? { value } : {}),
      spans: [
        { sourceId: reader.sourceId, offset: String(start), length: String(reader.offset - start) },
      ],
      children,
    };
  }
}

export class Reader {
  offset: number;

  constructor(
    readonly bytes: Uint8Array,
    readonly sourceId: string,
    readonly limits: DocumentLimits,
    readonly start = 0,
    readonly end = bytes.length,
  ) {
    this.offset = start;
  }

  get remaining(): number {
    return this.end - this.offset;
  }

  require(length: number): void {
    if (!Number.isSafeInteger(length) || length < 0 || length > this.remaining) {
      throw new ParseError("malformed", "Truncated value", this.offset);
    }
  }

  byte(): number {
    this.require(1);
    return this.bytes[this.offset++]!;
  }

  varint(bits = 64): bigint {
    const start = this.offset;
    let value = 0n;
    for (let shift = 0; shift < bits; shift += VARINT_GROUP_BITS) {
      const byte = this.byte();
      value |= BigInt(byte & VARINT_DATA_MASK) << BigInt(shift);
      if (value >= 1n << BigInt(bits)) {
        break;
      }
      if (byte < VARINT_CONTINUATION_BIT) {
        return value;
      }
    }
    throw new ParseError("malformed", `Invalid ${bits}-bit varint`, start);
  }

  fixed(width: number): bigint {
    this.require(width);
    let value = 0n;
    for (let index = 0; index < width; index++) {
      value |= BigInt(this.byte()) << BigInt(index * BITS_PER_BYTE);
    }
    return value;
  }

  floating(width: 4 | 8): number {
    this.require(width);
    const view = new DataView(this.bytes.buffer, this.bytes.byteOffset + this.offset, width);
    this.offset += width;
    return width === FLOAT32_BYTES ? view.getFloat32(0, true) : view.getFloat64(0, true);
  }

  take(length: number): Reader {
    this.require(length);
    const child = new Reader(
      this.bytes,
      this.sourceId,
      this.limits,
      this.offset,
      this.offset + length,
    );
    this.offset += length;
    return child;
  }

  node(name: string, start: number, value?: Value, children: Node[] = []): Node {
    return this.limits.node(name, this, start, value, children);
  }
}

export function integer(value: bigint | number): Value {
  return { kind: "integer", value: String(value) };
}

export function floating(value: number): Value {
  // JSON numbers cannot preserve the IEEE 754 sign of zero.
  if (Object.is(value, -0)) {
    return { kind: "string", value: "-0" };
  }
  return Number.isFinite(value)
    ? { kind: "float", value }
    : { kind: "string", value: Number.isNaN(value) ? "NaN" : value < 0 ? "-Infinity" : "Infinity" };
}

/** Validate the entire value, retaining only a bounded display preview. */
export function text(reader: Reader, wide = false): { value: string; clipped: boolean } {
  const result = scanText(reader.bytes, reader.offset, reader.end, wide, "validate-and-preview");
  reader.offset = reader.end;
  return result;
}

export function document(title: string, summary: string): VisualizationDocument {
  return {
    title,
    summary,
    status: "complete",
    updates: "final",
    presentation: "tree",
    roots: [],
    sources: [],
    diagnostics: [],
  };
}

export function failure(
  doc: VisualizationDocument,
  error: unknown,
  reader: Reader,
  start: number,
): void {
  if (!(error instanceof ParseError)) {
    throw error;
  }
  doc.status = error.status;
  doc.diagnostics!.push({ code: error.status, message: error.message });
  // Keep earlier complete fields and expose the rest as exact source bytes.
  doc.roots.push({
    id: `${reader.limits.prefix}.remainder`,
    name: "Unparsed bytes",
    value: { kind: "bytes" },
    spans: [
      { sourceId: reader.sourceId, offset: String(start), length: String(reader.end - start) },
    ],
    children: [],
  });
}

export function notice(title: string, status: ParseStatus, message: string): VisualizationDocument {
  return { ...document(title, message), status, diagnostics: [{ code: status, message }] };
}
