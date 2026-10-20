import { ParseBudget, ParseError } from "./model.js";
const integerWidths = {
    u8: 1,
    i8: 1,
    u16: 2,
    i16: 2,
    u32: 4,
    i32: 4,
};
/** A forward-only cursor. A child reader can never widen its parent's range. */
export class BinaryReader {
    bytes;
    sourceId;
    budget;
    start;
    end;
    position;
    constructor(bytes, sourceId, budget = new ParseBudget(), start = 0, end = bytes.length) {
        this.bytes = bytes;
        this.sourceId = sourceId;
        this.budget = budget;
        this.start = start;
        this.end = end;
        if (!Number.isSafeInteger(start) ||
            !Number.isSafeInteger(end) ||
            start < 0 ||
            end < start ||
            end > bytes.length) {
            throw new RangeError("Invalid binary reader range");
        }
        this.position = start;
    }
    get offset() {
        return this.position;
    }
    get remaining() {
        return this.end - this.position;
    }
    range(offset = this.position, length = 0) {
        return { sourceId: this.sourceId, offset, length };
    }
    require(length) {
        this.budget.step(this.range());
        if (!Number.isSafeInteger(length) || length < 0 || length > this.remaining) {
            throw new ParseError("malformed", `Expected ${length} bytes; ${this.remaining} remain`, this.range(this.position, this.remaining));
        }
    }
    peekUInt8(relativeOffset = 0) {
        this.require(relativeOffset + 1);
        if (relativeOffset < 0) {
            throw new RangeError("A peek cannot precede the cursor");
        }
        return this.bytes[this.position + relativeOffset];
    }
    integer(type) {
        const width = integerWidths[type];
        this.require(width);
        let value = 0;
        for (let index = width - 1; index >= 0; index--) {
            value = value * 256 + this.bytes[this.position + index];
        }
        this.position += width;
        const modulus = 2 ** (width * 8);
        return type.startsWith("i") && value >= modulus / 2 ? value - modulus : value;
    }
    integer64(signed) {
        this.require(8);
        let value = 0n;
        for (let index = 7; index >= 0; index--) {
            value = (value << 8n) | BigInt(this.bytes[this.position + index]);
        }
        this.position += 8;
        return signed ? BigInt.asIntN(64, value) : value;
    }
    floatingPoint(width) {
        const buffer = new ArrayBuffer(width);
        const view = new DataView(buffer);
        this.require(width);
        for (let index = 0; index < width; index++) {
            view.setUint8(index, this.bytes[this.position + index]);
        }
        this.position += width;
        return width === 4 ? view.getFloat32(0, true) : view.getFloat64(0, true);
    }
    take(length) {
        this.require(length);
        const result = this.bytes.slice(this.position, this.position + length);
        this.position += length;
        return Array.from(result);
    }
    skip(length) {
        this.require(length);
        this.position += length;
    }
    child(length) {
        this.require(length);
        const result = new BinaryReader(this.bytes, this.sourceId, this.budget, this.position, this.position + length);
        this.position += length;
        return result;
    }
    /** ASCII's replacement rule is deliberate; bytes are not decoded as UTF-8. */
    ascii(length) {
        return this.take(length)
            .map((value) => (value < 128 ? String.fromCharCode(value) : "?"))
            .join("");
    }
    utf16(length) {
        this.require(length);
        if (length % 2 !== 0) {
            throw new ParseError("malformed", "UTF-16 has an incomplete code unit", this.range());
        }
        const characters = [];
        for (let index = 0; index < length; index += 2) {
            characters.push(String.fromCharCode(this.integer("u16")));
        }
        return characters.join("");
    }
    terminatedString(wide) {
        const characters = [];
        for (;;) {
            const value = this.integer(wide ? "u16" : "u8");
            if (value === 0) {
                return characters.join("");
            }
            characters.push(wide || value < 128 ? String.fromCharCode(value) : "?");
        }
    }
    line() {
        const characters = [];
        for (;;) {
            const value = this.integer("u8");
            if (value === 13 && this.peekUInt8() === 10) {
                this.skip(1);
                return characters.join("");
            }
            characters.push(value < 128 ? String.fromCharCode(value) : "?");
        }
    }
    guid() {
        const hex = (value, digits) => value.toString(16).padStart(digits, "0");
        const first = hex(this.integer("u32"), 8);
        const second = hex(this.integer("u16"), 4);
        const third = hex(this.integer("u16"), 4);
        const tail = this.take(8).map((value) => hex(value, 2));
        return `${first}-${second}-${third}-${tail.slice(0, 2).join("")}-${tail.slice(2).join("")}`;
    }
}
