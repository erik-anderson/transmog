import { ParseError } from "./parse-error.js";
import { MAX_TEXT_PREVIEW_UNITS } from "./limits.js";

const BITS_PER_BYTE = 8;
const ASCII_MAX = 0x7f;
const HIGH_SURROGATE_FIRST = 0xd800;
const HIGH_SURROGATE_LAST = 0xdbff;
const LOW_SURROGATE_FIRST = 0xdc00;
const LOW_SURROGATE_LAST = 0xdfff;
const SURROGATE_PAYLOAD_BITS = 10;
const SUPPLEMENTARY_FIRST = 0x10000;
const UNICODE_MAX = 0x10ffff;
const CONTINUATION_MASK = 0xc0;
const CONTINUATION_TAG = 0x80;
const CONTINUATION_DATA_MASK = 0x3f;
const CONTINUATION_DATA_BITS = 6;
// UTF-8 lead ranges exclude overlong two-byte sequences and values above U+10FFFF.
const UTF8_LEADS = [
  { first: 0xc2, last: 0xdf, tail: 1, mask: 0x1f, minimum: 0x80 },
  { first: 0xe0, last: 0xef, tail: 2, mask: 0x0f, minimum: 0x800 },
  { first: 0xf0, last: 0xf4, tail: 3, mask: 0x07, minimum: SUPPLEMENTARY_FIRST },
] as const;

type ScanMode = "validate" | "preview" | "validate-and-preview";

/** No DOM APIs are needed in the worker. Preview-only is for already validated bytes. */
export function scanText(
  bytes: Uint8Array,
  start: number,
  end: number,
  wide: boolean,
  mode: ScanMode,
): { value: string; clipped: boolean } {
  let offset = start;
  let value = "";
  let clipped = false;
  const next = (): number => {
    if (offset >= end) {
      throw new ParseError("malformed", "Truncated Unicode character", offset);
    }
    return bytes[offset++]!;
  };
  while (offset < end) {
    const at = offset;
    let point = next();
    if (wide) {
      point |= next() << BITS_PER_BYTE;
      if (point >= HIGH_SURROGATE_FIRST && point <= HIGH_SURROGATE_LAST) {
        const low = next() | (next() << BITS_PER_BYTE);
        if (low < LOW_SURROGATE_FIRST || low > LOW_SURROGATE_LAST) {
          throw new ParseError("malformed", "Invalid UTF-16 surrogate pair", at);
        }
        point =
          SUPPLEMENTARY_FIRST +
          ((point - HIGH_SURROGATE_FIRST) << SURROGATE_PAYLOAD_BITS) +
          low -
          LOW_SURROGATE_FIRST;
      } else if (point >= LOW_SURROGATE_FIRST && point <= LOW_SURROGATE_LAST) {
        throw new ParseError("malformed", "Unpaired UTF-16 surrogate", at);
      }
    } else if (point > ASCII_MAX) {
      const lead = UTF8_LEADS.find((range) => point >= range.first && point <= range.last);
      if (!lead) {
        throw new ParseError("malformed", "Invalid UTF-8 leading byte", at);
      }
      point &= lead.mask;
      for (let index = 0; index < lead.tail; index++) {
        const byte = next();
        if ((byte & CONTINUATION_MASK) !== CONTINUATION_TAG) {
          throw new ParseError("malformed", "Invalid UTF-8 continuation", at);
        }
        point = (point << CONTINUATION_DATA_BITS) | (byte & CONTINUATION_DATA_MASK);
      }
      if (
        point < lead.minimum ||
        point > UNICODE_MAX ||
        (point >= HIGH_SURROGATE_FIRST && point <= LOW_SURROGATE_LAST)
      ) {
        throw new ParseError("malformed", "Invalid UTF-8 code point", at);
      }
    }
    if (mode !== "validate" && !clipped) {
      const character = String.fromCodePoint(point);
      if (value.length + character.length <= MAX_TEXT_PREVIEW_UNITS) {
        value += character;
      } else {
        clipped = true;
        if (mode === "preview") {
          return { value, clipped };
        }
      }
    }
  }
  return { value, clipped };
}
