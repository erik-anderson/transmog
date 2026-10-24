import type { BytePage, HttpDecodeResult, Input, Step } from "transmog:extensions/v1";
import { parse } from "./protobuf.js";
import { visualizer } from "./runtime.js";

const viewer = visualizer("Protobuf", parse);
// The host validates synchronous local function declarations before loading code.
export function visualize(input: Input): Step {
  return viewer.visualize(input);
}
export function resume(token: string, pages: readonly BytePage[]): Step {
  return viewer.resume(token, pages);
}
export function resumeDecoded(token: string, result: HttpDecodeResult): Step {
  return viewer.resumeDecoded(token, result);
}
