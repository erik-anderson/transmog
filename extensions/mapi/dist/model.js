export class ParseError extends Error {
    kind;
    range;
    constructor(kind, message, range) {
        super(message);
        this.kind = kind;
        this.range = range;
        this.name = "ParseError";
    }
}
/** Every reader in a decode shares these counters, including derived buffers. */
export class ParseBudget {
    maximumFields;
    maximumDepth;
    maximumWork;
    fields = 0;
    depth = 0;
    work = 0;
    constructor(maximumFields = 50_000, maximumDepth = 48, maximumWork = 100_000) {
        this.maximumFields = maximumFields;
        this.maximumDepth = maximumDepth;
        this.maximumWork = maximumWork;
    }
    field(range) {
        if (++this.fields > this.maximumFields) {
            throw new ParseError("limit", "Parsed field limit exceeded", range);
        }
        this.step(range);
    }
    step(range) {
        if (++this.work > this.maximumWork) {
            throw new ParseError("limit", "Parser work limit exceeded", range);
        }
    }
    enter(range) {
        if (++this.depth > this.maximumDepth) {
            this.depth--;
            throw new ParseError("limit", "Structure nesting limit exceeded", range);
        }
    }
    leave() {
        this.depth--;
    }
}
