import { DecodeSession } from "./decoder-state.js";
import type { DecodeResult, Direction, Exchange } from "./model.js";

/** Decode only an explicitly granted, already transport-decoded snapshot. */
export function decode(
  exchanges: readonly Exchange[],
  selectedId: number,
  direction: Direction,
): DecodeResult {
  const session = new DecodeSession(exchanges, selectedId, direction);
  session.prepare(direction === "response");
  const record = session.record(`${session.selected.requestType} ${direction}`);
  return session.capture(record, () => {
    session.readMessage(record, session.selected.requestType, direction);
  });
}
