import { displayLimits } from "./view-limits.js";
import { fieldValue, parseOutcome, parseStatus } from "./view-fields.js";
import type { ByteRange, DecodeResult, ParsedField } from "./model.js";
import type { Diagnostic, Node, Format, DerivedSource, Span } from "transmog:extensions/v1";
import type { DocumentStep } from "./shared.js";
import { notice, base64, unicode } from "./shared.js";
import { utf8Prefix } from "./text.js";

let epoch = 0;

function serializedSize(value: unknown): number {
  const text = JSON.stringify(value);
  let bytes = 0;
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    if (code < unicode.asciiEnd) {
      bytes++;
    } else if (code < unicode.twoByteEnd) {
      bytes += 2;
    } else if (code >= unicode.highSurrogateStart && code < unicode.lowSurrogateStart) {
      bytes += 4;
      index++;
    } else {
      bytes += 3;
    }
  }
  return bytes;
}

interface NodeIdentities {
  readonly byLocation: ReadonlyMap<string, string>;
  readonly nextId: number;
}

export interface Presentation {
  readonly identities: NodeIdentities;
  readonly expanded: Set<string>;
  readonly step: DocumentStep;
  expand(token: string): DocumentStep;
}

/** Resolve live identities without letting the rendering closures retain previous trees. */
function identifyNodes(root: ParsedField, previous?: Presentation) {
  const expanded = new Set<string>();
  const fieldIds = new WeakMap<ParsedField, string>();
  // Keep current locations only, and never reuse the ID of a removed field.
  const byLocation = new Map<string, string>();
  let nextId = previous?.identities.nextId ?? 0;
  // Match siblings by name, type and occurrence, independently of other branches.
  const identify = (field: ParsedField, parentId: string, occurrence: number): void => {
    const location = JSON.stringify([parentId, field.name, field.type, occurrence]);
    const id = previous?.identities.byLocation.get(location) ?? `n.${nextId++}`;
    byLocation.set(location, id);
    fieldIds.set(field, id);
    if (previous?.expanded.has(id)) {
      expanded.add(id);
    }
    const occurrences = new Map<string, number>();
    for (const child of field.children) {
      const kind = JSON.stringify([child.name, child.type]);
      const position = occurrences.get(kind) ?? 0;
      occurrences.set(kind, position + 1);
      identify(child, id, position);
    }
  };
  identify(root, "", 0);
  return { identities: { byLocation, nextId }, fieldIds, expanded };
}

function span(range: ByteRange): Span {
  return { sourceId: range.sourceId, offset: String(range.offset), length: String(range.length) };
}

/** Converts protocol facts to host-owned presentation. It never creates UI. */
export function present(
  decoded: DecodeResult,
  origins: Iterable<readonly [string, number]>,
  title: string,
  diagnostics: readonly Diagnostic[],
  previous?: Presentation,
  finalInput = false,
): Presentation {
  const revision = ++epoch;
  const { identities, fieldIds, expanded } = identifyNodes(decoded.root, previous);
  const known = new Map(origins);
  const sources: DerivedSource[] = [];
  let sourceBytes = 0;
  let sourceLimited = false;
  for (const source of decoded.derivedSources) {
    sourceBytes += source.bytes.length;
    if (sourceBytes > displayLimits.derivedBytes || sources.length >= displayLimits.sources) {
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

  function project(
    nodeLimit: number = displayLimits.nodes,
    textLimit: number = displayLimits.textBytes,
    forcedLimit = false,
  ): { step: DocumentStep; tokens: Map<string, ParsedField> } {
    const tokens = new Map<string, ParsedField>();
    let count = 0;
    let textBytes = 0;
    let limited = forcedLimit || sourceLimited;
    const boundedText = (
      value: string,
      maximumCodeUnits: number,
      maximumUtf8Bytes = maximumCodeUnits,
    ): string => {
      const room = Math.max(
        0,
        Math.min(
          maximumCodeUnits,
          Math.floor((textLimit - textBytes) / displayLimits.utf8BytesPerCodeUnit),
        ),
      );
      const text = utf8Prefix(value, maximumUtf8Bytes, room);
      if (text.length < value.length) {
        limited = true;
      }
      textBytes += text.length * displayLimits.utf8BytesPerCodeUnit;
      return text;
    };

    const outcome = parseOutcome(decoded.root, diagnostics, boundedText);
    limited ||= outcome.diagnosticsClipped;
    const issues = outcome.issues;
    const node = (field: ParsedField, depth: number): Node => {
      count++;
      const id = fieldIds.get(field)!;
      const result: Node & { children: Node[]; format: Format } = {
        id,
        name: boundedText(field.name, displayLimits.fieldName),
        children: [],
        format: {
          protocol: boundedText(field.type, displayLimits.protocolName),
          byteOrder: field.encoding === "utf-16be" ? "big-endian" : "little-endian",
        },
      };
      const parsed = fieldValue(field, boundedText);
      if (
        field.value?.kind === "enum" &&
        field.value.flags &&
        field.value.names.length > displayLimits.flagNames
      ) {
        limited = true;
      }
      if (parsed) {
        result.value = parsed;
      }
      if (field.encoding) {
        result.format.charset = field.encoding;
      }
      if (field.units) {
        result.format.units = field.units;
      }
      if (field.type === "f32" || field.type === "f64") {
        result.format.encoding = "IEEE 754";
      }
      if (field.value?.kind === "enum") {
        result.format.radix = 16;
      }
      if (field.value?.kind === "bytes") {
        result.presentation = "hex";
      }
      if (field.value?.kind === "string") {
        result.presentation = "text";
      }
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
        if (field.value?.kind === "integer" && count < nodeLimit && depth < displayLimits.depth) {
          count++;
          result.children.push({
            id: `${id}.ticks`,
            name: "FILETIME ticks",
            value: { kind: "integer", value: field.value.value },
            children: [],
            format: { units: "100 ns since 1601-01-01 UTC", byteOrder: "little-endian" },
            spans: result.spans ?? [],
          });
        } else {
          limited = true;
        }
      }
      if (field.children.length !== 0) {
        if (depth >= displayLimits.depth - 1) {
          limited = true;
          result.presentation = "hex";
        } else if (
          (depth < displayLimits.eagerDepth &&
            field.children.length <= displayLimits.eagerChildren) ||
          expanded.has(id)
        ) {
          for (const child of field.children) {
            if (count >= nodeLimit) {
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
        ) {
          result.presentation = "table";
        }
      }
      return result;
    };
    const roots = [node(decoded.root, 0)];
    if (limited) {
      issues.push({
        code: "display-limit",
        message:
          "Some display values, diagnostics or decoded byte sources exceed the visualizer limit. Original retained bytes remain available in Hex.",
      });
    }
    const status = parseStatus({ ...outcome, limited: limited || outcome.limited });
    const step: DocumentStep = {
      kind: "document",
      document: {
        title,
        status,
        ...(finalInput && status === "complete" ? { updates: "final" as const } : {}),
        presentation: "tree",
        roots,
        sources,
        diagnostics: issues,
      },
    };
    return { step, tokens };
  }

  /** Each retry shrinks the tree, removes sources once, or returns a small notice. */
  function fitDocument(): { step: DocumentStep; tokens: Map<string, ParsedField> } {
    let nodeLimit: number = displayLimits.nodes;
    let textLimit: number = displayLimits.textBytes;
    let clipped = false;
    for (;;) {
      const result = project(nodeLimit, textLimit, clipped);
      if (serializedSize(result.step) <= displayLimits.serializedBytes) {
        return result;
      }
      clipped = true;
      if (nodeLimit > displayLimits.minimumNodes || textLimit > displayLimits.minimumTextBytes) {
        nodeLimit = Math.max(displayLimits.minimumNodes, Math.floor(nodeLimit / 2));
        textLimit = Math.max(displayLimits.minimumTextBytes, Math.floor(textLimit / 2));
        continue;
      }
      if (sources.length !== 0) {
        sources.length = 0;
        for (const source of decoded.derivedSources) {
          known.delete(source.id);
        }
        continue;
      }
      return {
        step: notice(
          title.slice(0, displayLimits.title),
          "limited",
          "The decoded structure exceeds the display limit. Original retained bytes remain available in Hex.",
        ),
        tokens: new Map(),
      };
    }
  }

  let current = fitDocument();
  return {
    identities,
    expanded,
    get step() {
      return current.step;
    },
    expand(token: string): DocumentStep {
      const field = current.tokens.get(token);
      if (!field) {
        throw new Error("Expired MAPI expansion");
      }
      expanded.add(fieldIds.get(field)!);
      current = fitDocument();
      return current.step;
    },
  };
}
