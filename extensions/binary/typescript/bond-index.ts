import type { Node, Value, VisualizationDocument } from "transmog:extensions/v1";
import { names, parse, summary } from "./bond.js";
import { Budget, document, floating, integer, ParseError, Reader } from "./reader.js";

// Retain wire offsets, not a decoded object graph. Allocation is in small typed
// blocks so failed version probes do not reserve the maximum index up front.
const width = 10;
const blockSize = 4096;
const maximumNodes = 500_000;
const pageSize = 64;
const fanout = 16;
const incomplete = 1 << 24;
enum Column {
  Start,
  End,
  Data,
  Meta,
  Label,
  First,
  Next,
  Count,
  After,
  Depth,
}
enum Role {
  Root,
  Field,
  Item,
  Key,
  Value,
  Entry,
  Base,
}

class Index {
  private blocks: Uint32Array[] = [];
  length = 0;
  offset: number;
  end: number;
  error?: ParseError;
  constructor(
    readonly bytes: Uint8Array,
    readonly sourceId: string,
    readonly version: number,
    readonly start: number,
    readonly sequence: boolean,
  ) {
    this.offset = start;
    this.end = bytes.length;
    this.add(10, Role.Root, 0, 0, start);
    try {
      let ordinal = 0;
      do {
        const id = this.add(10, Role.Root, ordinal++, 0);
        this.link(0, id);
        this.finish(id, () => this.structure(id, 0));
        if (!sequence && this.offset < this.end) {
          this.fail("Trailing bytes after Bond struct");
        }
      } while (this.offset < this.end);
    } catch (error) {
      if (!(error instanceof ParseError)) {
        throw error;
      }
      this.error = error;
    }
    this.set(0, Column.End, this.offset);
    this.set(0, Column.After, this.length);
  }
  get(id: number, column: Column): number {
    return this.blocks[Math.floor(id / blockSize)]![(id % blockSize) * width + column]!;
  }
  set(id: number, column: Column, value: number): void {
    this.blocks[Math.floor(id / blockSize)]![(id % blockSize) * width + column] = value;
  }
  private add(kind: number, role: Role, label: number, depth: number, start = this.offset): number {
    if (this.length >= maximumNodes) {
      throw new ParseError("limited", "Bond index limit reached (500,000 values)", this.offset);
    }
    if (depth > 64) {
      throw new ParseError("limited", "Bond nesting limit reached (64 levels)", this.offset);
    }
    const id = this.length++;
    if (id % blockSize === 0) {
      this.blocks.push(new Uint32Array(blockSize * width));
    }
    this.set(id, Column.Start, start);
    this.set(id, Column.Meta, kind | (role << 8) | incomplete);
    this.set(id, Column.Label, label);
    this.set(id, Column.Depth, depth);
    this.set(id, Column.Data, this.offset);
    return id;
  }
  private link(parent: number, id: number): void {
    // Only active containers need a temporary sibling tail.
    const previous = this.tails.get(parent);
    if (previous == null) {
      this.set(parent, Column.First, id);
    } else {
      this.set(previous, Column.Next, id);
    }
    this.tails.set(parent, id);
    this.set(parent, Column.Count, this.get(parent, Column.Count) + 1);
  }
  private tails = new Map<number, number>();
  private finish(id: number, action: () => void): void {
    try {
      action();
      this.set(id, Column.Meta, this.get(id, Column.Meta) & ~incomplete);
    } finally {
      this.set(id, Column.End, this.offset);
      this.set(id, Column.After, this.length);
      this.tails.delete(id);
    }
  }
  private fail(message: string, offset = this.offset): never {
    throw new ParseError("malformed", message, offset);
  }
  private require(length: number): void {
    if (length < 0 || length > this.end - this.offset) {
      this.fail("Truncated value");
    }
  }
  private byte(): number {
    this.require(1);
    return this.bytes[this.offset++]!;
  }
  private tag(raw: number): number {
    if (raw < 2 || raw >= names.length) {
      this.fail("Invalid Bond value type", this.offset - 1);
    }
    return raw;
  }
  private varint(bits = 32): number {
    const start = this.offset;
    let result = 0;
    for (let shift = 0; shift < bits; shift += 7) {
      const byte = this.byte();
      const payload = byte & 127;
      if (shift + 7 > bits && payload >= 2 ** (bits - shift)) {
        break;
      }
      // 64-bit values are validated here and decoded exactly only for display.
      if (bits <= 32) {
        result += payload * 2 ** shift;
      }
      if (byte < 128) {
        return result;
      }
    }
    return this.fail(`Invalid ${bits}-bit varint`, start);
  }
  private structure(parent: number, depth: number): void {
    if (depth > 64) {
      throw new ParseError("limited", "Bond nesting limit reached (64 levels)", this.offset);
    }
    const outer = this.end;
    let truncated = false;
    if (this.version === 2) {
      const length = this.varint();
      truncated = length > this.end - this.offset;
      this.end = Math.min(this.end, this.offset + length);
    }
    try {
      while (this.offset < this.end) {
        const start = this.offset;
        const header = this.byte();
        const kind = header & 31;
        if (kind === 0) {
          if (header || (this.version === 2 && (truncated || this.offset !== this.end))) {
            this.fail("Invalid Bond struct terminator or length", start);
          }
          return;
        }
        if (kind === 1) {
          if (header !== 1) {
            this.fail("Invalid Bond base terminator", start);
          }
          const id = this.add(1, Role.Base, 0, depth, start);
          this.link(parent, id);
          this.finish(id, () => {});
          continue;
        }
        this.tag(kind);
        const encoded = header >>> 5;
        const label =
          encoded === 6 ? this.byte() : encoded === 7 ? this.byte() | (this.byte() << 8) : encoded;
        this.value(parent, kind, Role.Field, label, depth, start);
      }
      this.fail("Missing Bond struct terminator");
    } finally {
      this.end = outer;
    }
  }
  private value(
    parent: number,
    kind: number,
    role: Role,
    label: number,
    depth: number,
    start = this.offset,
  ): void {
    const id = this.add(kind, role, label, depth, start);
    this.link(parent, id);
    this.finish(id, () => {
      if (kind === 2) {
        if (this.byte() > 1) {
          this.fail("Invalid Bond Boolean", this.offset - 1);
        }
      } else if (kind === 3 || kind === 14) {
        this.byte();
      } else if ([4, 5, 6, 15, 16, 17].includes(kind)) {
        this.varint(kind === 4 || kind === 15 ? 16 : kind === 5 || kind === 16 ? 32 : 64);
      } else if (kind === 7 || kind === 8) {
        const length = kind === 7 ? 4 : 8;
        this.require(length);
        this.offset += length;
      } else if (kind === 9 || kind === 18) {
        const length = this.varint() * (kind === 18 ? 2 : 1);
        this.require(length);
        this.set(id, Column.Data, this.offset);
        const data = this.offset;
        this.offset += length;
        unicode(this.bytes, data, this.offset, kind === 18, false);
      } else if (kind === 10) {
        this.structure(id, depth + 1);
      } else if (kind === 11 || kind === 12) {
        const header = this.byte();
        const element = this.tag(header & 31);
        if (this.version === 1 && header > 31) {
          this.fail("Invalid Bond v1 container type", this.offset - 1);
        }
        const count = this.version === 2 && header >>> 5 ? (header >>> 5) - 1 : this.varint();
        this.set(id, Column.Meta, this.get(id, Column.Meta) | (element << 16));
        this.set(id, Column.Data, this.offset);
        const stride = fixedWidth(element);
        if (stride) {
          if (count && depth >= 64) {
            throw new ParseError("limited", "Bond nesting limit reached (64 levels)", this.offset);
          }
          const available = Math.min(count, Math.floor((this.end - this.offset) / stride));
          this.set(id, Column.Count, available);
          if (element === 2) {
            for (let index = 0; index < available; index++) {
              if (this.byte() > 1) {
                this.set(id, Column.Count, index);
                this.fail("Invalid Bond Boolean", this.offset - 1);
              }
            }
          } else {
            this.offset += available * stride;
          }
          if (available < count) {
            this.fail("Truncated value");
          }
        } else {
          for (let index = 0; index < count; index++) {
            this.value(id, element, Role.Item, index, depth + 1);
          }
        }
      } else if (kind === 13) {
        const key = this.tag(this.byte());
        const mapped = this.tag(this.byte());
        const count = this.varint();
        for (let index = 0; index < count; index++) {
          const entry = this.add(19, Role.Entry, index, depth + 1);
          this.link(id, entry);
          this.finish(entry, () => {
            this.value(entry, key, Role.Key, 0, depth + 1);
            this.value(entry, mapped, Role.Value, 0, depth + 1);
          });
        }
      }
    });
  }
}

function fixedWidth(kind: number): number {
  return [2, 3, 14].includes(kind) ? 1 : kind === 7 ? 4 : kind === 8 ? 8 : 0;
}

/** Validate the entire string, but allocate only a bounded display preview. */
function unicode(
  bytes: Uint8Array,
  start: number,
  end: number,
  wide: boolean,
  preview: boolean,
): { value: string; clipped: boolean } {
  let offset = start;
  let value = "";
  const next = (): number => {
    if (offset === end) {
      throw new ParseError("malformed", "Truncated Unicode character", offset);
    }
    return bytes[offset++]!;
  };
  while (offset < end) {
    const at = offset;
    let point = next();
    if (wide) {
      point |= next() << 8;
      if (point >= 0xd800 && point <= 0xdbff) {
        const low = next() | (next() << 8);
        if (low < 0xdc00 || low > 0xdfff) {
          throw new ParseError("malformed", "Invalid UTF-16 surrogate pair", at);
        }
        point = 0x10000 + ((point - 0xd800) << 10) + low - 0xdc00;
      } else if (point >= 0xdc00 && point <= 0xdfff) {
        throw new ParseError("malformed", "Unpaired UTF-16 surrogate", at);
      }
    } else if (point >= 128) {
      const count =
        point >= 0xc2 && point <= 0xdf
          ? 1
          : point >= 0xe0 && point <= 0xef
            ? 2
            : point >= 0xf0 && point <= 0xf4
              ? 3
              : -1;
      if (count < 0) {
        throw new ParseError("malformed", "Invalid UTF-8 leading byte", at);
      }
      point &= (1 << (6 - count)) - 1;
      for (let index = 0; index < count; index++) {
        const byte = next();
        if ((byte & 0xc0) !== 0x80) {
          throw new ParseError("malformed", "Invalid UTF-8 continuation", at);
        }
        point = (point << 6) | (byte & 63);
      }
      if (
        point < [0, 0x80, 0x800, 0x10000][count]! ||
        point > 0x10ffff ||
        (point >= 0xd800 && point <= 0xdfff)
      ) {
        throw new ParseError("malformed", "Invalid UTF-8 code point", at);
      }
    }
    if (preview) {
      const character = String.fromCodePoint(point);
      if (value.length + character.length > 1024) {
        return { value, clipped: true };
      }
      value += character;
    }
  }
  return { value, clipped: false };
}

type Range = {
  index: Index;
  parent: number;
  ids: number[] | undefined;
  from: number;
  to: number;
  flat: boolean;
  path: string[];
};

/** Bounded projection through the ordinary API-1 deferred-child contract. */
export class BondIndexViewer {
  private indexes = new Map<string, Index>();
  private tokens = new Map<string, Range>();
  private opened = new Set<string>();
  private bytes: Uint8Array = new Uint8Array(0);
  private sourceId = "";
  private nodes = 0;
  private characters = 0;
  reset(): void {
    this.indexes.clear();
    this.tokens.clear();
    this.opened.clear();
    this.bytes = new Uint8Array(0);
  }
  parse(bytes: Uint8Array, sourceId: string): VisualizationDocument {
    this.reset();
    const small = bytes.length <= 2 * 1024 * 1024 ? parse(bytes, sourceId) : undefined;
    if (small && small.status !== "limited") {
      return small;
    }
    this.bytes = bytes;
    this.sourceId = sourceId;
    return this.project();
  }
  expand(token: string): VisualizationDocument {
    const range = this.tokens.get(token);
    if (!range) {
      throw new Error("Expired Bond range. Reopen the visualizer to retry.");
    }
    // One open range path keeps the returned document bounded even after hours
    // of inspection. Previously visited ranges remain available for reloading.
    this.opened = new Set([...range.path, token]);
    return this.project();
  }
  private project(): VisualizationDocument {
    this.tokens.clear();
    this.nodes = 0;
    this.characters = 0;
    const result = parse(
      this.bytes,
      this.sourceId,
      (_bytes, _sourceId, version, start, sequence = false) => {
        const key = `${version}.${start}.${Number(sequence)}`;
        let index = this.indexes.get(key);
        if (!index) {
          index = new Index(this.bytes, this.sourceId, version, start, sequence);
          this.indexes.set(key, index);
        }
        const doc = document("Bond Compact Binary", summary);
        doc.roots = this.children(index, 0, [], 0);
        for (const root of doc.roots) {
          if (root.value?.kind === "array") {
            root.name = `Compact Binary v${version} · ${root.name}`;
          }
        }
        if (index.error) {
          doc.status = index.error.status;
          doc.diagnostics!.push({ code: index.error.status, message: index.error.message });
          doc.roots.push({
            id: `bond-index.${key}.remainder`,
            name: "Unparsed bytes",
            value: { kind: "bytes" },
            spans: [
              {
                sourceId: index.sourceId,
                offset: String(index.error.offset),
                length: String(index.bytes.length - index.error.offset),
              },
            ],
            children: [],
          });
        }
        return doc;
      },
    );
    // Candidate probes have separate tokens; expose only the returned document's
    // tokens, never an interpretation that was ruled out during version selection.
    const retained = new Set<string>();
    const visit = (node: Node): void => {
      if (node.childrenToken) {
        retained.add(node.childrenToken);
      }
      node.children?.forEach(visit);
    };
    result.roots.forEach(visit);
    for (const key of this.tokens.keys()) {
      if (!retained.has(key)) {
        this.tokens.delete(key);
      }
    }
    result.summary +=
      " Large structures load in ranges. Opening another range replaces the previously loaded range. Deep descendants are shown in wire order with their nesting depth. Full value bytes remain available in Hex.";
    return result;
  }
  private prefix(index: Index): string {
    return `bond-index.${index.version}.${index.start}.${Number(index.sequence)}`;
  }
  private span(index: Index, start: number, end: number) {
    return [{ sourceId: index.sourceId, offset: String(start), length: String(end - start) }];
  }
  private ids(index: Index, parent: number, flat: boolean): number[] {
    if (flat) {
      return Array.from(
        { length: index.get(parent, Column.After) - parent - 1 },
        (_, offset) => parent + offset + 1,
      );
    }
    const ids: number[] = [];
    for (let id = index.get(parent, Column.First); id; id = index.get(id, Column.Next)) {
      ids.push(id);
    }
    return ids;
  }
  private children(index: Index, parent: number, path: string[], depth: number): Node[] {
    const flat = depth >= 12;
    const virtual =
      !index.get(parent, Column.First) &&
      fixedWidth((index.get(parent, Column.Meta) >>> 16) & 255) > 0;
    const ids = virtual ? undefined : this.ids(index, parent, flat);
    const count = ids?.length ?? index.get(parent, Column.Count);
    return this.range({ index, parent, ids, from: 0, to: count, flat, path }, depth);
  }
  private range(range: Range, depth: number): Node[] {
    const { index, parent, ids, from, to, flat, path } = range;
    const count = to - from;
    if (count === 0) {
      return [];
    }
    if (count > pageSize) {
      let size = pageSize;
      while (Math.ceil(count / size) > fanout) {
        size *= fanout;
      }
      const nodes: Node[] = [];
      for (let start = from; start < to; start += size) {
        const end = Math.min(to, start + size);
        nodes.push(this.deferred({ ...range, from: start, to: end }, depth));
      }
      return nodes;
    }
    // A parent with many values or a deep subtree starts closed. An explicit
    // range expansion materializes at most 64 immediate values.
    if (this.nodes > 1500 || (count > 6 && path.length === 0)) {
      return [this.deferred(range, depth)];
    }
    const result: Node[] = [];
    for (let at = from; at < to; at++) {
      result.push(
        ids ? this.node(index, ids[at]!, path, depth, flat) : this.virtual(index, parent, at),
      );
    }
    return result;
  }
  private deferred(range: Range, depth: number): Node {
    const { index, parent, ids, from, to, flat } = range;
    const token = `${this.prefix(index)}.range.${parent}.${Number(flat)}.${from}.${to}`;
    const kind = index.get(parent, Column.Meta) & 255;
    const unit = flat
      ? "Descendants"
      : parent === 0
        ? "Structs"
        : [11, 12, 13].includes(kind)
          ? "Items"
          : "Fields";
    const start = ids
      ? index.get(ids[from]!, Column.Start)
      : index.get(parent, Column.Data) +
        from * fixedWidth((index.get(parent, Column.Meta) >>> 16) & 255);
    let end = ids
      ? index.get(ids[to - 1]!, Column.End)
      : start + (to - from) * fixedWidth((index.get(parent, Column.Meta) >>> 16) & 255);
    if (flat && ids) {
      for (let at = from; at < to; at++) {
        end = Math.max(end, index.get(ids[at]!, Column.End));
      }
    }
    const node: Node = {
      id: token,
      name: `${unit} ${from + 1}–${to}${flat ? " · flattened in wire order" : ""}`,
      value: { kind: "array" },
      spans: this.span(index, start, end),
      children: [],
    };
    if (this.opened.has(token) && this.nodes <= 1500) {
      node.children = this.range({ ...range, path: [...range.path, token] }, depth + 1);
    } else {
      node.childrenToken = token;
      this.tokens.set(token, range);
    }
    this.nodes++;
    return node;
  }
  private virtual(index: Index, parent: number, at: number): Node {
    const kind = (index.get(parent, Column.Meta) >>> 16) & 255;
    const start = index.get(parent, Column.Data) + at * fixedWidth(kind);
    this.nodes++;
    return {
      id: `${this.prefix(index)}.item.${parent}.${at}`,
      name: `[${at}] · ${names[kind]}`,
      value: this.scalar(index, kind, start),
      spans: this.span(index, start, start + fixedWidth(kind)),
      children: [],
    };
  }
  private scalar(index: Index, kind: number, data: number): Value {
    const reader = new Reader(index.bytes, index.sourceId, new Budget("display"), data);
    if (kind === 2) {
      return { kind: "boolean", value: reader.byte() === 1 };
    }
    if (kind === 3 || kind === 14) {
      const raw = reader.byte();
      return integer(kind === 14 && raw >= 128 ? raw - 256 : raw);
    }
    if (kind === 7 || kind === 8) {
      return floating(reader.floating(kind === 7 ? 4 : 8));
    }
    const raw = reader.varint(kind === 4 || kind === 15 ? 16 : kind === 5 || kind === 16 ? 32 : 64);
    return integer(kind >= 15 ? (raw >> 1n) ^ -(raw & 1n) : raw);
  }
  private node(index: Index, id: number, path: string[], depth: number, flat: boolean): Node {
    this.nodes++;
    const meta = index.get(id, Column.Meta);
    const kind = meta & 255;
    const role = (meta >>> 8) & 255;
    const label = index.get(id, Column.Label);
    const name =
      role === Role.Root
        ? index.get(0, Column.Count) > 1
          ? `Struct ${label + 1} · Compact Binary v${index.version}`
          : `Compact Binary v${index.version}`
        : role === Role.Field
          ? `Field ${label} · ${names[kind]}`
          : role === Role.Key
            ? `Key · ${names[kind]}`
            : role === Role.Value
              ? `Value · ${names[kind]}`
              : role === Role.Base
                ? "End of base struct"
                : `[${label}]${role === Role.Entry ? "" : ` · ${names[kind]}`}`;
    const start = index.get(id, Column.Start);
    const end = index.get(id, Column.End);
    const node: Node = {
      id: `${this.prefix(index)}.node.${id}`,
      name: `${flat ? `Depth ${index.get(id, Column.Depth)} · ` : ""}${meta & incomplete ? "Incomplete " : ""}${name}`,
      spans: this.span(index, start, end),
      children: [],
    };
    if ([1, 10, 11, 12, 13, 19].includes(kind)) {
      node.value = { kind: [11, 12, 13].includes(kind) ? "array" : "object" };
      if ([11, 12, 13].includes(kind)) {
        node.name += ` · ${index.get(id, Column.Count)} ${kind === 13 ? "entries" : "items"}`;
      }
      if (!flat || !index.get(id, Column.First)) {
        const count = index.get(id, Column.Count);
        if (count) {
          // Automatic expansion must not grow with the branching factor. A
          // single scalar costs the same as a deferred-range placeholder; all
          // other nested branches require an explicit range expansion.
          const singleLeaf =
            count === 1 &&
            index.get(id, Column.First) > 0 &&
            this.ids(index, id, false).every((child) => index.get(child, Column.Count) === 0);
          if ((depth < 1 || singleLeaf) && count <= 6 && this.nodes < 1500) {
            node.children = this.children(index, id, path, depth + 1);
          } else {
            const flatten = depth >= 12;
            const virtual = !index.get(id, Column.First);
            const ids = virtual ? undefined : this.ids(index, id, flatten);
            node.children = [
              this.deferred(
                { index, parent: id, ids, from: 0, to: ids?.length ?? count, flat: flatten, path },
                depth + 1,
              ),
            ];
          }
        }
      }
    } else if (meta & incomplete) {
      node.value = { kind: "bytes" };
    } else if (kind === 9 || kind === 18) {
      const decoded = unicode(index.bytes, index.get(id, Column.Data), end, kind === 18, true);
      if (this.characters + decoded.value.length > 32_768) {
        node.name += " · text available in Hex";
        node.value = { kind: "bytes" };
      } else {
        this.characters += decoded.value.length;
        node.value = { kind: "string", value: decoded.value };
        node.format = { charset: kind === 18 ? "utf-16le" : "utf-8" };
        if (decoded.clipped) {
          node.name += " · preview (first 1,024 characters)";
        }
      }
    } else {
      node.value = this.scalar(index, kind, index.get(id, Column.Data));
    }
    return node;
  }
}
