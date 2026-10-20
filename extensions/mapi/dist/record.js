import { BinaryReader } from "./reader.js";
/** Collects protocol fields while reading. It has no presentation behavior. */
export class RecordReader {
    reader;
    field;
    constructor(reader, name, type = name) {
        this.reader = reader;
        reader.budget.field(reader.range());
        this.field = { name, type, range: reader.range(), children: [] };
    }
    finish() {
        return {
            ...this.field,
            range: this.reader.range(this.field.range.offset, this.reader.offset - this.field.range.offset),
        };
    }
    add(field) {
        this.field.children.push(field);
    }
    scalar(name, type) {
        const start = this.reader.offset;
        let value;
        let parsed;
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
    integer(name, type) {
        return this.scalar(name, type);
    }
    boolean(name, width = "u8") {
        const start = this.reader.offset;
        const value = this.reader.integer(width) !== 0;
        this.value(name, "bool", start, { kind: "boolean", value });
        return value;
    }
    enumeration(name, type, definition) {
        const start = this.reader.offset;
        const raw = definition.wire.endsWith("64")
            ? this.reader.integer64(definition.wire === "i64")
            : this.reader.integer(definition.wire);
        const value = Number(raw);
        const exact = Object.entries(definition.values).find(([, candidate]) => candidate === value);
        const names = exact
            ? [exact[0]]
            : definition.flags
                ? Object.entries(definition.values)
                    .filter(([, candidate]) => candidate !== 0 && (BigInt(value) & BigInt(candidate)) === BigInt(candidate))
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
    string(name, wide, length) {
        const start = this.reader.offset;
        const value = length === undefined
            ? this.reader.terminatedString(wide)
            : wide
                ? this.reader.utf16(length)
                : this.reader.ascii(length);
        const field = this.value(name, "string", start, { kind: "string", value });
        field.encoding = wide ? "utf-16le" : "us-ascii";
        return value;
    }
    line(name) {
        const start = this.reader.offset;
        const value = this.reader.line();
        this.value(name, "string", start, { kind: "string", value }).encoding = "us-ascii";
        return value;
    }
    bytes(name, length) {
        const start = this.reader.offset;
        const value = this.reader.take(length);
        this.value(name, "bytes", start, { kind: "bytes" });
        return value;
    }
    structure(name, parse, type = name) {
        this.reader.budget.enter(this.reader.range());
        const child = new RecordReader(this.reader, name, type);
        try {
            parse(child);
        }
        finally {
            this.reader.budget.leave();
        }
        const field = child.finish();
        this.add(field);
        return field;
    }
    value(name, type, start, value) {
        const range = this.reader.range(start, this.reader.offset - start);
        this.reader.budget.field(range);
        const field = { name, type, value, range, children: [] };
        this.add(field);
        return field;
    }
}
