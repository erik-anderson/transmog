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
  units?: string;
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
  readonly budgets?: readonly ParserBudgetReport[];
}

export interface ParserBudgetReport {
  readonly phase: string;
  readonly inputBytes: number;
  readonly work: number;
  readonly maximumWork: number;
  readonly fields: number;
  readonly maximumFields: number;
  readonly peakDepth: number;
  readonly maximumDepth: number;
  readonly reads: number;
  readonly loops: number;
  readonly decodedBytes: number;
  readonly encodedBytes: number;
  readonly expandedBytes: number;
  readonly largest?: {
    readonly name: string;
    readonly range: ByteRange;
    readonly work: number;
  };
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
    readonly reason: "invalid" | "truncated" = "invalid",
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
  private peakDepth = 0;
  private inputBytes = 0;
  private encodedBytes = 0;
  private expandedBytes = 0;
  private readonly categories = { reads: 0, fields: 0, loops: 0, decodedBytes: 0 };
  private readonly scopes: { name: string; range: ByteRange; start: number; children: number }[] =
    [];
  private largest: ParserBudgetReport["largest"];

  constructor(
    readonly maximumFields = 50_000,
    readonly maximumDepth = 48,
    readonly maximumWork = 1_000_000,
    readonly phase = "Parser",
  ) {}

  field(range: ByteRange): void {
    if (++this.fields > this.maximumFields) {
      throw new ParseError("limit", "Parsed field limit exceeded", range);
    }
    this.step(range, 1, "fields");
  }

  step(range: ByteRange, units = 1, category: keyof ParseBudget["categories"] = "reads"): void {
    if (!Number.isSafeInteger(units) || units < 1) {
      throw new RangeError("Invalid parser work count");
    }
    const charged = Math.min(units, this.maximumWork - this.work);
    this.work += charged;
    this.categories[category] += charged;
    if (charged !== units) {
      const operation = this.scopes.at(-1)?.name ?? "binary reader";
      throw new ParseError(
        "limit",
        `${this.phase} work limit exceeded (${this.work}/${this.maximumWork} units; ${operation})`,
        range,
      );
    }
  }

  input(bytes: number): void {
    this.inputBytes += bytes;
  }

  transformed(encoded: number, expanded: number): void {
    this.encodedBytes += encoded;
    this.expandedBytes += expanded;
  }

  track<T>(name: string, range: ByteRange, parse: () => T): T {
    const scope = { name, range, start: this.work, children: 0 };
    this.scopes.push(scope);
    try {
      return parse();
    } finally {
      this.scopes.pop();
      const total = this.work - scope.start,
        own = total - scope.children;
      const parent = this.scopes.at(-1);
      if (parent) {
        parent.children += total;
      }
      if (!this.largest || own > this.largest.work) {
        this.largest = { name, range, work: own };
      }
    }
  }

  report(): ParserBudgetReport {
    return {
      phase: this.phase,
      inputBytes: this.inputBytes,
      work: this.work,
      maximumWork: this.maximumWork,
      fields: this.fields,
      maximumFields: this.maximumFields,
      peakDepth: this.peakDepth,
      maximumDepth: this.maximumDepth,
      reads: this.categories.reads,
      loops: this.categories.loops,
      decodedBytes: this.categories.decodedBytes,
      encodedBytes: this.encodedBytes,
      expandedBytes: this.expandedBytes,
      ...(this.largest ? { largest: this.largest } : {}),
    };
  }

  enter(range: ByteRange): void {
    if (++this.depth > this.maximumDepth) {
      this.depth--;
      throw new ParseError("limit", "Structure nesting limit exceeded", range);
    }
    this.peakDepth = Math.max(this.peakDepth, this.depth);
  }

  leave(): void {
    this.depth--;
  }
}
