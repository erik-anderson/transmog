import { invoke } from "@tauri-apps/api/core";
import {
  CopyError,
  COPY_TEXT_BYTES,
  copyScalar,
  copyTree,
  type CopyTreeNode,
} from "../inspection-copy.js";
import { decodeBytes } from "../hex-viewer/hex-formats.js";
import {
  DOCUMENT_NODE,
  documentNode,
  nodeIndex,
  valueText,
  type VisualizerNode,
  type VisualizerView,
} from "./models.js";

export function hasCopyValue(node: VisualizerNode): boolean {
  return !!node.value && !["object", "array"].includes(node.value.kind);
}
/** Reads only declared byte-value ranges, never a containing message by default. */
export async function nodeCopyValue(
  node: VisualizerNode,
  view: VisualizerView,
  current: () => boolean,
  budget = COPY_TEXT_BYTES,
): Promise<string> {
  if (node.value?.kind !== "bytes") {
    return valueText(node.value);
  }
  if (!node.spans.length) {
    throw new CopyError("The byte value has no retained source range.");
  }
  const pieces: string[] = [];
  let size = 0;
  for (const span of node.spans) {
    let offset = BigInt(span.offset),
      left = BigInt(span.length);
    if (left * 3n + BigInt(size) > BigInt(budget)) {
      throw new CopyError("The byte value exceeds the clipboard text limit.");
    }
    const derived = view.document.sources.find((source) => source.id === span.sourceId);
    const inline = derived ? decodeBytes(derived.bytesBase64) : null;
    if (inline && offset + left > BigInt(inline.length)) {
      throw new CopyError("The byte value is unavailable.");
    }
    while (left) {
      if (!current()) {
        throw new CopyError("The inspection changed. Copy again from the current value.");
      }
      const count = Number(left < 65536n ? left : 65536n);
      const bytes = inline
        ? inline.subarray(Number(offset), Number(offset) + count)
        : decodeBytes(
            (
              await invoke<{ bytesBase64: string; offset: string }>("visualizer_source", {
                handle: view.handle,
                request: { sourceId: span.sourceId, offset: offset.toString(), maxBytes: count },
              }).then((page) => {
                if (page.offset !== offset.toString()) {
                  throw new CopyError("The returned byte range changed.");
                }
                return page;
              })
            ).bytesBase64,
          );
      if (bytes.length !== count) {
        throw new CopyError("The complete byte value is unavailable.");
      }
      const text = Array.from(bytes, (byte) =>
        byte.toString(16).padStart(2, "0").toUpperCase(),
      ).join(" ");
      pieces.push(text);
      size += text.length + 1;
      offset += BigInt(count);
      left -= BigInt(count);
    }
  }
  return pieces.join(" ");
}
export async function visualizerCopyTree(
  id: string,
  initial: VisualizerView,
  current: () => boolean,
  expanded: (view: VisualizerView) => void,
): Promise<string> {
  let view = initial,
    expansions = 0;
  const deadline = performance.now() + 30000;
  const checked = (): boolean => current() && performance.now() < deadline;
  let changed = false;
  const visited = new Set<string>();
  try {
    while (true) {
      if (!checked()) {
        throw new CopyError("The subtree copy expired or its inspection changed.");
      }
      const root = nodeIndex(documentNode(view.document)).get(id);
      if (!root) {
        throw new CopyError("The subtree is no longer available.");
      }
      const pending = [...nodeIndex(root).values()].find((node) => node.childrenToken);
      if (!pending) {
        break;
      }
      // A producer may append children under a stable node and reuse its token.
      // Reject a repeated load state, while allowing additional materialization.
      const state = JSON.stringify([pending.id, pending.childrenToken, nodeIndex(pending).size]);
      if (visited.has(state)) {
        throw new CopyError(
          "This subtree loads in ranges. Copy a smaller field range or use the source bytes in Hex.",
        );
      }
      visited.add(state);
      if (++expansions > 256) {
        throw new CopyError("The subtree exceeds the deferred expansion limit.");
      }
      view = await invoke<VisualizerView>("expand_visualizer", {
        handle: view.handle,
        token: pending.childrenToken,
      });
      changed = true;
    }
    const root =
      id === DOCUMENT_NODE
        ? documentNode(view.document)
        : nodeIndex(documentNode(view.document)).get(id)!;
    let remaining = COPY_TEXT_BYTES,
      nodes = 0;
    const convert = async (node: VisualizerNode, depth = 0): Promise<CopyTreeNode> => {
      if (!checked()) {
        throw new CopyError("The subtree copy expired or its inspection changed.");
      }
      if (++nodes > 10000 || depth > 32) {
        throw new CopyError("The subtree exceeds the copy depth or node limit.");
      }
      const value = hasCopyValue(node) ? await nodeCopyValue(node, view, checked, remaining) : null;
      remaining -=
        new TextEncoder().encode(
          copyScalar(node.name) + (value == null ? "" : ": " + copyScalar(value)),
        ).length +
        depth * 2 +
        1;
      if (remaining < 0) {
        throw new CopyError("The subtree exceeds the clipboard text limit.");
      }
      return {
        name: node.name,
        value,
        children: await sequential(node.children, (child) => convert(child, depth + 1)),
      };
    };
    return copyTree(await convert(root));
  } finally {
    // Expansion tokens may be single-use. Retain materialized data while keeping the
    // existing expansion set, selection, view mode and scroll position unchanged.
    if (changed && current()) {
      expanded(view);
    }
  }
}
async function sequential<T, R>(items: T[], read: (item: T) => Promise<R>): Promise<R[]> {
  const result: R[] = [];
  for (const item of items) {
    result.push(await read(item));
  }
  return result;
}
