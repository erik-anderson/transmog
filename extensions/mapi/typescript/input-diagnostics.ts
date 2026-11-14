import type { Diagnostic } from "transmog:extensions/v1";

const inputLimitCodes = new Set([
  "input-limit",
  "context-limit",
  "live-context-limit",
  "http-decoding-limited",
]);

/** Input safeguards have the same result status during acquisition and presentation. */
export function isInputLimit(issue: Diagnostic): boolean {
  return inputLimitCodes.has(issue.code);
}
