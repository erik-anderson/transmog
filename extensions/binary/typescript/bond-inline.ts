import * as Wire from "./bond-wire.js";
import type { Node, VisualizationDocument } from "transmog:extensions/v1";
import {
  DocumentLimits,
  Reader,
  ParseError,
  document,
  failure,
  floating,
  integer,
  text,
} from "./reader.js";
import { mapEntry, retainPartial } from "./values.js";
import {
  MAX_INLINE_COLLECTION_ITEMS,
  MAX_INLINE_MAP_ENTRIES,
  TEXT_PREVIEW_LABEL,
} from "./limits.js";

function type(reader: Reader, raw: number): number {
  if (raw <= Wire.Type.StopBase || raw >= Wire.TYPE_NAMES.length) {
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
  reader.limits.depth(depth, reader.offset);
  const children: Node[] = [];
  let result: Node;
  try {
    if (kind === Wire.Type.Bool) {
      const byte = reader.byte();
      if (byte > 1) {
        throw new ParseError("malformed", "Invalid Bond Boolean", reader.offset - 1);
      }
      result = reader.node(name, start, { kind: "boolean", value: byte === 1 });
    } else if (kind === Wire.Type.UInt8 || kind === Wire.Type.Int8) {
      const raw = reader.byte();
      result = reader.node(
        name,
        start,
        integer(
          kind === Wire.Type.Int8 && raw >= Wire.INT8_SIGN_BIT ? raw - Wire.UINT8_MODULUS : raw,
        ),
      );
    } else if (
      [
        Wire.Type.UInt16,
        Wire.Type.UInt32,
        Wire.Type.UInt64,
        Wire.Type.Int16,
        Wire.Type.Int32,
        Wire.Type.Int64,
      ].includes(kind)
    ) {
      const bits = Wire.integerBits(kind);
      const raw = reader.varint(bits);
      result = reader.node(
        name,
        start,
        integer(Wire.isSignedInteger(kind) ? (raw >> 1n) ^ -(raw & 1n) : raw),
      );
    } else if (kind === Wire.Type.Float32 || kind === Wire.Type.Float64) {
      result = reader.node(
        name,
        start,
        floating(
          reader.floating(kind === Wire.Type.Float32 ? Wire.FLOAT32_BYTES : Wire.FLOAT64_BYTES),
        ),
      );
    } else if (kind === Wire.Type.String || kind === Wire.Type.WString) {
      const count = Number(reader.varint(Wire.LENGTH_BITS));
      const data = reader.take(count * (kind === Wire.Type.WString ? Wire.UINT16_BYTES : 1));
      const decoded = text(data, kind === Wire.Type.WString);
      result = reader.node(decoded.clipped ? name + TEXT_PREVIEW_LABEL : name, start, {
        kind: "string",
        value: decoded.value,
      });
      result.format = { charset: kind === Wire.Type.WString ? "utf-16le" : "utf-8" };
    } else if (kind === Wire.Type.Struct) {
      structure(reader, children, version, depth + 1);
      result = reader.node(name, start, { kind: "object" }, children);
    } else if (kind === Wire.Type.List || kind === Wire.Type.Set) {
      const header = reader.byte();
      const element = type(reader, header & Wire.TYPE_TAG_MASK);
      if (version === Wire.Version.V1 && header > Wire.TYPE_TAG_MASK) {
        throw new ParseError("malformed", "Invalid Bond v1 container type", reader.offset - 1);
      }
      const count =
        version === Wire.Version.V2 && header >>> Wire.CONTAINER_COUNT_SHIFT
          ? (header >>> Wire.CONTAINER_COUNT_SHIFT) - 1
          : Number(reader.varint(Wire.LENGTH_BITS));
      for (let index = 0; index < count; index++) {
        if (index >= MAX_INLINE_COLLECTION_ITEMS) {
          throw new ParseError("limited", "Collection display limit reached", reader.offset);
        }
        children.push(
          value(reader, element, `[${index}] · ${Wire.TYPE_NAMES[element]}`, version, depth + 1),
        );
      }
      result = reader.node(`${name} · ${count} items`, start, { kind: "array" }, children);
    } else if (kind === Wire.Type.Map) {
      const key = type(reader, reader.byte());
      const mapped = type(reader, reader.byte());
      const count = Number(reader.varint(Wire.LENGTH_BITS));
      for (let index = 0; index < count; index++) {
        if (index >= MAX_INLINE_MAP_ENTRIES) {
          throw new ParseError("limited", "Map display limit reached", reader.offset);
        }
        const itemStart = reader.offset;
        const keyNode = value(reader, key, `Key · ${Wire.TYPE_NAMES[key]}`, version, depth + 1);
        children.push(
          mapEntry(reader, `[${index}]`, itemStart, keyNode, () =>
            value(reader, mapped, `Value · ${Wire.TYPE_NAMES[mapped]}`, version, depth + 1),
          ),
        );
      }
      result = reader.node(`${name} · ${count} entries`, start, { kind: "array" }, children);
    } else {
      throw new ParseError("malformed", "Unsupported Bond type", reader.offset);
    }
    return result;
  } catch (error) {
    retainPartial(
      error,
      reader,
      name,
      start,
      children,
      [Wire.Type.List, Wire.Type.Set, Wire.Type.Map].includes(kind),
    );
  }
}

function structure(reader: Reader, children: Node[], version: number, depth: number): void {
  reader.limits.depth(depth, reader.offset);
  const length =
    version === Wire.Version.V2 ? Number(reader.varint(Wire.LENGTH_BITS)) : reader.remaining;
  const truncated = length > reader.remaining;
  const fields =
    version === Wire.Version.V2 ? reader.take(Math.min(length, reader.remaining)) : reader;
  while (fields.remaining) {
    const start = fields.offset;
    const header = fields.byte();
    const kind = header & Wire.TYPE_TAG_MASK;
    if (kind === Wire.Type.Stop) {
      if (
        header !== Wire.Type.Stop ||
        (version === Wire.Version.V2 && (fields.remaining || truncated))
      ) {
        throw new ParseError("malformed", "Invalid Bond struct terminator or length", start);
      }
      return;
    }
    if (kind === Wire.Type.StopBase) {
      if (header !== Wire.Type.StopBase) {
        throw new ParseError("malformed", "Invalid Bond base terminator", start);
      }
      children.push(fields.node("End of base struct", start, { kind: "object" }));
      continue;
    }
    type(fields, kind);
    const encodedId = header >>> Wire.FIELD_ID_SHIFT;
    const id =
      encodedId === Wire.EXTENDED_FIELD_ID8
        ? fields.byte()
        : encodedId === Wire.EXTENDED_FIELD_ID16
          ? Number(fields.fixed(Wire.UINT16_BYTES))
          : encodedId;
    children.push(
      value(fields, kind, `Field ${id} · ${Wire.TYPE_NAMES[kind]}`, version, depth, start),
    );
  }
  throw new ParseError("malformed", "Missing Bond struct terminator", fields.offset);
}

export function readInlineCandidate(
  bytes: Uint8Array,
  sourceId: string,
  version: number,
  start: number,
  sequence = false,
): VisualizationDocument {
  const doc = document("Bond Compact Binary", Wire.SUMMARY);
  const reader = new Reader(
    bytes,
    sourceId,
    new DocumentLimits(`bond${version}.${start ? "marshaled" : "raw"}`),
    start,
  );
  let children: Node[] = [];
  let structStart = start;
  let ordinal = 0;
  const root = (): void => {
    doc.roots.push({
      id: `${reader.limits.prefix}.root.${ordinal++}`,
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
      // Empty structs count toward the display-node limit.
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
    let remainderStart = structStart;
    if (completed) {
      // The completed root already includes its terminator.
      remainderStart = reader.offset;
    } else if (partial) {
      remainderStart = Number(partial.spans?.[0]?.offset ?? start);
    } else if (last) {
      remainderStart = Number(last.offset) + Number(last.length);
    }
    failure(doc, error, reader, remainderStart);
    if (partial) {
      doc.roots.at(-1)!.name = "Incomplete value bytes";
    }
  }
  return doc;
}

export type CandidateReader = typeof readInlineCandidate;
