import type { DebugProperty, DebugValue } from "./models.js";

export interface PropertyRow extends DebugProperty {
  label: string;
  preview: string;
  key: string;
  depth: number;
  expanded: boolean;
  expandable: boolean;
  branch: string;
  offset?: number;
  propertyIndex?: number;
  rangeStart?: number;
  rangeEnd?: number;
}
export interface PropertyQuery {
  key: string;
  handle: number;
  offset: number;
  limit: number;
  refresh: boolean;
}
interface Branch {
  handle: number;
  path: string[];
  properties: DebugProperty[];
  start: number;
  end: number | null;
  total: number;
  next: number | null;
  grouped: boolean;
}
function inline(text: string): string {
  return text
    .replaceAll("\r", "\\r")
    .replaceAll("\n", "\\n")
    .replaceAll("\t", "\\t")
    .replaceAll("\u2028", "\\u2028")
    .replaceAll("\u2029", "\\u2029");
}
/** Property reads append to a tree. Large collections use bounded, expandable ranges. */
export class PropertyTree {
  private branches = new Map<string, Branch>();
  private expanded = new Set<string>();
  private root = "";
  reset(): void {
    this.branches.clear();
    this.expanded.clear();
    this.root = "";
  }
  handles(): Set<number> {
    const handles = new Set<number>();
    for (const branch of this.branches.values()) {
      handles.add(branch.handle);
      for (const property of branch.properties) {
        if (property.value.handle) {
          handles.add(property.value.handle);
        }
      }
    }
    return handles;
  }
  select(handle: number): PropertyQuery {
    this.reset();
    this.root = JSON.stringify([]);
    return this.open(handle, [])!;
  }
  private open(
    handle: number,
    path: string[],
    start = 0,
    end: number | null = null,
    refresh = true,
  ): PropertyQuery | undefined {
    const key = JSON.stringify(path);
    if (this.branches.size >= 64 || path.length > 16 || this.branches.has(key)) {
      return;
    }
    this.branches.set(key, {
      handle,
      path,
      properties: [],
      start,
      end,
      total: 0,
      next: null,
      grouped: false,
    });
    return {
      key,
      handle,
      offset: start,
      limit: Math.min(100, (end ?? start + 100) - start),
      refresh,
    };
  }
  receive(
    key: string,
    properties: DebugProperty[],
    offset: number,
    next: number | null,
    total?: number,
  ): void {
    const branch = this.branches.get(key);
    if (!branch) {
      return;
    }
    branch.total = Math.min(
      total ?? (next === null ? offset + properties.length : next + 1),
      branch.end ?? Infinity,
    );
    branch.grouped = branch.total - branch.start > 1000;
    if (branch.grouped) {
      branch.properties = [];
      branch.next = null;
      return;
    }
    if (offset !== branch.start + branch.properties.length) {
      return;
    }
    branch.properties.push(...properties);
    branch.next = next !== null && next < branch.total ? next : null;
  }
  property(row: PropertyRow): { handle: number; index: number } | undefined {
    const branch = this.branches.get(row.branch);
    return branch && row.propertyIndex !== undefined
      ? { handle: branch.handle, index: row.propertyIndex }
      : undefined;
  }
  update(row: PropertyRow, value: DebugValue): void {
    const branch = this.branches.get(row.branch);
    if (branch && row.propertyIndex !== undefined) {
      const property = branch.properties[row.propertyIndex - branch.start];
      if (property) {
        property.value = { ...value, handle: null };
      }
    }
  }
  private remove(path: string[]): void {
    const matches = (candidate: string[]): boolean =>
      candidate.length >= path.length && path.every((part, index) => candidate[index] === part);
    for (const [key, branch] of this.branches) {
      if (matches(branch.path)) {
        this.branches.delete(key);
      }
    }
    for (const key of this.expanded) {
      if (matches(JSON.parse(key) as string[])) {
        this.expanded.delete(key);
      }
    }
  }
  /** Null reports an expansion limit; undefined represents collapse or a stale row. */
  toggle(row: PropertyRow): PropertyQuery | null | undefined {
    const branch = this.branches.get(row.branch);
    if (!branch) {
      return;
    }
    if (row.offset !== undefined) {
      if (branch.next !== row.offset) {
        return;
      }
      return {
        key: row.branch,
        handle: branch.handle,
        offset: row.offset,
        limit: Math.min(100, branch.total - row.offset),
        refresh: false,
      };
    }
    if (this.expanded.delete(row.key)) {
      this.remove(JSON.parse(row.key) as string[]);
      return;
    }
    if (this.expanded.size >= 64 || this.branches.size >= 64) {
      return null;
    }
    const path = JSON.parse(row.key) as string[];
    const query =
      row.rangeStart !== undefined
        ? this.open(branch.handle, path, row.rangeStart, row.rangeEnd ?? null, false)
        : row.value.handle
          ? this.open(row.value.handle, path)
          : undefined;
    if (query) {
      this.expanded.add(row.key);
    }
    return query ?? null;
  }
  rows(): PropertyRow[] {
    const rows: Omit<PropertyRow, "label" | "preview">[] = [];
    const visit = (key: string): void => {
      const branch = this.branches.get(key);
      if (!branch) {
        return;
      }
      const base = { branch: key, depth: branch.path.length };
      if (branch.grouped) {
        let size = 100;
        while ((branch.total - branch.start) / size > 100) {
          size *= 100;
        }
        for (let start = branch.start; start < branch.total; start += size) {
          const end = Math.min(start + size, branch.total);
          const path = JSON.stringify([...branch.path, `range:${start}:${end}`]);
          const expanded = this.expanded.has(path);
          rows.push({
            ...base,
            name: `Properties ${start + 1}–${end}`,
            key: path,
            value: { kind: "", display: "", handle: null, truncated: false },
            expanded,
            expandable: branch.path.length < 16,
            rangeStart: start,
            rangeEnd: end,
          });
          if (expanded) {
            visit(path);
          }
        }
      } else {
        for (const [index, property] of branch.properties.entries()) {
          const propertyIndex = branch.start + index;
          const path = JSON.stringify([...branch.path, String(propertyIndex)]);
          const expanded = this.expanded.has(path);
          rows.push({
            ...base,
            ...property,
            key: path,
            propertyIndex,
            expanded,
            expandable:
              branch.path.length < 16 &&
              (property.value.handle !== null ||
                ["object", "array", "function"].includes(property.value.kind)),
          });
          if (expanded) {
            visit(path);
          }
        }
        if (branch.next !== null) {
          rows.push({
            ...base,
            name: "Show more properties",
            key: `${key}:more`,
            value: { kind: "", display: "", handle: null, truncated: false },
            offset: branch.next,
            expandable: false,
            expanded: false,
          });
        }
      }
    };
    visit(this.root);
    return rows.map((row) => ({
      ...row,
      label: inline(
        row.name === "" || /[\r\n\t\u2028\u2029"\\]/u.test(row.name)
          ? JSON.stringify(row.name)
          : row.name,
      ),
      preview: inline(
        row.value.kind === "string" ? JSON.stringify(row.value.display) : row.value.display,
      ),
    }));
  }
}
