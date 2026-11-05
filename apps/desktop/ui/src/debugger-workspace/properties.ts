import type { DebugProperty } from "./models.js";

export interface PropertyRow extends DebugProperty {
  key: string;
  depth: number;
  expanded: boolean;
  expandable: boolean;
  branch: string;
  offset?: number;
}
interface Branch {
  handle: number;
  path: string[];
  properties: DebugProperty[];
  offset: number;
  next: number | null;
  pages: number[];
}
/** Inert property pages. Only visible pages and expanded branches retain handles. */
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
  select(handle: number): { key: string; handle: number; offset: number } {
    this.reset();
    this.root = JSON.stringify([]);
    return this.open(handle, [])!;
  }
  private open(handle: number, path: string[]) {
    const key = JSON.stringify(path);
    if (this.branches.size >= 64 || path.length > 16 || this.branches.has(key)) {
      return;
    }
    this.branches.set(key, { handle, path, properties: [], offset: 0, next: null, pages: [0] });
    return { key, handle, offset: 0 };
  }
  receive(key: string, properties: DebugProperty[], offset: number, next: number | null): void {
    const branch = this.branches.get(key);
    if (!branch) {
      return;
    }
    branch.properties = properties;
    branch.offset = offset;
    branch.next = next;
    if (!branch.pages.includes(offset)) {
      branch.pages.push(offset);
    }
    branch.pages.sort((a, b) => a - b);
  }
  private remove(path: string[], includeSelf: boolean): void {
    const matches = (candidate: string[]): boolean =>
      candidate.length >= path.length && path.every((part, index) => candidate[index] === part);
    for (const [key, branch] of this.branches) {
      if (matches(branch.path) && (includeSelf || branch.path.length > path.length)) {
        this.branches.delete(key);
      }
    }
    for (const key of this.expanded) {
      const parts = JSON.parse(key) as string[];
      if (matches(parts) && (includeSelf || parts.length > path.length)) {
        this.expanded.delete(key);
      }
    }
  }
  toggle(row: PropertyRow) {
    const branch = this.branches.get(row.branch);
    if (row.offset !== undefined && branch) {
      this.remove(branch.path, false);
      branch.properties = [];
      branch.next = null;
      return { key: row.branch, handle: branch.handle, offset: row.offset };
    }
    if (!row.value.handle) {
      return;
    }
    if (this.expanded.delete(row.key)) {
      this.remove(JSON.parse(row.key) as string[], true);
      return;
    }
    if (this.expanded.size >= 256) {
      return;
    }
    this.expanded.add(row.key);
    return this.open(row.value.handle, JSON.parse(row.key) as string[]);
  }
  rows(): PropertyRow[] {
    const rows: PropertyRow[] = [];
    const visit = (key: string): void => {
      const branch = this.branches.get(key);
      if (!branch) {
        return;
      }
      const page = (name: string, offset: number): PropertyRow => ({
        name,
        key: `${key}:${name}`,
        branch: key,
        depth: branch.path.length,
        value: { kind: "", display: "", handle: null, truncated: false },
        offset,
        expandable: false,
        expanded: false,
      });
      if (branch.offset > 0) {
        rows.push(
          page(
            "Previous properties",
            branch.pages.filter((offset) => offset < branch.offset).at(-1) ?? 0,
          ),
        );
      }
      for (const [index, property] of branch.properties.entries()) {
        // Preview names can be identical (symbols or shortened long keys).
        const path = JSON.stringify([...branch.path, String(branch.offset + index)]);
        const expanded = this.expanded.has(path);
        rows.push({
          ...property,
          key: path,
          branch: key,
          depth: branch.path.length,
          expanded,
          expandable: property.value.handle !== null && branch.path.length < 16,
        });
        if (expanded) {
          visit(path);
        }
      }
      if (branch.next !== null) {
        rows.push(page("Next properties", branch.next));
      }
    };
    visit(this.root);
    return rows;
  }
}
