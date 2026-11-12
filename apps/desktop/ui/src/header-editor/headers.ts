export interface HeaderField {
  name: string;
  value: string;
}

export class HeaderValidationError extends Error {
  readonly line: number;
  readonly field: "name" | "value";

  constructor(message: string, line: number, field: "name" | "value" = "name") {
    super(message);
    this.name = "HeaderValidationError";
    this.line = line;
    this.field = field;
  }
}

/** Validate every authored row, including an empty row left by an edit. */
export function validateHeaderFields(headers: readonly HeaderField[]): void {
  for (const [index, header] of headers.entries()) {
    const row = index + 1;
    if (!header.name) {
      throw new HeaderValidationError(`Header ${row}: enter a name or remove this row.`, row);
    }
    if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(header.name)) {
      throw new HeaderValidationError(
        `Header ${row}: names must use HTTP token characters, without spaces or a colon.`,
        row,
      );
    }
    // eslint-disable-next-line no-control-regex -- HTTP values allow tab but not other control bytes.
    if (/[\x00-\x08\x0a-\x1f\x7f]/.test(header.value)) {
      throw new HeaderValidationError(
        `Header ${row}: the value contains a newline or another invalid control character.`,
        row,
        "value",
      );
    }
  }
}

/** Parse bulk text without dropping malformed nonempty lines or merging duplicates. */
export function parseHeaderText(text: string): HeaderField[] {
  const headers: HeaderField[] = [];
  for (const [index, line] of text.split(/\r?\n/).entries()) {
    if (/^[ \t]*$/.test(line)) {
      continue;
    }
    const separator = line.indexOf(":");
    if (separator <= 0) {
      throw new HeaderValidationError(`Line ${index + 1}: use Name: value.`, index + 1);
    }
    const header = {
      name: line.slice(0, separator),
      // The first space is the conventional Name: value separator. Preserve
      // additional whitespace in the authored value, including trailing tabs.
      value: line.slice(separator + 1).replace(/^ /, ""),
    };
    try {
      validateHeaderFields([header]);
    } catch (error) {
      if (error instanceof HeaderValidationError) {
        throw new HeaderValidationError(
          error.message.replace(/^Header 1:/, `Line ${index + 1}:`),
          index + 1,
          error.field,
        );
      }
      throw error;
    }
    headers.push(header);
  }
  return headers;
}

export function formatHeaderText(headers: readonly HeaderField[]): string {
  return headers.map((header) => `${header.name}: ${header.value}`).join("\n");
}
