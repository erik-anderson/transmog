import type { Step, Message, ParseStatus } from "transmog:extensions/v1";

/** Header-only entries import this small module without importing the decoder. */
export type DocumentStep = Extract<Step, { kind: "document" }>;

export const unicode = {
  asciiEnd: 0x80,
  twoByteEnd: 0x800,
  highSurrogateStart: 0xd800,
  lowSurrogateStart: 0xdc00,
  surrogateEnd: 0xe000,
} as const;
const stringChunkLength = 4096;

export const maximumDisplayStringLength = 16_384;

export function header(message: Message | null | undefined, name: string): string | null {
  const fields = (message?.headers ?? []).filter((field) => field.name.toLowerCase() === name);
  if (fields.length === 0 || fields.some((field) => field.redacted || !field.value)) {
    return null;
  }
  return fields.map((field) => latin1(field.value!)).join(", ");
}

export function latin1(bytes: readonly number[]): string {
  let result = "";
  for (let offset = 0; offset < bytes.length; offset += stringChunkLength) {
    result += String.fromCharCode(...bytes.slice(offset, offset + stringChunkLength));
  }
  return result;
}

export function base64(bytes: readonly number[]): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const parts: string[] = [];
  let part = "";
  for (let offset = 0; offset < bytes.length; offset += 3) {
    const first = bytes[offset]!;
    const second = bytes[offset + 1];
    const third = bytes[offset + 2];
    part += alphabet[first >>> 2]! + alphabet[((first & 3) << 4) | ((second ?? 0) >>> 4)]!;
    part += second === undefined ? "=" : alphabet[((second & 15) << 2) | ((third ?? 0) >>> 6)]!;
    part += third === undefined ? "=" : alphabet[third & 63]!;
    if (part.length >= stringChunkLength) {
      parts.push(part);
      part = "";
    }
  }
  parts.push(part);
  return parts.join("");
}

export function notice(title: string, status: ParseStatus, message: string): DocumentStep {
  return {
    kind: "document",
    document: {
      title,
      status,
      presentation: "tree",
      summary: message,
      roots: [],
      diagnostics: [{ code: status, message }],
    },
  };
}
