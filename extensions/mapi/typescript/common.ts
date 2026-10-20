import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import type { ParsedField } from "./model.js";
import { RecordReader } from "./record.js";
import type { ScalarType } from "./record.js";

export function readEnum(record: RecordReader, name: string, type = name): number {
  const definition = enums[type];
  if (!definition) {
    throw new Error(`Missing protocol enum: ${type}`);
  }
  return record.enumeration(name, type, definition);
}

export function readFields(
  record: RecordReader,
  fields: Readonly<Record<string, ScalarType>>,
): void {
  for (const [name, type] of Object.entries(fields)) {
    record.scalar(name, type);
  }
}

export function repeat(
  record: RecordReader,
  count: number,
  parse: (index: number) => void,
  minimumBytes = 1,
): void {
  if (!Number.isSafeInteger(count) || count < 0 || count > 10_000) {
    throw new ParseError("limit", "Array element limit exceeded", record.reader.range());
  }
  record.reader.require(count * minimumBytes);
  for (let index = 0; index < count; index++) {
    record.reader.budget.step(record.reader.range(), 1, "loops");
    parse(index);
  }
}

export function nested(
  record: RecordReader,
  name: string,
  parse: (child: RecordReader) => void,
  type = name,
): ParsedField {
  return record.structure(name, parse, type);
}

export function bounded(
  record: RecordReader,
  name: string,
  length: number,
  parse: (child: RecordReader) => void,
): ParsedField {
  const reader = record.reader.child(length);
  reader.budget.enter(reader.range());
  const child = new RecordReader(reader, name);
  try {
    parse(child);
    if (reader.remaining !== 0) {
      throw new ParseError("malformed", "Unexpected trailing bytes in structure", reader.range());
    }
  } finally {
    reader.budget.leave();
    record.add(child.finish());
  }
  const field = child.finish();
  return field;
}

export function readObjectId(record: RecordReader): void {
  record.integer("ReplicaId", "u16");
  record.bytes("GlobalCounter", 6);
}

export function readLongTermId(record: RecordReader): void {
  record.scalar("DatabaseGuid", "guid");
  record.bytes("GlobalCounter", 6);
  record.integer("Pad", "u16");
}

export function filetime(ticks: bigint): string | undefined {
  if (ticks < 0n || ticks > 2_650_467_743_999_999_999n) {
    return undefined;
  }
  const relative = ticks - 116_444_736_000_000_000n;
  const milliseconds = relative / 10_000n - (relative % 10_000n < 0n ? 1n : 0n);
  const fractions = String(ticks % 10_000_000n).padStart(7, "0");
  return new Date(Number(milliseconds)).toISOString().replace(/\.\d{3}Z$/, `.${fractions}Z`);
}
