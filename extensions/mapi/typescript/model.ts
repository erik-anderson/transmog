/** Protocol facts, independent of the host UI and the reference inspector. */
export interface ByteRange {
  readonly sourceId: string;
  readonly offset: number;
  readonly length: number;
}

export type FieldValue =
  | { readonly kind: "integer"; readonly value: string }
  | { readonly kind: "boolean"; readonly value: boolean }
  | { readonly kind: "float"; readonly value: number | string }
  | { readonly kind: "string"; readonly value: string }
  | { readonly kind: "guid"; readonly value: string }
  | {
      readonly kind: "enum";
      readonly value: string;
      readonly names: readonly string[];
      readonly flags: boolean;
    }
  | { readonly kind: "bytes" };

export interface ParsedField {
  readonly name: string;
  readonly type: string;
  readonly range: ByteRange;
  readonly children: ParsedField[];
  value?: FieldValue;
  encoding?: string;
  timestamp?: string;
  issue?: ParseIssue;
}

export interface ParseIssue {
  readonly kind: "malformed" | "unsupported" | "missing-context" | "limit";
  readonly message: string;
}

export interface DerivedSource {
  readonly id: string;
  readonly bytes: readonly number[];
  readonly parents: readonly ByteRange[];
  readonly transform: string;
  readonly exactMapping: boolean;
}

export interface DecodeResult {
  readonly root: ParsedField;
  readonly derivedSources: readonly DerivedSource[];
}

export interface MessageBody {
  readonly bytes: readonly number[];
  readonly sourceId: string;
}

export interface Exchange {
  readonly id: number;
  readonly requestType: string;
  readonly request?: MessageBody;
  readonly response?: MessageBody;
}

export type Direction = "request" | "response";

export class ParseError extends Error {
  constructor(
    readonly kind: ParseIssue["kind"],
    message: string,
    readonly range: ByteRange,
  ) {
    super(message);
    this.name = "ParseError";
  }
}

/** Every reader in a decode shares these counters, including derived buffers. */
export class ParseBudget {
  private fields = 0;
  private depth = 0;
  private work = 0;

  constructor(
    readonly maximumFields = 50_000,
    readonly maximumDepth = 48,
    readonly maximumWork = 100_000,
  ) {}

  field(range: ByteRange): void {
    if (++this.fields > this.maximumFields) {
      throw new ParseError("limit", "Parsed field limit exceeded", range);
    }
    this.step(range);
  }

  step(range: ByteRange): void {
    if (++this.work > this.maximumWork) {
      throw new ParseError("limit", "Parser work limit exceeded", range);
    }
  }

  enter(range: ByteRange): void {
    if (++this.depth > this.maximumDepth) {
      this.depth--;
      throw new ParseError("limit", "Structure nesting limit exceeded", range);
    }
  }

  leave(): void {
    this.depth--;
  }
}
