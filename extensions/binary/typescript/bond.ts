import type { Node, VisualizationDocument } from "transmog:extensions/v1";
import {
  Budget,
  Reader,
  ParseError,
  document,
  failure,
  floating,
  integer,
  text,
} from "./reader.js";
import { mapEntry, retainPartial } from "./values.js";

// Wire tags from BondDataType; this reader has no dependency on the retired Bond runtime.
export const names = [
  "stop",
  "stop-base",
  "bool",
  "uint8",
  "uint16",
  "uint32",
  "uint64",
  "float",
  "double",
  "string",
  "struct",
  "list",
  "set",
  "map",
  "int8",
  "int16",
  "int32",
  "int64",
  "wstring",
];
export const summary =
  "Schema-free Compact Binary. Field IDs and wire types are available; field names, enum labels and schema aliases are unknown.";

function type(reader: Reader, raw: number): number {
  if (raw < 2 || raw >= names.length) {
    throw new ParseError("malformed", "Invalid Bond value type", reader.offset - 1);
  }
  return raw;
}

function value(
  reader: Reader,
  kind: number,
  name: string,
  version: number,
  depth: number,
  start = reader.offset,
): Node {
  reader.budget.depth(depth, reader.offset);
  const children: Node[] = [];
  let result: Node;
  try {
    if (kind === 2) {
      const byte = reader.byte();
      if (byte > 1) {
        throw new ParseError("malformed", "Invalid Bond Boolean", reader.offset - 1);
      }
      result = reader.node(name, start, { kind: "boolean", value: byte === 1 });
    } else if (kind === 3 || kind === 14) {
      const raw = reader.byte();
      result = reader.node(name, start, integer(kind === 14 && raw >= 128 ? raw - 256 : raw));
    } else if ([4, 5, 6, 15, 16, 17].includes(kind)) {
      const bits = kind === 4 || kind === 15 ? 16 : kind === 5 || kind === 16 ? 32 : 64;
      const raw = reader.varint(bits);
      result = reader.node(name, start, integer(kind >= 15 ? (raw >> 1n) ^ -(raw & 1n) : raw));
    } else if (kind === 7 || kind === 8) {
      result = reader.node(name, start, floating(reader.floating(kind === 7 ? 4 : 8)));
    } else if (kind === 9 || kind === 18) {
      const count = Number(reader.varint(32));
      const data = reader.take(count * (kind === 18 ? 2 : 1));
      const decoded = text(data, kind === 18);
      result = reader.node(
        decoded.clipped ? name + " · preview (first 1,024 characters)" : name,
        start,
        { kind: "string", value: decoded.value },
      );
      result.format = { charset: kind === 18 ? "utf-16le" : "utf-8" };
    } else if (kind === 10) {
      structure(reader, children, version, depth + 1);
      result = reader.node(name, start, { kind: "object" }, children);
    } else if (kind === 11 || kind === 12) {
      const header = reader.byte();
      const element = type(reader, header & 31);
      if (version === 1 && header > 31) {
        throw new ParseError("malformed", "Invalid Bond v1 container type", reader.offset - 1);
      }
      const count = version === 2 && header >>> 5 ? (header >>> 5) - 1 : Number(reader.varint(32));
      for (let index = 0; index < count; index++) {
        if (index >= 4_000) {
          throw new ParseError("limited", "Collection display limit reached", reader.offset);
        }
        children.push(value(reader, element, `[${index}] · ${names[element]}`, version, depth + 1));
      }
      result = reader.node(`${name} · ${count} items`, start, { kind: "array" }, children);
    } else if (kind === 13) {
      const key = type(reader, reader.byte());
      const mapped = type(reader, reader.byte());
      const count = Number(reader.varint(32));
      for (let index = 0; index < count; index++) {
        if (index >= 1_000) {
          throw new ParseError("limited", "Map display limit reached", reader.offset);
        }
        const itemStart = reader.offset;
        const keyNode = value(reader, key, `Key · ${names[key]}`, version, depth + 1);
        children.push(
          mapEntry(reader, `[${index}]`, itemStart, keyNode, () =>
            value(reader, mapped, `Value · ${names[mapped]}`, version, depth + 1),
          ),
        );
      }
      result = reader.node(`${name} · ${count} entries`, start, { kind: "array" }, children);
    } else {
      throw new ParseError("malformed", "Unsupported Bond type", reader.offset);
    }
    return result;
  } catch (error) {
    retainPartial(error, reader, name, start, children, [11, 12, 13].includes(kind));
  }
}

function structure(reader: Reader, children: Node[], version: number, depth: number): void {
  reader.budget.depth(depth, reader.offset);
  const length = version === 2 ? Number(reader.varint(32)) : reader.remaining;
  const truncated = length > reader.remaining;
  const fields = version === 2 ? reader.take(Math.min(length, reader.remaining)) : reader;
  while (fields.remaining) {
    const start = fields.offset;
    const header = fields.byte();
    const kind = header & 31;
    if (kind === 0) {
      if (header !== 0 || (version === 2 && (fields.remaining || truncated))) {
        throw new ParseError("malformed", "Invalid Bond struct terminator or length", start);
      }
      return;
    }
    if (kind === 1) {
      if (header !== 1) {
        throw new ParseError("malformed", "Invalid Bond base terminator", start);
      }
      children.push(fields.node("End of base struct", start, { kind: "object" }));
      continue;
    }
    type(fields, kind);
    const encodedId = header >>> 5;
    const id =
      encodedId === 6 ? fields.byte() : encodedId === 7 ? Number(fields.fixed(2)) : encodedId;
    children.push(value(fields, kind, `Field ${id} · ${names[kind]}`, version, depth, start));
  }
  throw new ParseError("malformed", "Missing Bond struct terminator", fields.offset);
}

function candidate(
  bytes: Uint8Array,
  sourceId: string,
  version: number,
  start: number,
  sequence = false,
): VisualizationDocument {
  const doc = document("Bond Compact Binary", summary);
  const reader = new Reader(
    bytes,
    sourceId,
    new Budget(`bond${version}.${start ? "marshaled" : "raw"}`),
    start,
  );
  let children: Node[] = [];
  let structStart = start;
  let ordinal = 0;
  const root = (): void => {
    doc.roots.push({
      id: `${reader.budget.prefix}.root.${ordinal++}`,
      name: `Compact Binary v${version}`,
      value: { kind: "object" },
      spans: [
        { sourceId, offset: String(structStart), length: String(reader.offset - structStart) },
      ],
      children,
    });
  };
  try {
    do {
      structStart = reader.offset;
      children = [];
      reader.budget.step(structStart);
      // Empty structs must also consume the display budget.
      reader.node("Struct", structStart);
      structure(reader, children, version, 0);
      root();
      if (reader.remaining && !sequence) {
        throw new ParseError("malformed", "Trailing bytes after Bond struct", reader.offset);
      }
    } while (reader.remaining);
  } catch (error) {
    const completed = doc.roots.at(-1)?.children === children;
    const last = children.at(-1)?.spans?.[0];
    const partial = error instanceof ParseError ? error.partial : undefined;
    if (partial) {
      children.push(partial);
    }
    if (!completed) {
      root();
    }
    failure(
      doc,
      error,
      reader,
      partial
        ? Number(partial.spans?.[0]?.offset ?? start)
        : last
          ? Number(last.offset) + Number(last.length)
          : structStart,
    );
    if (partial) {
      doc.roots.at(-1)!.name = "Incomplete value bytes";
    }
  }
  return doc;
}

export type CandidateReader = typeof candidate;

export function parse(
  bytes: Uint8Array,
  sourceId: string,
  read: CandidateReader = candidate,
): VisualizationDocument {
  const header = bytes[0] === 0x43 && bytes[1] === 0x42;
  // A leading protocol header frames the whole body. Probe the existing raw
  // single-struct alternative; do not count the header itself as a batch item.
  const raw = rawDocument(bytes, sourceId, !header, read);
  if (header) {
    // The protocol magic can also be a raw field header and value. Do not hide
    // a valid alternative or silently treat a damaged envelope as established raw data.
    const rawValid =
      raw.status === "complete" ||
      raw.diagnostics!.some(
        (item) => item.code === "ambiguous-version" || item.code === "version-limited",
      );
    if (bytes.length < 4) {
      if (rawValid) {
        return framingAmbiguity(raw);
      }
      return {
        ...document("Bond Compact Binary", summary),
        status: "malformed",
        diagnostics: [{ code: "malformed", message: "Truncated Bond protocol/version header." }],
      };
    }
    const version = bytes[2]! | (bytes[3]! << 8);
    if (version !== 1 && version !== 2) {
      if (rawValid) {
        return framingAmbiguity(raw);
      }
      return {
        ...document("Bond Compact Binary", summary),
        status: "partial",
        diagnostics: [
          {
            code: "unsupported-version",
            message: `Compact Binary version ${version} is unsupported. Inspect the original bytes in Hex.`,
          },
        ],
      };
    }
    const doc = read(bytes, sourceId, version, 4);
    doc.summary += ` The marshaled header declares version ${version}.`;
    doc.roots.unshift({
      id: "bond.header",
      name: `Protocol header · Compact Binary v${version}`,
      spans: [{ sourceId, offset: "0", length: "4" }],
      value: { kind: "bytes" },
      children: [],
    });
    if (rawValid) {
      if (doc.status === "complete") {
        raw.roots.unshift(...doc.roots);
      }
      return framingAmbiguity(raw);
    }
    if (raw.status === "limited" && doc.status === "complete") {
      return limitedInterpretation(
        doc,
        "framing-limited",
        "A raw Compact Binary interpretation reached a parser limit and was not ruled out. Framing remains unresolved.",
      );
    }
    return doc;
  }
  return raw;
}

function framingAmbiguity(raw: VisualizationDocument): VisualizationDocument {
  if (raw.status !== "limited") {
    raw.status = "partial";
  }
  raw.summary =
    summary +
    " Framing is ambiguous: the protocol-header bytes can also be raw Compact Binary fields.";
  raw.diagnostics!.push({
    code: "ambiguous-framing",
    message:
      "The bytes resemble a marshaled header and also parse as raw fields. The available interpretations are shown; neither framing is established.",
  });
  for (const root of raw.roots) {
    if (!root.name.startsWith("Possible")) {
      root.name = "Possible " + root.name;
    }
  }
  return raw;
}

function limitedInterpretation(
  doc: VisualizationDocument,
  code: string,
  message: string,
): VisualizationDocument {
  doc.status = "limited";
  doc.summary += " " + message;
  doc.diagnostics!.push({ code, message });
  for (const root of doc.roots) {
    if (!root.name.startsWith("Possible")) {
      root.name = "Possible " + root.name;
    }
  }
  return doc;
}

function rawDocument(
  bytes: Uint8Array,
  sourceId: string,
  sequence: boolean,
  read: CandidateReader,
): VisualizationDocument {
  const first = read(bytes, sourceId, 1, 0, sequence);
  const second = read(bytes, sourceId, 2, 0, sequence);
  if (sequence && first.status === "malformed" && second.status === "malformed") {
    return {
      ...document(
        "Bond Compact Binary",
        summary +
          " Neither raw version parsed completely. Both incomplete interpretations are shown.",
      ),
      status: "malformed",
      roots: [first, second].flatMap((doc, index) =>
        doc.roots.map((root) => ({
          ...root,
          name: `Possible Compact Binary v${index + 1} (incomplete) · ${root.name}`,
        })),
      ),
      diagnostics: [first, second].flatMap((doc, index) =>
        doc.diagnostics!.map((item) => ({ ...item, message: `v${index + 1}: ${item.message}` })),
      ),
    };
  }
  if (first.status === "complete" && second.status === "complete") {
    return {
      ...document(
        "Bond Compact Binary",
        summary + " Version is ambiguous: both raw v1 and v2 interpretations parse completely.",
      ),
      status: "partial",
      roots: [
        ...first.roots.map((root) => ({ ...root, name: "Possible " + root.name })),
        ...second.roots.map((root) => ({ ...root, name: "Possible " + root.name })),
      ],
      diagnostics: [
        {
          code: "ambiguous-version",
          message:
            "No protocol/version header was captured. Both interpretations are shown; neither version is established.",
        },
      ],
    };
  }
  const doc =
    second.status === "complete" ||
    (first.status !== "complete" &&
      first.status !== "limited" &&
      (second.status === "limited" ||
        second.roots[0]!.children!.length > first.roots[0]!.children!.length))
      ? second
      : first;
  const version = doc === second ? 2 : 1;
  const alternative = doc === second ? first : second;
  if (doc.status === "complete" && alternative.status === "limited") {
    doc.roots[0]!.name = `Possible Compact Binary v${version} (version unresolved)`;
    return limitedInterpretation(
      doc,
      "version-limited",
      `The v${version === 1 ? 2 : 1} interpretation reached a parser limit and was not ruled out. Version remains unresolved.`,
    );
  }
  for (const [index, root] of doc.roots.entries()) {
    if (root.value?.kind === "object") {
      root.name = `Possible Compact Binary v${version} (${doc.status === "complete" ? "inferred" : "incomplete interpretation"})${doc.roots.length > 1 ? ` · Struct ${index + 1}` : ""}`;
    }
  }
  doc.summary +=
    doc.status === "complete"
      ? ` No protocol/version header was captured; v${version} is inferred from structure.`
      : ` Neither raw version parsed completely; showing a possible v${version} interpretation.`;
  doc.diagnostics!.unshift({
    code: "inferred-version",
    message:
      "The Compact Binary version is inferred, not declared. Schema aliases such as enums, blobs and nullable values cannot be recovered.",
  });
  return doc;
}
