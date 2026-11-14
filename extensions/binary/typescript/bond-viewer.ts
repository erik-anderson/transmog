import * as Wire from "./bond-wire.js";
import * as Limits from "./bond-limits.js";
import type { Node, Value, VisualizationDocument } from "transmog:extensions/v1";
import { parse } from "./bond-framing.js";
import { BondIndex, Column, Role, MAP_ENTRY_KIND } from "./bond-index.js";
import { scanText } from "./unicode.js";
import { DocumentLimits, Reader, document, floating, integer } from "./reader.js";
import { MAX_BINARY_INPUT_BYTES, TEXT_PREVIEW_LABEL } from "./limits.js";
type Range = {
  index: BondIndex;
  parent: number;
  ids: number[] | undefined;
  from: number;
  to: number;
  flat: boolean;
  path: string[];
};

/** Bounded projection through the ordinary API-1 deferred-child contract. */
export class BondIndexViewer {
  private generation = 0;
  private indexes = new Map<string, BondIndex>();
  private tokens = new Map<string, Range>();
  private opened = new Set<string>();
  private bytes: Uint8Array = new Uint8Array(0);
  private sourceId = "";
  private nodes = 0;
  private characters = 0;
  reset(): void {
    this.generation++;
    this.indexes.clear();
    this.tokens.clear();
    this.opened.clear();
    this.bytes = new Uint8Array(0);
  }
  parse(bytes: Uint8Array, sourceId: string): VisualizationDocument {
    this.reset();
    const small = bytes.length <= MAX_BINARY_INPUT_BYTES ? parse(bytes, sourceId) : undefined;
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
          index = new BondIndex(this.bytes, this.sourceId, version, start, sequence);
          this.indexes.set(key, index);
        }
        const doc = document("Bond Compact Binary", Wire.SUMMARY);
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
  private prefix(index: BondIndex): string {
    return `bond-index.${this.generation}.${index.version}.${index.start}.${Number(index.sequence)}`;
  }
  private span(index: BondIndex, start: number, end: number) {
    return [{ sourceId: index.sourceId, offset: String(start), length: String(end - start) }];
  }
  private ids(index: BondIndex, parent: number, flat: boolean): number[] {
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
  private children(index: BondIndex, parent: number, path: string[], depth: number): Node[] {
    const flat = depth >= Limits.FLATTEN_AT_TREE_DEPTH;
    const virtual =
      !index.get(parent, Column.First) && Wire.fixedWidth(index.elementType(parent)) > 0;
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
    if (count > Limits.RANGE_PAGE_SIZE) {
      let size = Limits.RANGE_PAGE_SIZE;
      while (Math.ceil(count / size) > Limits.RANGE_FANOUT) {
        size *= Limits.RANGE_FANOUT;
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
    if (
      this.nodes > Limits.PROJECTION_NODE_THRESHOLD ||
      (count > Limits.MAX_AUTOMATIC_CHILDREN && path.length === 0)
    ) {
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
    const kind = index.valueType(parent);
    const unit = flat
      ? "Descendants"
      : parent === 0
        ? "Structs"
        : [Wire.Type.List, Wire.Type.Set, Wire.Type.Map].includes(kind)
          ? "Items"
          : "Fields";
    const start = ids
      ? index.get(ids[from]!, Column.Start)
      : index.get(parent, Column.Data) + from * Wire.fixedWidth(index.elementType(parent));
    let end = ids
      ? index.get(ids[to - 1]!, Column.End)
      : start + (to - from) * Wire.fixedWidth(index.elementType(parent));
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
    if (this.opened.has(token) && this.nodes <= Limits.PROJECTION_NODE_THRESHOLD) {
      node.children = this.range({ ...range, path: [...range.path, token] }, depth + 1);
    } else {
      node.childrenToken = token;
      this.tokens.set(token, range);
    }
    this.nodes++;
    return node;
  }
  private virtual(index: BondIndex, parent: number, at: number): Node {
    const kind = index.elementType(parent);
    const start = index.get(parent, Column.Data) + at * Wire.fixedWidth(kind);
    this.nodes++;
    return {
      id: `${this.prefix(index)}.item.${parent}.${at}`,
      name: `[${at}] · ${Wire.TYPE_NAMES[kind]}`,
      value: this.scalar(index, kind, start),
      spans: this.span(index, start, start + Wire.fixedWidth(kind)),
      children: [],
    };
  }
  private scalar(index: BondIndex, kind: number, data: number): Value {
    const reader = new Reader(index.bytes, index.sourceId, new DocumentLimits("display"), data);
    if (kind === Wire.Type.Bool) {
      return { kind: "boolean", value: reader.byte() === 1 };
    }
    if (kind === Wire.Type.UInt8 || kind === Wire.Type.Int8) {
      const raw = reader.byte();
      return integer(
        kind === Wire.Type.Int8 && raw >= Wire.INT8_SIGN_BIT ? raw - Wire.UINT8_MODULUS : raw,
      );
    }
    if (kind === Wire.Type.Float32 || kind === Wire.Type.Float64) {
      return floating(
        reader.floating(kind === Wire.Type.Float32 ? Wire.FLOAT32_BYTES : Wire.FLOAT64_BYTES),
      );
    }
    const raw = reader.varint(Wire.integerBits(kind));
    return integer(Wire.isSignedInteger(kind) ? (raw >> 1n) ^ -(raw & 1n) : raw);
  }
  private node(index: BondIndex, id: number, path: string[], depth: number, flat: boolean): Node {
    this.nodes++;
    const kind = index.valueType(id);
    const role = index.rowRole(id);
    const label = index.get(id, Column.Label);
    const name =
      role === Role.Root
        ? index.get(0, Column.Count) > 1
          ? `Struct ${label + 1} · Compact Binary v${index.version}`
          : `Compact Binary v${index.version}`
        : role === Role.Field
          ? `Field ${label} · ${Wire.TYPE_NAMES[kind]}`
          : role === Role.Key
            ? `Key · ${Wire.TYPE_NAMES[kind]}`
            : role === Role.Value
              ? `Value · ${Wire.TYPE_NAMES[kind]}`
              : role === Role.Base
                ? "End of base struct"
                : `[${label}]${role === Role.Entry ? "" : ` · ${Wire.TYPE_NAMES[kind]}`}`;
    const start = index.get(id, Column.Start);
    const end = index.get(id, Column.End);
    const node: Node = {
      id: `${this.prefix(index)}.node.${id}`,
      name: `${flat ? `Depth ${index.get(id, Column.Depth)} · ` : ""}${index.isIncomplete(id) ? "Incomplete " : ""}${name}`,
      spans: this.span(index, start, end),
      children: [],
    };
    if (
      [
        Wire.Type.StopBase,
        Wire.Type.Struct,
        Wire.Type.List,
        Wire.Type.Set,
        Wire.Type.Map,
        MAP_ENTRY_KIND,
      ].includes(kind)
    ) {
      node.value = {
        kind: [Wire.Type.List, Wire.Type.Set, Wire.Type.Map].includes(kind) ? "array" : "object",
      };
      if ([Wire.Type.List, Wire.Type.Set, Wire.Type.Map].includes(kind)) {
        node.name += ` · ${index.get(id, Column.Count)} ${kind === Wire.Type.Map ? "entries" : "items"}`;
      }
      if (flat && index.get(id, Column.First)) {
        // Descendants are peer rows in this projection, not children of this
        // node. Expose its full encoded value rather than an apparently empty
        // object whose Copy subtree action would silently omit those peers.
        node.value = { kind: "bytes" };
        node.name += " · wire bytes (descendants shown separately)";
        return node;
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
          if (
            (depth < 1 || singleLeaf) &&
            count <= Limits.MAX_AUTOMATIC_CHILDREN &&
            this.nodes < Limits.PROJECTION_NODE_THRESHOLD
          ) {
            node.children = this.children(index, id, path, depth + 1);
          } else {
            const flatten = depth >= Limits.FLATTEN_AT_TREE_DEPTH;
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
    } else if (index.isIncomplete(id)) {
      node.value = { kind: "bytes" };
    } else if (kind === Wire.Type.String || kind === Wire.Type.WString) {
      const decoded = scanText(
        index.bytes,
        index.get(id, Column.Data),
        end,
        kind === Wire.Type.WString,
        "preview",
      );
      if (this.characters + decoded.value.length > Limits.MAX_PROJECTION_TEXT_UNITS) {
        node.name += " · text available in Hex";
        node.value = { kind: "bytes" };
      } else {
        this.characters += decoded.value.length;
        node.value = { kind: "string", value: decoded.value };
        node.format = { charset: kind === Wire.Type.WString ? "utf-16le" : "utf-8" };
        if (decoded.clipped) {
          node.name += TEXT_PREVIEW_LABEL;
        }
      }
    } else {
      node.value = this.scalar(index, kind, index.get(id, Column.Data));
    }
    return node;
  }
}
