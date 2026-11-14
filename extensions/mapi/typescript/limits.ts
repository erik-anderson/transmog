import { ParseError } from "./model.js";
import type { ByteRange } from "./model.js";

const mebibyte = 1024 * 1024;

/** Local safeguards keep useful partial results below the host's hard limits. */
export const inputLimits = {
  snapshotBytes: 8 * mebibyte,
  relatedExchanges: 64,
  // The standalone decoder also serves fixtures for the host's maximum context window.
  decoderExchanges: 257,
  pageBytes: 65_536,
  contextPageExchanges: 32,
} as const;

export const parserLimits = {
  fields: 50_000,
  depth: 48,
  collectionElements: 10_000,
  derivedBytes: 8 * mebibyte,
  derivedSources: 128,
  transferFragments: 64,
} as const;

/** Only structure size is counted; primitive reads and loops need no accounting. */
export class StructureLimits {
  private fields = 0;
  private depth = 0;

  constructor(
    private readonly phase = "Parser",
    private readonly maximumFields: number = parserLimits.fields,
    private readonly maximumDepth: number = parserLimits.depth,
  ) {}

  field(range: ByteRange): void {
    if (this.fields >= this.maximumFields) {
      throw new ParseError("limit", `${this.phase}: parsed field limit exceeded`, range);
    }
    this.fields++;
  }

  enter(range: ByteRange): void {
    if (this.depth >= this.maximumDepth) {
      throw new ParseError("limit", `${this.phase}: structure nesting limit exceeded`, range);
    }
    this.depth++;
  }

  leave(): void {
    this.depth--;
  }
}
