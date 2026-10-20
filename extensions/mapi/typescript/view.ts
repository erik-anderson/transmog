import type { ByteRange, DecodeResult, ParsedField } from "./model.js";
import type {
  Diagnostic,
  DocumentStep,
  HostSource,
  HostValue,
  SourceSpan,
  TreeNode,
} from "./host.js";
import { base64 } from "./shared.js";

const maximumNodes = 18_000;
const maximumTextBytes = 2 * 1024 * 1024;
const maximumDerivedBytes = 2 * 1024 * 1024;
let epoch = 0;

function serializedSize(value: unknown): number {
  const text = JSON.stringify(value);
  let bytes = 0;
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    if (code < 0x80) bytes++;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff) {
      bytes += 4;
      index++;
    } else bytes += 3;
  }
  return bytes;
}

export interface Presentation {
  readonly expanded: Set<string>;
  readonly step: DocumentStep;
  expand(token: string): DocumentStep;
}

function span(range: ByteRange): SourceSpan {
  return { sourceId: range.sourceId, offset: String(range.offset), length: String(range.length) };
}

/** Converts protocol facts to host-owned presentation. It never creates UI. */
export function present(
  decoded: DecodeResult,
  origins: Iterable<readonly [string, number]>,
  title: string,
  diagnostics: readonly Diagnostic[],
  previous?: Presentation,
): Presentation {
  const revision = ++epoch;
  const expanded = new Set(previous?.expanded ?? []);
  const identities = new WeakMap<ParsedField, string>();
  const known = new Map(origins);
  const sources: HostSource[] = [];
  let identity = 0;
  let sourceBytes = 0;
  let sourceLimited = false;
  const identify = (field: ParsedField): void => {
    identities.set(field, `n.${identity++}`);
    field.children.forEach(identify);
  };
  identify(decoded.root);
  for (const source of decoded.derivedSources) {
    sourceBytes += source.bytes.length;
    if (sourceBytes > maximumDerivedBytes) {
      sourceLimited = true;
      continue;
    }
    known.set(source.id, source.bytes.length);
    sources.push({
      id: source.id,
      name: `Decoded MAPI buffer ${sources.length + 1}`,
      bytesBase64: base64(source.bytes),
      parents: source.parents.map(span),
      transform: source.transform,
      exactMapping: source.exactMapping,
    });
  }

  function document(
    nodeBudget = maximumNodes,
    textBudget = maximumTextBytes,
    forcedLimit = false,
  ): { step: DocumentStep; tokens: Map<string, ParsedField> } {
    const tokens = new Map<string, ParsedField>();
    let count = 0;
    let textBytes = 0;
    let limited =
      forcedLimit ||
      sourceLimited ||
      diagnostics.some((item) => ["input-limit", "live-context-limit"].includes(item.code));
    let malformed = false;
    let partial = diagnostics.length !== 0;
    const incompleteInput = diagnostics.some((item) => item.code === "body-incomplete");
    const issues: Diagnostic[] = [...diagnostics];

    const boundedText = (value: string, maximum: number): string => {
      const room = Math.max(0, Math.min(maximum, Math.floor((textBudget - textBytes) / 3)));
      if (value.length > room) limited = true;
      const text = value.slice(0, room);
      textBytes += text.length * 3;
      return text;
    };

    const inspect = (field: ParsedField): void => {
      if (field.issue) {
        limited ||= field.issue.kind === "limit";
        malformed ||= field.issue.kind === "malformed";
        partial ||= field.issue.kind === "missing-context" || field.issue.kind === "unsupported";
        if (!issues.some((item) => item.message === field.issue!.message)) {
          issues.push({
            code: `mapi-${field.issue.kind}`,
            message: boundedText(field.issue.message, 4096),
          });
        }
      }
      if (field.name === "RopBufferTooSmall response") partial = true;
      field.children.forEach(inspect);
    };
    inspect(decoded.root);

    const value = (field: ParsedField): HostValue | undefined => {
      const parsed = field.value;
      if (!parsed) return field.children.length ? { kind: "object" } : undefined;
      switch (parsed.kind) {
        case "string":
          return { kind: "string", value: boundedText(parsed.value, 16_384) };
        case "integer":
          return { kind: "integer", value: parsed.value };
        case "guid":
          return { kind: "guid", value: parsed.value };
        case "boolean":
          return { kind: "boolean", value: parsed.value };
        case "bytes":
          return { kind: "bytes" };
        case "float":
          return typeof parsed.value === "number"
            ? { kind: "float", value: parsed.value }
            : { kind: "string", value: parsed.value };
        case "enum":
          return parsed.flags
            ? {
                kind: "flags",
                value: {
                  names: parsed.names.slice(0, 64).map((name) => boundedText(name, 256)),
                  raw: parsed.value,
                },
              }
            : {
                kind: "enum",
                value: {
                  name: boundedText(parsed.names[0] ?? parsed.value, 256),
                  raw: parsed.value,
                },
              };
      }
    };

    const node = (field: ParsedField, depth: number): TreeNode => {
      count++;
      const id = identities.get(field)!;
      const result: TreeNode = {
        id,
        name: boundedText(field.name, 384),
        children: [],
        format: {
          protocol: boundedText(field.type, 128),
          byteOrder: field.encoding === "utf-16be" ? "big-endian" : "little-endian",
        },
      };
      const parsed = value(field);
      if (parsed) result.value = parsed;
      if (field.encoding) result.format.charset = field.encoding;
      if (field.units) result.format.units = field.units;
      if (field.type === "f32" || field.type === "f64") result.format.encoding = "IEEE 754";
      if (field.value?.kind === "enum") result.format.radix = 16;
      if (field.value?.kind === "bytes") result.presentation = "hex";
      if (field.value?.kind === "string") result.presentation = "text";
      const length = known.get(field.range.sourceId);
      if (
        length !== undefined &&
        field.range.length > 0 &&
        field.range.offset >= 0 &&
        field.range.offset + field.range.length <= length
      ) {
        result.spans = [span(field.range)];
      }
      if (field.timestamp) {
        result.value = { kind: "timestamp", value: field.timestamp };
        result.format.units = "UTC";
        if (field.value?.kind === "integer" && count < nodeBudget && depth < 32) {
          count++;
          result.children.push({
            id: `${id}.ticks`,
            name: "FILETIME ticks",
            value: { kind: "integer", value: field.value.value },
            children: [],
            format: { units: "100 ns since 1601-01-01 UTC", byteOrder: "little-endian" },
            spans: result.spans ?? [],
          });
        } else limited = true;
      }
      if (field.children.length !== 0) {
        if (depth >= 31) {
          limited = true;
          result.presentation = "hex";
        } else if ((depth < 6 && field.children.length <= 6) || expanded.has(id)) {
          for (const child of field.children) {
            if (count >= nodeBudget) {
              limited = true;
              break;
            }
            result.children.push(node(child, depth + 1));
          }
        } else {
          const token = `e.${revision}.${id}`;
          tokens.set(token, field);
          result.childrenToken = token;
        }
        if (
          field.children.length > 1 &&
          field.children.every((child) => child.type === "PropertyRow")
        )
          result.presentation = "table";
      }
      return result;
    };
    const roots = [node(decoded.root, 0)];
    if (limited)
      issues.push({
        code: "display-limit",
        message:
          "Some display values or decoded byte sources exceed the visualizer limit. Original retained bytes remain available in Hex.",
      });
    const status = limited
      ? "limited"
      : malformed && !incompleteInput
        ? "malformed"
        : partial || malformed
          ? "partial"
          : "complete";
    const step: DocumentStep = {
      kind: "document",
      document: { title, status, presentation: "tree", roots, sources, diagnostics: issues },
    };
    if (serializedSize(step) > 7 * 1024 * 1024) {
      if (nodeBudget > 128 || textBudget > 16_384) {
        return document(
          Math.max(128, Math.floor(nodeBudget / 2)),
          Math.max(16_384, Math.floor(textBudget / 2)),
          true,
        );
      }
      // Keep original source spans when transformed bytes and their parent
      // metadata alone cannot fit. The limited result still exposes Raw/Hex.
      sources.length = 0;
      for (const source of decoded.derivedSources) known.delete(source.id);
      return document(nodeBudget, textBudget, true);
    }
    return { step, tokens };
  }

  let current = document();
  return {
    expanded,
    get step() {
      return current.step;
    },
    expand(token: string): DocumentStep {
      const field = current.tokens.get(token);
      if (!field) throw new Error("Expired MAPI expansion");
      expanded.add(identities.get(field)!);
      current = document();
      return current.step;
    },
  };
}
