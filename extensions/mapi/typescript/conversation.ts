import type { ParsedField } from "./model.js";
import type { PropertyTag } from "./properties.js";

export interface RopRequest {
  readonly operation: string;
  readonly field: ParsedField;
  readonly handle: number | undefined;
  tags?: readonly PropertyTag[];
}

/** State is supplied by replaying only the host-granted conversation snapshot. */
export class Conversation {
  readonly logons = new Map<number, number>();
  readonly columns = new Map<number, readonly PropertyTag[]>();
  readonly requests: RopRequest[] = [];
  private readonly responsePositions = new Map<string, number>();

  beginExchange(): void {
    this.requests.length = 0;
    this.responsePositions.clear();
  }

  requestFor(operation: string): RopRequest | undefined {
    const position = this.responsePositions.get(operation) ?? 0;
    this.responsePositions.set(operation, position + 1);
    return this.requests.filter((request) => request.operation === operation)[position];
  }
}

export function numberField(field: ParsedField, name: string): number {
  const value = field.children.find((child) => child.name === name)?.value;
  if (value?.kind === "boolean") {
    return value.value ? 1 : 0;
  }
  if (value?.kind === "integer" || value?.kind === "enum") {
    return Number(value.value);
  }
  return 0;
}
