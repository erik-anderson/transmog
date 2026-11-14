import type { Node } from "transmog:extensions/v1";

export class ParseError extends Error {
  partial?: Node;
  constructor(
    readonly status: "malformed" | "limited",
    message: string,
    readonly offset: number,
  ) {
    super(`${message} (byte ${offset}).`);
  }
}
