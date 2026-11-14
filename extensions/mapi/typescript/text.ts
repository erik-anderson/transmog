import { unicode } from "./shared.js";

/** A prefix that fits the host's UTF-8 byte limit without splitting a character. */
export function utf8Prefix(
  value: string,
  maximumBytes: number,
  maximumCodeUnits = value.length,
): string {
  let bytes = 0;
  let end = 0;
  for (const character of value) {
    const code = character.codePointAt(0)!;
    // Readers reject invalid UTF-16. Keep this output boundary safe as well.
    if (code >= unicode.highSurrogateStart && code < unicode.surrogateEnd) {
      break;
    }
    let width: number;
    if (code < unicode.asciiEnd) {
      width = 1;
    } else if (code < unicode.twoByteEnd) {
      width = 2;
    } else if (character.length === 1) {
      width = 3;
    } else {
      width = 4;
    }
    if (bytes + width > maximumBytes || end + character.length > maximumCodeUnits) {
      break;
    }
    bytes += width;
    end += character.length;
  }
  return value.slice(0, end);
}
