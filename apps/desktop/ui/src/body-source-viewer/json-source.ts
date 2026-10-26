import {
  COPY_TEXT_BYTES,
  CopyError,
  copyScalar,
  copyTree,
  fieldCopyActions,
  nameCopyAction,
  type CopyAction,
  type CopyTreeNode,
} from "../inspection-copy.js";
import type { StructuredSnapshot, Value, VisualizerNode } from "../extension-visualizer/models.js";

export type JsonKind = "object" | "array" | "string" | "number" | "boolean" | "null";
export interface JsonSourceNode {
  id: string;
  name: string;
  named: boolean;
  keyToken: string | null;
  nameStart: number;
  start: number;
  end: number;
  kind: JsonKind;
  /** Decoded strings and exact literal tokens; never rounded through Number. */
  scalar: string | null;
  pointer: string | null;
  children: JsonSourceNode[];
}
export interface JsonSourceDocument {
  source: string;
  root: JsonSourceNode;
  nodes: Map<string, JsonSourceNode>;
  prefixLength: number;
}
export class JsonSourceError extends Error {
  constructor(readonly reason: "invalid" | "limit") {
    super(reason);
  }
}
const pointerPart = (value: string): string => value.replaceAll("~", "~0").replaceAll("/", "~1");

/** A bounded JSON token parser. Object occurrences/order and literal spelling survive. */
export function parseJsonSource(source: string): JsonSourceDocument {
  if (
    source.length > COPY_TEXT_BYTES ||
    new TextEncoder().encode(source).length > COPY_TEXT_BYTES
  ) {
    throw new JsonSourceError("limit");
  }
  const prefixLength = source.startsWith(")]}'\n") ? 5 : source.startsWith(")]}',\n") ? 6 : 0;
  let offset = prefixLength;
  const nodes = new Map<string, JsonSourceNode>();
  const invalid = (): never => {
    throw new JsonSourceError("invalid");
  };
  const whitespace = (): void => {
    while (offset < source.length && /[\x20\t\r\n]/.test(source[offset]!)) {
      offset++;
    }
  };
  const stringToken = (): { token: string; value: string } => {
    const start = offset;
    if (source[offset++] !== '"') {
      invalid();
    }
    while (offset < source.length) {
      const character = source[offset++]!;
      if (character === '"') {
        const token = source.slice(start, offset);
        // Only a single, grammar-checked string token is decoded with JSON.parse.
        return { token, value: JSON.parse(token) as string };
      }
      if (character.charCodeAt(0) < 0x20) {
        invalid();
      }
      if (character === "\\") {
        const escape = source[offset++];
        if (escape === "u") {
          if (!/^[\da-fA-F]{4}$/.test(source.slice(offset, offset + 4))) {
            invalid();
          }
          offset += 4;
        } else if (!escape || !'"\\/bfnrt'.includes(escape)) {
          invalid();
        }
      }
    }
    return invalid();
  };
  const parse = (
    depth: number,
    name: string,
    named: boolean,
    keyToken: string | null,
    nameStart: number,
  ): JsonSourceNode => {
    if (depth > 32 || nodes.size >= 10000) {
      throw new JsonSourceError("limit");
    }
    whitespace();
    const start = offset;
    const node: JsonSourceNode = {
      id: "json-" + nodes.size,
      name,
      named,
      keyToken,
      nameStart,
      start,
      end: start,
      kind: "null",
      scalar: null,
      pointer: null,
      children: [],
    };
    nodes.set(node.id, node);
    const character = source[offset];
    if (character === "{" || character === "[") {
      node.kind = character === "{" ? "object" : "array";
      const close = character === "{" ? "}" : "]";
      offset++;
      whitespace();
      if (source[offset] !== close) {
        for (;;) {
          whitespace();
          const nameAt = offset;
          let childName = "[" + node.children.length + "]",
            key: string | null = null;
          if (node.kind === "object") {
            const token = stringToken();
            childName = token.value;
            key = token.token;
            whitespace();
            if (source[offset++] !== ":") {
              invalid();
            }
          }
          node.children.push(parse(depth + 1, childName, true, key, nameAt));
          whitespace();
          if (source[offset] === close) {
            break;
          }
          if (source[offset++] !== ",") {
            invalid();
          }
        }
      }
      if (source[offset++] !== close) {
        invalid();
      }
    } else if (character === '"') {
      node.kind = "string";
      node.scalar = stringToken().value;
    } else {
      const rest = source.slice(offset, offset + 6);
      const literal = rest.startsWith("true")
        ? "true"
        : rest.startsWith("false")
          ? "false"
          : rest.startsWith("null")
            ? "null"
            : null;
      if (literal) {
        node.kind = literal === "null" ? "null" : "boolean";
        node.scalar = literal;
        offset += literal.length;
      } else {
        const match = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(source.slice(offset));
        if (!match) {
          invalid();
        }
        node.kind = "number";
        node.scalar = match![0];
        offset += match![0].length;
      }
    }
    node.end = offset;
    return node;
  };
  const root = parse(0, "JSON", false, null, prefixLength);
  whitespace();
  if (offset !== source.length) {
    invalid();
  }
  const pointers = (node: JsonSourceNode, pointer: string | null): void => {
    node.pointer = pointer;
    const counts = new Map<string, number>();
    if (node.kind === "object") {
      for (const child of node.children) {
        counts.set(child.name, (counts.get(child.name) ?? 0) + 1);
      }
    }
    for (const [index, child] of node.children.entries()) {
      const part = node.kind === "array" ? String(index) : pointerPart(child.name);
      // Clipboard Unicode cannot retain unpaired surrogates or embedded NUL,
      // and operating systems can normalize line-ending controls.
      // eslint-disable-next-line no-control-regex -- Only offer lossless plain pointers.
      const uncopyable = /[\u0000-\u001f\ud800-\udfff]/u.test(child.name);
      pointers(
        child,
        pointer === null || counts.get(child.name)! > 1 || uncopyable ? null : pointer + "/" + part,
      );
    }
  };
  pointers(root, "");
  return { source, root, nodes, prefixLength };
}

/** Indentation changes whitespace only; keys, duplicate occurrences and tokens stay exact. */
export function formatJsonSource(document: JsonSourceDocument): string {
  let length = 0;
  const pieces: string[] = [];
  const add = (text: string): void => {
    length += text.length;
    if (length > COPY_TEXT_BYTES) {
      throw new JsonSourceError("limit");
    }
    pieces.push(text);
  };
  const render = (node: JsonSourceNode, depth: number): void => {
    if (node.kind !== "object" && node.kind !== "array") {
      add(document.source.slice(node.start, node.end));
      return;
    }
    const object = node.kind === "object";
    add(object ? "{" : "[");
    for (const [index, child] of node.children.entries()) {
      add((index ? ",\n" : "\n") + "  ".repeat(depth + 1));
      if (object) {
        add(child.keyToken! + ": ");
      }
      render(child, depth + 1);
    }
    if (node.children.length) {
      add("\n" + "  ".repeat(depth));
    }
    add(object ? "}" : "]");
  };
  render(document.root, 0);
  const output = pieces.join("");
  if (new TextEncoder().encode(output).length > COPY_TEXT_BYTES) {
    throw new JsonSourceError("limit");
  }
  return output;
}

export function jsonCopyActions(document: JsonSourceDocument, node: JsonSourceNode): CopyAction[] {
  const scalar = node.kind !== "object" && node.kind !== "array";
  const actions = node.named
    ? scalar
      ? fieldCopyActions(node.name, () => node.scalar!)
      : [nameCopyAction(node.name)]
    : scalar
      ? [{ id: "value", label: "Copy value", read: () => copyScalar(node.scalar!) }]
      : [];
  if (!scalar) {
    const tree = (item: JsonSourceNode): CopyTreeNode => ({
      name: item.name,
      value: item.scalar,
      children: item.children.map(tree),
    });
    actions.push({ id: "subtree", label: "Copy subtree", read: () => copyTree(tree(node)) });
  }
  actions.push({
    id: "json",
    label: "Copy as JSON",
    read: () => document.source.slice(node.start, node.end),
  });
  if (node.pointer !== null) {
    actions.push({ id: "pointer", label: "Copy JSON Pointer", read: () => node.pointer! });
  }
  return actions;
}

/** A pointer target is a field's key or value; whitespace targets its container. */
export function jsonNodeAt(document: JsonSourceDocument, offset: number): JsonSourceNode | null {
  const visit = (node: JsonSourceNode): JsonSourceNode | null => {
    if (offset < Math.min(node.nameStart, node.start) || offset >= node.end) {
      return null;
    }
    for (const child of node.children) {
      const found = visit(child);
      if (found) {
        return found;
      }
    }
    return node;
  };
  return visit(document.root);
}

export function jsonSnapshot(
  document: JsonSourceDocument,
  id: string,
  showSource: (id: string) => void,
): StructuredSnapshot {
  const convert = (node: JsonSourceNode): VisualizerNode => {
    let value: Value;
    if (node.kind === "object" || node.kind === "array") {
      value = { kind: node.kind, count: node.children.length };
    } else if (node.kind === "boolean") {
      value = { kind: "boolean", value: node.scalar === "true" };
    } else if (node.kind === "null") {
      value = { kind: "null" };
    } else if (node.kind === "number") {
      value = { kind: "number", value: node.scalar! };
    } else {
      value = { kind: "string", value: node.scalar! };
    }
    return {
      id: node.id,
      name: node.name,
      value,
      format: { protocol: "JSON " + node.kind },
      spans: [],
      children: node.children.map(convert),
    };
  };
  return {
    id,
    document: {
      title: "JSON fields",
      status: "Read only",
      presentation: "tree",
      summary: document.nodes.size.toLocaleString() + " JSON values",
      roots: [convert(document.root)],
      sources: [],
      diagnostics: [],
    },
    nodePresentation: (nodeId) => {
      const node = document.nodes.get(nodeId);
      if (!node) {
        return;
      }
      const blank = (text: string, label: string): string | undefined => {
        if (!text) {
          return `"" (empty ${label})`;
        }
        if (/^\s+$/u.test(text)) {
          const preview = JSON.stringify(text.slice(0, 32)).replace(
            /\s/gu,
            (character) => "\\u" + character.charCodeAt(0).toString(16).padStart(4, "0"),
          );
          return `${preview}${text.length > 32 ? "…" : ""} (whitespace ${label}, ${text.length} ${text.length === 1 ? "character" : "characters"})`;
        }
        return;
      };
      return {
        name:
          blank(node.name, "name") ??
          (/["\\\p{Cc}]/u.test(node.name) ? JSON.stringify(node.name) : node.name),
        ...(node.kind === "string"
          ? { value: blank(node.scalar!, "string") ?? document.source.slice(node.start, node.end) }
          : {}),
      };
    },
    copyActions: (nodeId) => {
      const node = document.nodes.get(nodeId) ?? (nodeId === "$document" ? document.root : null);
      if (!node) {
        throw new CopyError("The JSON field is unavailable.");
      }
      return jsonCopyActions(document, node);
    },
    showSource,
    about: "Built-in JSON fields",
  };
}
