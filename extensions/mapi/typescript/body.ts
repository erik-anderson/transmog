/** API 1 entry points are synchronous local functions. The host owns this worker's lifetime. */
import type {
  BytePage,
  ContextPage,
  HttpDecodeResult,
  Input,
  Step,
  TrafficEvent,
  TrafficBatchInfo,
} from "transmog:extensions/v1";
import { BodyVisualizer } from "./body-session.js";

let active: BodyVisualizer | undefined;
let generation = 0;

function session(): BodyVisualizer {
  if (!active) {
    throw new Error("No active MAPI visualizer");
  }
  return active;
}

export function visualize(input: Input): Step {
  active = new BodyVisualizer(input, ++generation);
  return active.start();
}

export function resume(token: string, pages: readonly BytePage[]): Step {
  return session().resume(token, pages);
}

export function resumeDecoded(token: string, result: HttpDecodeResult): Step {
  return session().resumeDecoded(token, result);
}

export function resumeContext(token: string, page: ContextPage): Step {
  return session().resumeContext(token, page);
}

export function expand(token: string): Step {
  return session().expand(token);
}

export function onTraffic(batch: readonly TrafficEvent[], info: TrafficBatchInfo): Step {
  return session().onTraffic(batch, info);
}
