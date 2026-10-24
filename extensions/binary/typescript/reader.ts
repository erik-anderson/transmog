import type { Node, ParseStatus, Value, VisualizationDocument } from "transmog:extensions/v1";

export class ParseError extends Error {
  partial?: Node;
  constructor(
    readonly status: "malformed" | "limited",
    message: string,
    readonly offset: number,
  ) {
    super(`${message} (byte ${offset}).`);
  }
}

/** Shared by speculative interpretations too: failed probes still consume work. */
export class Budget {
  private work = 0;
  private nodes = 0;
  private characters = 0;

  constructor(readonly prefix: string) {}

  step(offset: number, cost = 1): void {
    this.work += cost;
    if (this.work > 200_000) {
      throw new ParseError("limited", "Parser work limit reached", offset);
    }
  }

  depth(depth: number, offset: number): void {
    if (depth > 10) {
      throw new ParseError("limited", "Nesting limit reached", offset);
    }
  }

  node(name: string, reader: Reader, start: number, value?: Value, children: Node[] = []): Node {
    this.nodes++;
    if (this.nodes > 4_000) {
      throw new ParseError("limited", "Field display limit reached", start);
    }
    if (value?.kind === "string") {
      this.characters += value.value.length;
      if (this.characters > 65_536) {
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
    readonly budget: Budget,
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
    this.budget.step(this.offset);
    this.require(1);
    return this.bytes[this.offset++]!;
  }

  varint(bits = 64): bigint {
    const start = this.offset;
    let value = 0n;
    for (let shift = 0; shift < bits; shift += 7) {
      const byte = this.byte();
      value |= BigInt(byte & 127) << BigInt(shift);
      if (value >= 1n << BigInt(bits)) {
        break;
      }
      if (byte < 128) {
        return value;
      }
    }
    throw new ParseError("malformed", `Invalid ${bits}-bit varint`, start);
  }

  fixed(width: number): bigint {
    this.require(width);
    let value = 0n;
    for (let index = 0; index < width; index++) {
      value |= BigInt(this.byte()) << BigInt(index * 8);
    }
    return value;
  }

  floating(width: 4 | 8): number {
    this.require(width);
    const view = new DataView(this.bytes.buffer, this.bytes.byteOffset + this.offset, width);
    this.offset += width;
    return width === 4 ? view.getFloat32(0, true) : view.getFloat64(0, true);
  }

  take(length: number): Reader {
    this.require(length);
    const child = new Reader(
      this.bytes,
      this.sourceId,
      this.budget,
      this.offset,
      this.offset + length,
    );
    this.offset += length;
    return child;
  }

  node(name: string, start: number, value?: Value, children: Node[] = []): Node {
    return this.budget.node(name, this, start, value, children);
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

/** Strict Unicode decoding without depending on DOM APIs in the isolated worker. */
export function text(reader: Reader, wide = false): { value: string; clipped: boolean } {
  let value = "";
  let clipped = false;
  while (reader.remaining) {
    const start = reader.offset;
    let point = reader.byte();
    if (wide) {
      point |= reader.byte() << 8;
      if (point >= 0xd800 && point <= 0xdbff) {
        const low = reader.byte() | (reader.byte() << 8);
        if (low < 0xdc00 || low > 0xdfff) {
          throw new ParseError("malformed", "Invalid UTF-16 surrogate pair", start);
        }
        point = 0x10000 + ((point - 0xd800) << 10) + low - 0xdc00;
      } else if (point >= 0xdc00 && point <= 0xdfff) {
        throw new ParseError("malformed", "Unpaired UTF-16 surrogate", start);
      }
    } else if (point >= 128) {
      const count =
        point >= 0xc2 && point <= 0xdf
          ? 1
          : point >= 0xe0 && point <= 0xef
            ? 2
            : point >= 0xf0 && point <= 0xf4
              ? 3
              : -1;
      if (count < 0) {
        throw new ParseError("malformed", "Invalid UTF-8 leading byte", start);
      }
      point &= (1 << (6 - count)) - 1;
      for (let index = 0; index < count; index++) {
        const next = reader.byte();
        if ((next & 0xc0) !== 0x80) {
          throw new ParseError("malformed", "Invalid UTF-8 continuation", start);
        }
        point = (point << 6) | (next & 63);
      }
      if (
        point < [0, 0x80, 0x800, 0x10000][count]! ||
        point > 0x10ffff ||
        (point >= 0xd800 && point <= 0xdfff)
      ) {
        throw new ParseError("malformed", "Invalid UTF-8 code point", start);
      }
    }
    const character = String.fromCodePoint(point);
    if (!clipped && value.length + character.length <= 1_024) {
      value += character;
    } else {
      clipped = true;
    }
  }
  return { value, clipped };
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
    id: `${reader.budget.prefix}.remainder`,
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
