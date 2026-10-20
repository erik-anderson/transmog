import { enums } from "./constants.js";
import { ParseError } from "./model.js";
import { RecordReader } from "./record.js";
export function readEnum(record, name, type = name) {
    const definition = enums[type];
    if (!definition) {
        throw new Error(`Missing protocol enum: ${type}`);
    }
    return record.enumeration(name, type, definition);
}
export function readFields(record, fields) {
    for (const [name, type] of Object.entries(fields)) {
        record.scalar(name, type);
    }
}
export function repeat(record, count, parse, minimumBytes = 1) {
    if (!Number.isSafeInteger(count) || count < 0 || count > 10_000) {
        throw new ParseError("limit", "Array element limit exceeded", record.reader.range());
    }
    record.reader.require(count * minimumBytes);
    for (let index = 0; index < count; index++) {
        record.reader.budget.step(record.reader.range());
        parse(index);
    }
}
export function nested(record, name, parse, type = name) {
    return record.structure(name, parse, type);
}
export function bounded(record, name, length, parse) {
    const reader = record.reader.child(length);
    reader.budget.enter(reader.range());
    const child = new RecordReader(reader, name);
    try {
        parse(child);
        if (reader.remaining !== 0) {
            throw new ParseError("malformed", "Unexpected trailing bytes in structure", reader.range());
        }
    }
    finally {
        reader.budget.leave();
    }
    const field = child.finish();
    record.add(field);
    return field;
}
export function readObjectId(record) {
    record.integer("ReplicaId", "u16");
    record.bytes("GlobalCounter", 6);
}
export function readLongTermId(record) {
    record.scalar("DatabaseGuid", "guid");
    record.bytes("GlobalCounter", 6);
    record.integer("Pad", "u16");
}
export function filetime(ticks) {
    if (ticks < 0n || ticks > 2650467743999999999n) {
        return undefined;
    }
    const relative = ticks - 116444736000000000n;
    const milliseconds = relative / 10000n - (relative % 10000n < 0n ? 1n : 0n);
    const fractions = String(ticks % 10000000n).padStart(7, "0");
    return new Date(Number(milliseconds)).toISOString().replace(/\.\d{3}Z$/, `.${fractions}Z`);
}
