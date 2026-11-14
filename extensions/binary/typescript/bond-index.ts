import * as Wire from "./bond-wire.js";
import * as Limits from "./bond-limits.js";
import { ParseError } from "./reader.js";
import { scanText } from "./unicode.js";
// Retain wire offsets, not a decoded object graph. Allocation is in small typed
// blocks so failed version probes do not reserve the maximum index up front.
// Row 0 groups top-level structs. Child/sibling links use 0 for "none".
// Start/End include the field header; Data points at its scalar or list payload.
export enum Column {
  Start,
  End,
  Data,
  Meta,
  Label,
  First,
  Next,
  Count,
  After, // First row after this value's entire subtree.
  Depth,
}
export enum Role {
  Root,
  Field,
  Item,
  Key,
  Value,
  Entry,
  Base,
}

const ROW_WIDTH = Column.Depth + 1;
const VALUE_TYPE_MASK = 0xff;
const ROLE_SHIFT = 8;
const ELEMENT_TYPE_SHIFT = 16;
const INCOMPLETE_FLAG = 1 << 24;
// A map entry is an index-only grouping, never an accepted wire type.
export const MAP_ENTRY_KIND = Wire.Type.WString + 1;

export class BondIndex {
  private blocks: Uint32Array[] = [];
  length = 0;
  offset: number;
  end: number;
  error?: ParseError;
  constructor(
    readonly bytes: Uint8Array,
    readonly sourceId: string,
    readonly version: number,
    readonly start: number,
    readonly sequence: boolean,
  ) {
    this.offset = start;
    this.end = bytes.length;
    this.add(Wire.Type.Struct, Role.Root, 0, 0, start);
    try {
      let ordinal = 0;
      do {
        const id = this.add(Wire.Type.Struct, Role.Root, ordinal++, 0);
        this.link(0, id);
        this.finish(id, () => this.structure(id, 0));
        if (!sequence && this.offset < this.end) {
          this.fail("Trailing bytes after Bond struct");
        }
      } while (this.offset < this.end);
    } catch (error) {
      if (!(error instanceof ParseError)) {
        throw error;
      }
      this.error = error;
    }
    this.set(0, Column.End, this.offset);
    this.set(0, Column.After, this.length);
  }
  get(id: number, column: Column): number {
    return this.blocks[Math.floor(id / Limits.INDEX_BLOCK_ROWS)]![
      (id % Limits.INDEX_BLOCK_ROWS) * ROW_WIDTH + column
    ]!;
  }
  private set(id: number, column: Column, value: number): void {
    this.blocks[Math.floor(id / Limits.INDEX_BLOCK_ROWS)]![
      (id % Limits.INDEX_BLOCK_ROWS) * ROW_WIDTH + column
    ] = value;
  }
  valueType(id: number): number {
    return this.get(id, Column.Meta) & VALUE_TYPE_MASK;
  }
  rowRole(id: number): Role {
    return (this.get(id, Column.Meta) >>> ROLE_SHIFT) & VALUE_TYPE_MASK;
  }
  elementType(id: number): number {
    return (this.get(id, Column.Meta) >>> ELEMENT_TYPE_SHIFT) & VALUE_TYPE_MASK;
  }
  isIncomplete(id: number): boolean {
    return (this.get(id, Column.Meta) & INCOMPLETE_FLAG) !== 0;
  }
  private add(kind: number, role: Role, label: number, depth: number, start = this.offset): number {
    if (this.length >= Limits.MAX_INDEX_VALUES) {
      throw new ParseError(
        "limited",
        `Bond index limit reached (${Limits.MAX_INDEX_VALUES.toLocaleString("en-US")} values)`,
        this.offset,
      );
    }
    if (depth > Limits.MAX_WIRE_DEPTH) {
      throw new ParseError(
        "limited",
        `Bond nesting limit reached (${Limits.MAX_WIRE_DEPTH} levels)`,
        this.offset,
      );
    }
    const id = this.length++;
    if (id % Limits.INDEX_BLOCK_ROWS === 0) {
      this.blocks.push(new Uint32Array(Limits.INDEX_BLOCK_ROWS * ROW_WIDTH));
    }
    this.set(id, Column.Start, start);
    this.set(id, Column.Meta, kind | (role << ROLE_SHIFT) | INCOMPLETE_FLAG);
    this.set(id, Column.Label, label);
    this.set(id, Column.Depth, depth);
    this.set(id, Column.Data, this.offset);
    return id;
  }
  private link(parent: number, id: number): void {
    // Only active containers need a temporary sibling tail.
    const previous = this.tails.get(parent);
    if (previous == null) {
      this.set(parent, Column.First, id);
    } else {
      this.set(previous, Column.Next, id);
    }
    this.tails.set(parent, id);
    this.set(parent, Column.Count, this.get(parent, Column.Count) + 1);
  }
  private tails = new Map<number, number>();
  private finish(id: number, action: () => void): void {
    try {
      action();
      this.set(id, Column.Meta, this.get(id, Column.Meta) & ~INCOMPLETE_FLAG);
    } finally {
      this.set(id, Column.End, this.offset);
      this.set(id, Column.After, this.length);
      this.tails.delete(id);
    }
  }
  private fail(message: string, offset = this.offset): never {
    throw new ParseError("malformed", message, offset);
  }
  private require(length: number): void {
    if (length < 0 || length > this.end - this.offset) {
      this.fail("Truncated value");
    }
  }
  private byte(): number {
    this.require(1);
    return this.bytes[this.offset++]!;
  }
  private tag(raw: number): number {
    if (raw <= Wire.Type.StopBase || raw >= Wire.TYPE_NAMES.length) {
      this.fail("Invalid Bond value type", this.offset - 1);
    }
    return raw;
  }
  private varint(bits = Wire.LENGTH_BITS): number {
    const start = this.offset;
    let result = 0;
    for (let shift = 0; shift < bits; shift += Wire.VARINT_GROUP_BITS) {
      const byte = this.byte();
      const payload = byte & Wire.VARINT_DATA_MASK;
      if (shift + Wire.VARINT_GROUP_BITS > bits && payload >= 2 ** (bits - shift)) {
        break;
      }
      // 64-bit values are validated here and decoded exactly only for display.
      if (bits <= Wire.LENGTH_BITS) {
        result += payload * 2 ** shift;
      }
      if (byte < Wire.VARINT_CONTINUATION_BIT) {
        return result;
      }
    }
    return this.fail(`Invalid ${bits}-bit varint`, start);
  }
  private structure(parent: number, depth: number): void {
    if (depth > Limits.MAX_WIRE_DEPTH) {
      throw new ParseError(
        "limited",
        `Bond nesting limit reached (${Limits.MAX_WIRE_DEPTH} levels)`,
        this.offset,
      );
    }
    const outer = this.end;
    let truncated = false;
    if (this.version === Wire.Version.V2) {
      const length = this.varint();
      truncated = length > this.end - this.offset;
      this.end = Math.min(this.end, this.offset + length);
    }
    try {
      while (this.offset < this.end) {
        const start = this.offset;
        const header = this.byte();
        const kind = header & Wire.TYPE_TAG_MASK;
        if (kind === Wire.Type.Stop) {
          if (
            header ||
            (this.version === Wire.Version.V2 && (truncated || this.offset !== this.end))
          ) {
            this.fail("Invalid Bond struct terminator or length", start);
          }
          return;
        }
        if (kind === Wire.Type.StopBase) {
          if (header !== Wire.Type.StopBase) {
            this.fail("Invalid Bond base terminator", start);
          }
          const id = this.add(Wire.Type.StopBase, Role.Base, 0, depth, start);
          this.link(parent, id);
          this.finish(id, () => {});
          continue;
        }
        this.tag(kind);
        const encoded = header >>> Wire.FIELD_ID_SHIFT;
        const label =
          encoded === Wire.EXTENDED_FIELD_ID8
            ? this.byte()
            : encoded === Wire.EXTENDED_FIELD_ID16
              ? this.byte() | (this.byte() << Wire.BITS_PER_BYTE)
              : encoded;
        this.value(parent, kind, Role.Field, label, depth, start);
      }
      this.fail("Missing Bond struct terminator");
    } finally {
      this.end = outer;
    }
  }
  private value(
    parent: number,
    kind: number,
    role: Role,
    label: number,
    depth: number,
    start = this.offset,
  ): void {
    const id = this.add(kind, role, label, depth, start);
    this.link(parent, id);
    this.finish(id, () => {
      if (kind === Wire.Type.Bool) {
        if (this.byte() > 1) {
          this.fail("Invalid Bond Boolean", this.offset - 1);
        }
      } else if (kind === Wire.Type.UInt8 || kind === Wire.Type.Int8) {
        this.byte();
      } else if (
        [
          Wire.Type.UInt16,
          Wire.Type.UInt32,
          Wire.Type.UInt64,
          Wire.Type.Int16,
          Wire.Type.Int32,
          Wire.Type.Int64,
        ].includes(kind)
      ) {
        this.varint(Wire.integerBits(kind));
      } else if (kind === Wire.Type.Float32 || kind === Wire.Type.Float64) {
        const length = kind === Wire.Type.Float32 ? Wire.FLOAT32_BYTES : Wire.FLOAT64_BYTES;
        this.require(length);
        this.offset += length;
      } else if (kind === Wire.Type.String || kind === Wire.Type.WString) {
        const length = this.varint() * (kind === Wire.Type.WString ? Wire.UINT16_BYTES : 1);
        this.require(length);
        this.set(id, Column.Data, this.offset);
        const data = this.offset;
        this.offset += length;
        scanText(this.bytes, data, this.offset, kind === Wire.Type.WString, "validate");
      } else if (kind === Wire.Type.Struct) {
        this.structure(id, depth + 1);
      } else if (kind === Wire.Type.List || kind === Wire.Type.Set) {
        const header = this.byte();
        const element = this.tag(header & Wire.TYPE_TAG_MASK);
        if (this.version === Wire.Version.V1 && header > Wire.TYPE_TAG_MASK) {
          this.fail("Invalid Bond v1 container type", this.offset - 1);
        }
        const count =
          this.version === Wire.Version.V2 && header >>> Wire.CONTAINER_COUNT_SHIFT
            ? (header >>> Wire.CONTAINER_COUNT_SHIFT) - 1
            : this.varint();
        this.set(id, Column.Meta, this.get(id, Column.Meta) | (element << ELEMENT_TYPE_SHIFT));
        this.set(id, Column.Data, this.offset);
        const stride = Wire.fixedWidth(element);
        if (stride) {
          if (count && depth >= Limits.MAX_WIRE_DEPTH) {
            throw new ParseError(
              "limited",
              `Bond nesting limit reached (${Limits.MAX_WIRE_DEPTH} levels)`,
              this.offset,
            );
          }
          const available = Math.min(count, Math.floor((this.end - this.offset) / stride));
          this.set(id, Column.Count, available);
          if (element === Wire.Type.Bool) {
            for (let index = 0; index < available; index++) {
              if (this.byte() > 1) {
                this.set(id, Column.Count, index);
                this.fail("Invalid Bond Boolean", this.offset - 1);
              }
            }
          } else {
            this.offset += available * stride;
          }
          if (available < count) {
            this.fail("Truncated value");
          }
        } else {
          for (let index = 0; index < count; index++) {
            this.value(id, element, Role.Item, index, depth + 1);
          }
        }
      } else if (kind === Wire.Type.Map) {
        const key = this.tag(this.byte());
        const mapped = this.tag(this.byte());
        const count = this.varint();
        for (let index = 0; index < count; index++) {
          const entry = this.add(MAP_ENTRY_KIND, Role.Entry, index, depth + 1);
          this.link(id, entry);
          this.finish(entry, () => {
            this.value(entry, key, Role.Key, 0, depth + 1);
            this.value(entry, mapped, Role.Value, 0, depth + 1);
          });
        }
      }
    });
  }
}
