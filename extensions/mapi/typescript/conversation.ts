import type { PropertyTag } from "./properties.js";

export interface RopRequest {
  readonly operation: string;
  readonly logonId: number;
  readonly parameters: ReadonlyMap<string, number>;
  readonly handle: number | undefined;
  tags?: readonly PropertyTag[];
}

/** State is supplied by replaying only the host-granted conversation snapshot. */
export class Conversation {
  readonly logons = new Map<number, number>();
  readonly columns = new Map<number, readonly PropertyTag[]>();
  private readonly requests = new Map<string, RopRequest[]>();
  private readonly responsePositions = new Map<string, number>();

  beginExchange(): void {
    this.requests.clear();
    this.responsePositions.clear();
  }

  rememberRequest(request: RopRequest): void {
    const queue = this.requests.get(request.operation) ?? [];
    queue.push(request);
    this.requests.set(request.operation, queue);
  }

  requestFor(operation: string): RopRequest | undefined {
    const position = this.responsePositions.get(operation) ?? 0;
    this.responsePositions.set(operation, position + 1);
    return this.requests.get(operation)?.[position];
  }
}

/** A missing fixed-layout field is an authoring error, never a wire value of zero. */
export function requestParameter(request: RopRequest, name: string): number {
  const value = request.parameters.get(name);
  if (value === undefined) {
    throw new Error(`Missing ${request.operation} request parameter: ${name}`);
  }
  return value;
}
