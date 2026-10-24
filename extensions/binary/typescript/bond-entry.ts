import type { BytePage, HttpDecodeResult, Input, Step } from "transmog:extensions/v1";
import { BondIndexViewer } from "./bond-index.js";
import { visualizer } from "./runtime.js";

const indexed = new BondIndexViewer();
const viewer = visualizer(
  "Bond Compact Binary",
  (bytes, sourceId) => indexed.parse(bytes, sourceId),
  16 * 1024 * 1024,
);
let incompleteEvidence = false;
export function visualize(input: Input): Step {
  indexed.reset();
  incompleteEvidence = input[input.direction]?.body?.availability !== "complete";
  return viewer.visualize(input);
}
export function resume(token: string, pages: readonly BytePage[]): Step {
  return viewer.resume(token, pages);
}
export function resumeDecoded(token: string, result: HttpDecodeResult): Step {
  return viewer.resumeDecoded(token, result);
}
export function expand(token: string): Step {
  const document = indexed.expand(token);
  if (incompleteEvidence) {
    if (document.status !== "limited") {
      document.status = "partial";
    }
    document.diagnostics!.push({
      code: "incomplete-evidence",
      message:
        "The body is incomplete. Only retained bytes are shown; absent fields and message boundaries cannot be inferred.",
    });
  }
  return { kind: "document", document };
}
