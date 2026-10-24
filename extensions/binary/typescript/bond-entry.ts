import type { BytePage, HttpDecodeResult, Input, Step } from "transmog:extensions/v1";
import { parse } from "./bond.js";
import { visualizer } from "./runtime.js";

const viewer = visualizer("Bond Compact Binary", parse);
export function visualize(input: Input): Step {
  return viewer.visualize(input);
}
export function resume(token: string, pages: readonly BytePage[]): Step {
  return viewer.resume(token, pages);
}
export function resumeDecoded(token: string, result: HttpDecodeResult): Step {
  return viewer.resumeDecoded(token, result);
}
