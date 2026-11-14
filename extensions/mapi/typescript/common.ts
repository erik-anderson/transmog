import { wireSizes } from "./wire.js";
import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import { parserLimits } from "./limits.js";
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
  if (!Number.isSafeInteger(count) || count < 0 || count > parserLimits.collectionElements) {
    throw new ParseError("limit", "Array element limit exceeded", record.reader.range());
  }
  record.reader.require(count * minimumBytes);
  for (let index = 0; index < count; index++) {
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
  const child = new RecordReader(reader, name);
  reader.limits.enter(reader.range());
  try {
    parse(child);
    if (reader.remaining !== 0) {
      throw new ParseError("malformed", "Unexpected trailing bytes in structure", reader.range());
    }
  } finally {
    reader.limits.leave();
    record.add(child.finish());
  }
  return child.finish();
}

export function readObjectId(record: RecordReader): void {
  record.integer("ReplicaId", "u16");
  record.bytes("GlobalCounter", wireSizes.globalCounter);
}

export function readLongTermId(record: RecordReader): void {
  record.scalar("DatabaseGuid", "guid");
  record.bytes("GlobalCounter", wireSizes.globalCounter);
  record.integer("Pad", "u16");
}

const maximumFiletime = 2_650_467_743_999_999_999n;
const filetimeUnixEpoch = 116_444_736_000_000_000n;
const filetimeTicksPerMillisecond = 10_000n;
const filetimeTicksPerSecond = 10_000_000n;
const filetimeFractionDigits = 7;

export function filetime(ticks: bigint): string | undefined {
  if (ticks < 0n || ticks > maximumFiletime) {
    return undefined;
  }
  const relative = ticks - filetimeUnixEpoch;
  const milliseconds =
    relative / filetimeTicksPerMillisecond -
    (relative % filetimeTicksPerMillisecond < 0n ? 1n : 0n);
  const fractions = String(ticks % filetimeTicksPerSecond).padStart(filetimeFractionDigits, "0");
  return new Date(Number(milliseconds)).toISOString().replace(/\.\d{3}Z$/, `.${fractions}Z`);
}
