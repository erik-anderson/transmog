import { BinaryReader } from "./reader.js";
import type { IntegerType } from "./reader.js";
import type { FieldValue, ParsedField } from "./model.js";

export type ScalarType = IntegerType | "u64" | "i64" | "f32" | "f64" | "bool" | "guid";

export interface EnumDefinition {
  readonly wire: IntegerType | "u64" | "i64";
  readonly flags: boolean;
  readonly values: Readonly<Record<string, number>>;
}

/** Collects protocol fields while reading. It has no presentation behavior. */
export class RecordReader {
  readonly field: ParsedField;

  constructor(
    readonly reader: BinaryReader,
    name: string,
    type = name,
  ) {
    reader.budget.field(reader.range());
    this.field = { name, type, range: reader.range(), children: [] };
  }

  finish(): ParsedField {
    return {
      ...this.field,
      range: this.reader.range(
        this.field.range.offset,
        this.reader.offset - this.field.range.offset,
      ),
    };
  }

  add(field: ParsedField): void {
    this.field.children.push(field);
  }

  scalar(name: string, type: ScalarType): number | bigint | boolean | string {
    const start = this.reader.offset;
    let value: number | bigint | boolean | string;
    let parsed: FieldValue;
    switch (type) {
      case "u64":
      case "i64":
        value = this.reader.integer64(type === "i64");
        parsed = { kind: "integer", value: value.toString() };
        break;
      case "f32":
      case "f64":
        value = this.reader.floatingPoint(type === "f32" ? 4 : 8);
        parsed = { kind: "float", value: Number.isFinite(value) ? value : String(value) };
        break;
      case "bool":
        value = this.reader.integer("u8") !== 0;
        parsed = { kind: "boolean", value };
        break;
      case "guid":
        value = this.reader.guid();
        parsed = { kind: "guid", value };
        break;
      default:
        value = this.reader.integer(type);
        parsed = { kind: "integer", value: String(value) };
    }
    this.value(name, type, start, parsed);
    return value;
  }

  integer(name: string, type: IntegerType): number {
    return this.scalar(name, type) as number;
  }

  boolean(name: string, width: IntegerType = "u8"): boolean {
    const start = this.reader.offset;
    const value = this.reader.integer(width) !== 0;
    this.value(name, "bool", start, { kind: "boolean", value });
    return value;
  }

  enumeration(name: string, type: string, definition: EnumDefinition): number {
    const start = this.reader.offset;
    const raw = definition.wire.endsWith("64")
      ? this.reader.integer64(definition.wire === "i64")
      : this.reader.integer(definition.wire as IntegerType);
    const value = Number(raw);
    const exact = Object.entries(definition.values).find(([, candidate]) => candidate === value);
    const names = exact
      ? [exact[0]]
      : definition.flags
        ? Object.entries(definition.values)
            .filter(
              ([, candidate]) =>
                candidate !== 0 && (BigInt(value) & BigInt(candidate)) === BigInt(candidate),
            )
            .map(([name]) => name)
        : [];
    this.value(name, type, start, {
      kind: "enum",
      value: raw.toString(),
      names,
      flags: definition.flags,
    });
    return value;
  }

  string(name: string, wide: boolean, length?: number): string {
    const start = this.reader.offset;
    const decoded =
      length === undefined
        ? this.reader.terminatedString(wide)
        : wide
          ? this.reader.utf16(length)
          : this.reader.ascii(length);
    const value = length === undefined ? decoded : decoded.replace(/\0$/, "");
    const field = this.value(name, "string", start, { kind: "string", value });
    field.encoding = wide ? "utf-16le" : "us-ascii";
    return value;
  }

  line(name: string): string {
    const start = this.reader.offset;
    const value = this.reader.line();
    this.value(name, "string", start, { kind: "string", value }).encoding = "us-ascii";
    return value;
  }

  bytes(name: string, length: number): number[] {
    const start = this.reader.offset;
    const value = this.reader.take(length);
    this.value(name, "bytes", start, { kind: "bytes" });
    return value;
  }

  structure(name: string, parse: (record: RecordReader) => void, type = name): ParsedField {
    this.reader.budget.enter(this.reader.range());
    const child = new RecordReader(this.reader, name, type);
    try {
      parse(child);
    } finally {
      this.reader.budget.leave();
    }
    const field = child.finish();
    this.add(field);
    return field;
  }

  private value(name: string, type: string, start: number, value: FieldValue): ParsedField {
    const range = this.reader.range(start, this.reader.offset - start);
    this.reader.budget.field(range);
    const field: ParsedField = { name, type, value, range, children: [] };
    this.add(field);
    return field;
  }
}
