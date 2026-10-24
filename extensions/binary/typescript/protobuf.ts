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

const summary =
  "Schema-free wire view. Field names and declared types are unknown. Possible interpretations are alternatives, not schema facts. Length-delimited values may be text, bytes, messages or packed numbers.";

function packed(reader: Reader, start: number, end: number): Node[] {
  const result: Node[] = [];
  // Full structural validation precedes a bounded preview; no candidate is a schema fact.
  const candidates = [
    "varints",
    "ZigZag integers",
    "fixed32 integers",
    "float32",
    "fixed64 integers",
    "float64",
  ];
  for (const kind of candidates) {
    const width = kind.includes("32") ? 4 : kind.includes("64") ? 8 : 0;
    if (width && (end - start) % width) {
      continue;
    }
    const payload = new Reader(reader.bytes, reader.sourceId, reader.budget, start, end);
    const values: Node[] = [];
    let count = 0;
    try {
      while (payload.remaining) {
        const offset = payload.offset;
        const raw = width
          ? kind.startsWith("float")
            ? floating(payload.floating(width as 4 | 8))
            : integer(payload.fixed(width))
          : payload.varint();
        count++;
        if (count <= 128) {
          const value =
            typeof raw === "bigint"
              ? integer(kind === "ZigZag integers" ? (raw >> 1n) ^ -(raw & 1n) : raw)
              : raw;
          values.push(payload.node(`Item ${count}`, offset, value));
        }
      }
      if (count) {
        result.push(
          payload.node(
            `Possible packed ${kind}${count > 128 ? " (preview; first 128 items)" : ""}`,
            start,
            { kind: "array" },
            values,
          ),
        );
      }
    } catch (error) {
      if (!(error instanceof ParseError) || error.status === "limited") {
        throw error;
      }
    }
  }
  return result;
}

function fields(reader: Reader, output: Node[], depth: number, group?: number): void {
  reader.budget.depth(depth, reader.offset);
  while (reader.remaining) {
    const start = reader.offset;
    const tag = Number(reader.varint(32));
    const field = Math.floor(tag / 8);
    const wire = tag & 7;
    if (!field || field > 0x1fffffff || wire > 5) {
      throw new ParseError("malformed", "Invalid Protobuf field tag", start);
    }
    if (wire === 4) {
      if (group !== field) {
        throw new ParseError("malformed", "Unmatched end-group tag", start);
      }
      return;
    }
    const valueStart = reader.offset;
    const children: Node[] = [];
    if (wire === 0) {
      const raw = reader.varint();
      if (raw >= 1n << 63n) {
        children.push(
          reader.node("Possible signed int64", valueStart, integer(BigInt.asIntN(64, raw))),
        );
      }
      children.push(
        reader.node(
          "Possible ZigZag signed integer",
          valueStart,
          integer((raw >> 1n) ^ -(raw & 1n)),
        ),
      );
      if (raw <= 1n) {
        children.push(
          reader.node("Possible Boolean", valueStart, { kind: "boolean", value: raw === 1n }),
        );
      }
      output.push(
        reader.node(`Field ${field} · varint (raw unsigned)`, start, integer(raw), children),
      );
    } else if (wire === 1 || wire === 5) {
      const width = wire === 1 ? 8 : 4;
      const raw = reader.fixed(width);
      const bits = new Reader(
        reader.bytes,
        reader.sourceId,
        reader.budget,
        valueStart,
        reader.offset,
      );
      children.push(
        reader.node(
          `Possible signed ${width * 8}-bit integer`,
          valueStart,
          integer(BigInt.asIntN(width * 8, raw)),
        ),
      );
      children.push(
        reader.node(
          `Possible ${width === 4 ? "float" : "double"}`,
          valueStart,
          floating(bits.floating(width)),
        ),
      );
      output.push(
        reader.node(
          `Field ${field} · fixed${width * 8} (raw unsigned)`,
          start,
          integer(raw),
          children,
        ),
      );
    } else if (wire === 2) {
      const length = Number(reader.varint(32));
      if (length > 0x7fffffff) {
        throw new ParseError("malformed", "Invalid Protobuf length", valueStart);
      }
      const payload = reader.take(length);
      const rawStart = payload.offset;
      const raw = new Reader(reader.bytes, reader.sourceId, reader.budget, rawStart, payload.end);
      raw.offset = payload.end;
      children.push(raw.node("Raw payload", rawStart, { kind: "bytes" }));
      // Large payloads remain inspectable as bytes; speculative decoding is bounded.
      if (length <= 65_536) {
        try {
          const decoded = text(
            new Reader(reader.bytes, reader.sourceId, reader.budget, rawStart, payload.end),
          );
          children.push(
            raw.node(
              decoded.clipped
                ? "Possible UTF-8 text preview (first 1,024 characters)"
                : "Possible UTF-8 text",
              rawStart,
              { kind: "string", value: decoded.value },
            ),
          );
        } catch (error) {
          if (!(error instanceof ParseError) || error.status === "limited") {
            throw error;
          }
        }
        if (length && depth < 10) {
          const candidate: Node[] = [];
          try {
            fields(payload, candidate, depth + 1);
            children.push(
              raw.node("Possible embedded message", rawStart, { kind: "object" }, candidate),
            );
          } catch (error) {
            if (!(error instanceof ParseError) || error.status === "limited") {
              throw error;
            }
          }
        }
      }
      if (length && length <= 4_096) {
        children.push(...packed(reader, rawStart, payload.end));
      }
      output.push(
        reader.node(
          `Field ${field} · length-delimited · ${length} bytes`,
          start,
          { kind: "object" },
          children,
        ),
      );
    } else {
      fields(reader, children, depth + 1, field);
      output.push(reader.node(`Field ${field} · group`, start, { kind: "object" }, children));
    }
  }
  if (group !== undefined) {
    throw new ParseError("malformed", "Missing end-group tag", reader.offset);
  }
}

export function parse(bytes: Uint8Array, sourceId: string): VisualizationDocument {
  const doc = document("Protobuf", summary);
  const reader = new Reader(bytes, sourceId, new Budget("protobuf"));
  try {
    fields(reader, doc.roots, 0);
  } catch (error) {
    const end = doc.roots.at(-1)?.spans?.[0];
    failure(doc, error, reader, end ? Number(end.offset) + Number(end.length) : 0);
  }
  return doc;
}
