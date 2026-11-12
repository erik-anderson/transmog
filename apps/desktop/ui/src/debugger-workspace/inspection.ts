import type { DebugAction, DebugEvent, DebugValue } from "./models.js";
import {
  isCompletePropertyPath,
  PropertyTree,
  valuePreview,
  type PropertyRow,
} from "./properties.js";

export interface InspectionScope {
  generation?: number;
  session: string;
  pause: number;
  frame: number | null;
}
export interface ValueLease {
  scope: InspectionScope;
  handles: number[];
}
export interface ValueView {
  key: string;
  label: string;
  value: DebugValue;
  expanded: boolean;
  rows: PropertyRow[];
  expired: boolean;
  loading: boolean;
  issue: string;
  preview: string;
  editable: boolean;
  epoch: string;
  restoring: boolean;
}
export interface InspectionState {
  open: boolean;
  paths: string[][];
  values: Array<[string, string]>;
  pages?: Array<[string[], number]>;
}
export interface ValueEditRequest {
  key: string;
  row: PropertyRow;
  expression: string;
  reply: (issue: string) => void;
}
export interface ValueWatchRequest {
  key: string;
  row: PropertyRow;
}
export interface ValueEditDraft {
  key: string;
  label: string;
  epoch: string;
  path: string[];
  text: string;
  issue?: string;
}
export interface ValueEditOutcome {
  issue: string;
  refresh: boolean;
}
interface Root {
  view: ValueView;
  tree: PropertyTree;
  previous?: Map<string, string>;
  restoration?: InspectionState;
  scope: InspectionScope;
}
interface Pending {
  receive: (event: DebugEvent) => void;
  timer: number;
}
/** One bounded registry routes native replies to their original value or prompt. */
export class Inspection {
  private releases = new Map<string, { scope: InspectionScope; handles: Set<number> }>();
  private releaseScheduled = false;
  private releasePending: Promise<void> = Promise.resolve();
  private releasing: ValueLease[] = [];
  private roots = new Map<string, Root>();
  private pending = new Map<string, Pending>();
  registering = 0;
  constructor(
    private send: (action: DebugAction, session: string) => Promise<number | undefined>,
    private valid: (scope: InspectionScope) => boolean,
    private changed: () => void,
  ) {}
  async request(action: DebugAction, scope: InspectionScope): Promise<DebugEvent | undefined> {
    if (!this.valid(scope) || this.pending.size + this.registering >= 24) {
      return;
    }
    this.registering++;
    let request: number | undefined;
    try {
      await this.releasePending;
      if (!this.valid(scope)) {
        return;
      }
      request = await this.send(action, scope.session);
    } finally {
      this.registering--;
    }
    if (request === undefined || !this.valid(scope)) {
      return;
    }
    return new Promise<DebugEvent | undefined>((resolve) => {
      const key = `${scope.session}/${request}`;
      const timer = window.setTimeout(() => {
        this.pending.delete(key);
        resolve(undefined);
      }, 2500);
      this.pending.set(key, { timer, receive: resolve });
    });
  }
  receive(session: string, request: number, event: DebugEvent): boolean {
    const key = `${session}/${request}`;
    const pending = this.pending.get(key);
    if (!pending) {
      return false;
    }
    if (event.kind === "evaluating" || event.kind === "output") {
      return true;
    }
    window.clearTimeout(pending.timer);
    this.pending.delete(key);
    pending.receive(event);
    return true;
  }
  set(key: string, label: string, value: DebugValue, scope: InspectionScope): ValueView {
    this.remove(key);
    const view: ValueView = {
      key,
      label,
      value,
      expanded: false,
      rows: [],
      expired: false,
      loading: false,
      issue: "",
      preview: valuePreview(value),
      editable: scope.pause > 0 && scope.frame !== null,
      epoch: JSON.stringify(scope),
      restoring: false,
    };
    this.roots.set(key, { view, tree: new PropertyTree(), scope });
    return view;
  }
  get(key: string): ValueView | undefined {
    return this.roots.get(key)?.view;
  }
  capture(key: string): InspectionState | undefined {
    const root = this.roots.get(key);
    if (!root) {
      return;
    }
    if (root.view.restoring && root.restoration) {
      return { ...root.restoration, open: root.view.expanded };
    }
    const unique = new Set<string>();
    const ambiguous = new Set<string>();
    for (const row of root.view.rows) {
      const path = JSON.stringify(row.path);
      if (unique.has(path)) {
        ambiguous.add(path);
      }
      unique.add(path);
    }
    let valueBudget = 32768;
    let pathBudget = 32768;
    const pages = new Map<string, number>();
    for (const row of root.view.rows) {
      if (row.propertyIndex === undefined) {
        continue;
      }
      const parent = JSON.stringify(row.path.slice(0, -1));
      pages.set(parent, Math.max(pages.get(parent) ?? 0, row.propertyIndex + 1));
    }
    return {
      open: root.view.expanded,
      paths: root.view.rows
        .filter(
          (row) =>
            row.expanded &&
            isCompletePropertyPath(row.path) &&
            !ambiguous.has(JSON.stringify(row.path)) &&
            (pathBudget -= JSON.stringify(row.path).length) >= 0,
        )
        .slice(0, 64)
        .map((row) => row.path),
      values: root.view.rows
        .filter(
          (row) =>
            row.propertyIndex !== undefined &&
            isCompletePropertyPath(row.path) &&
            !ambiguous.has(JSON.stringify(row.path)) &&
            (valueBudget -= JSON.stringify(row.path).length + row.preview.length) >= 0,
        )
        .slice(0, 256)
        .map((row) => [JSON.stringify(row.path), valuePreview(row.value)]),
      pages: [...pages].slice(0, 64).map(([path, count]) => [JSON.parse(path) as string[], count]),
    };
  }
  private rows(root: Root): PropertyRow[] {
    return root.tree.rows().map((row) => ({
      ...row,
      changed:
        root.previous?.has(JSON.stringify(row.path)) === true &&
        root.previous.get(JSON.stringify(row.path)) !== row.preview,
    }));
  }
  async restore(key: string, state: InspectionState): Promise<void> {
    const root = this.roots.get(key);
    if (!root) {
      return;
    }
    root.previous = new Map(state.values);
    if (!state.open) {
      return;
    }
    root.view.restoring = true;
    root.restoration = state;
    let reads = 0;
    const current = (): boolean =>
      this.roots.get(key) === root && this.valid(root.scope) && root.view.expanded;
    const pages = async (): Promise<void> => {
      for (const [path, count] of state.pages ?? []) {
        while (current() && reads < 64 && !root.view.issue) {
          const more = root.view.rows.find(
            (row) =>
              row.offset !== undefined &&
              row.offset < count &&
              JSON.stringify(row.path.slice(0, -1)) === JSON.stringify(path),
          );
          if (!more) {
            break;
          }
          reads++;
          await this.toggle(key, more);
        }
      }
    };
    try {
      await this.toggle(key);
      await pages();
      for (const path of [...state.paths].sort((a, b) => a.length - b.length)) {
        if (!isCompletePropertyPath(path)) {
          continue;
        }
        if (!current() || reads >= 64) {
          return;
        }
        let matches = root.view.rows.filter(
          (row) => JSON.stringify(row.path) === JSON.stringify(path),
        );
        while (!matches.length && current() && reads < 64 && !root.view.issue) {
          const more = root.view.rows.find(
            (row) =>
              row.offset !== undefined &&
              JSON.stringify(row.path.slice(0, -1)) === JSON.stringify(path.slice(0, -1)),
          );
          if (!more) {
            break;
          }
          reads++;
          await this.toggle(key, more);
          matches = root.view.rows.filter(
            (row) => JSON.stringify(row.path) === JSON.stringify(path),
          );
        }
        const row = matches.length === 1 ? matches[0] : undefined;
        if (row?.expandable && !row.expanded) {
          reads++;
          await this.toggle(key, row);
          await pages();
        }
      }
    } finally {
      root.view.restoring = false;
      delete root.restoration;
      if (this.roots.get(key) === root) {
        this.changed();
      }
    }
  }
  async edit(key: string, row: PropertyRow, expression: string): Promise<ValueEditOutcome> {
    const root = this.roots.get(key);
    if (!root || !this.valid(root.scope) || !root.view.editable || !row.writable) {
      return { issue: "This value is read-only or no longer available.", refresh: false };
    }
    const property = root.tree.property(row);
    if (
      !property ||
      !root.view.rows.some(
        (current) =>
          current.key === row.key && JSON.stringify(current.path) === JSON.stringify(row.path),
      )
    ) {
      return { issue: "This property is no longer available.", refresh: false };
    }
    const event = await this.request(
      {
        kind: "set-value",
        pause: root.scope.pause,
        frame: root.scope.frame!,
        ...property,
        expression,
      },
      root.scope,
    );
    return event?.kind === "acknowledged"
      ? { issue: "", refresh: true }
      : event?.kind === "rejected"
        ? { issue: event.message, refresh: false }
        : {
            issue: "The edit could not be confirmed. Inspect the current value before retrying.",
            refresh: true,
          };
  }
  leases(): ValueLease[] {
    const groups = new Map<string, { scope: InspectionScope; handles: Set<number> }>();
    const add = (scope: InspectionScope, handles: Iterable<number>): void => {
      const key = `${scope.session}/${scope.pause}/${scope.generation ?? ""}`;
      const group = groups.get(key) ?? { scope, handles: new Set<number>() };
      for (const handle of handles) {
        group.handles.add(handle);
      }
      groups.set(key, group);
    };
    for (const root of this.roots.values()) {
      add(root.scope, root.tree.handles());
      if (root.view.value.handle) {
        add(root.scope, [root.view.value.handle]);
      }
    }
    for (const release of this.releases.values()) {
      add(release.scope, release.handles);
    }
    for (const release of this.releasing) {
      add(release.scope, release.handles);
    }
    return [...groups.values()].map((group) => ({
      scope: group.scope,
      handles: [...group.handles].slice(0, 64),
    }));
  }
  releaseLeases(leases: ValueLease[]): void {
    for (const lease of leases) {
      if (!lease.handles.length || !this.valid(lease.scope)) {
        continue;
      }
      const key = `${lease.scope.session}/${lease.scope.pause}/${lease.scope.generation ?? ""}`;
      const group = this.releases.get(key) ?? { scope: lease.scope, handles: new Set<number>() };
      for (const handle of lease.handles) {
        group.handles.add(handle);
      }
      this.releases.set(key, group);
    }
    if (!this.releaseScheduled && this.releases.size) {
      this.releaseScheduled = true;
      const previous = this.releasePending;
      this.releasePending = new Promise((resolve) =>
        queueMicrotask(async () => {
          await previous;
          const groups = [...this.releases.values()];
          this.releasing = groups.map((group) => ({
            scope: group.scope,
            handles: [...group.handles],
          }));
          this.releases.clear();
          this.releaseScheduled = false;
          await Promise.allSettled(
            groups
              .filter((group) => this.valid(group.scope))
              .map((group) =>
                this.send(
                  {
                    kind: "release",
                    pause: group.scope.pause,
                    handles: [...group.handles].slice(0, 64),
                  },
                  group.scope.session,
                ),
              ),
          );
          this.releasing = [];
          resolve();
        }),
      );
    }
  }
  async toggle(key: string, row?: PropertyRow): Promise<void> {
    const root = this.roots.get(key);
    if (!root || root.view.expired || !root.view.value.handle || !this.valid(root.scope)) {
      return;
    }
    if (row && root.view.loading) {
      return;
    }
    const before = root.tree.handles();
    let selectedHandle: number | undefined;
    if (
      row &&
      row.expandable &&
      !row.expanded &&
      !row.value.handle &&
      row.rangeStart === undefined
    ) {
      const property = root.tree.property(row);
      if (!property) {
        return;
      }
      root.view.loading = true;
      root.view.issue = "Loading…";
      this.changed();
      const event = await this.request(
        { kind: "property", pause: root.scope.pause, ...property },
        root.scope,
      );
      root.view.loading = false;
      if (this.roots.get(key) !== root || !this.valid(root.scope) || !root.view.expanded) {
        if (event?.kind === "evaluated" && event.value.handle) {
          this.releaseLeases([{ scope: root.scope, handles: [event.value.handle] }]);
        }
        if (this.roots.get(key) === root && !root.view.expanded) {
          this.releaseLeases([
            {
              scope: root.scope,
              handles: [...root.tree.handles()].filter(
                (handle) => handle !== root.view.value.handle,
              ),
            },
          ]);
          root.tree.reset();
          root.view.rows = [];
          root.view.issue = "";
          this.changed();
        }
        return;
      }
      if (event?.kind !== "evaluated") {
        root.view.issue =
          event?.kind === "rejected" ? event.message : "Property is no longer available.";
        this.changed();
        return;
      }
      root.tree.update(row, event.value);
      root.view.rows = this.rows(root);
      if (!event.value.handle) {
        root.view.issue = ["object", "array", "function"].includes(event.value.kind)
          ? "Collapse another value before expanding this one."
          : event.value.kind === "unavailable"
            ? event.value.display
            : "";
        this.changed();
        return;
      }
      selectedHandle = event.value.handle;
      row = { ...row, value: event.value };
    }
    let branch;
    if (row) {
      branch = root.tree.toggle(row);
    } else {
      root.view.expanded = !root.view.expanded;
      if (root.view.expanded && !root.view.loading) {
        branch = root.tree.select(root.view.value.handle);
      } else if (!root.view.expanded && !root.view.loading) {
        root.tree.reset();
        root.view.issue = "";
      }
    }
    if (selectedHandle && !branch) {
      this.releaseLeases([{ scope: root.scope, handles: [selectedHandle] }]);
    }
    if (branch === null) {
      root.view.issue = "Collapse another value before expanding this one.";
    }
    const releaseRemoved = (handles: Set<number>): void => {
      const retained = root.tree.handles();
      this.releaseLeases([
        {
          scope: root.scope,
          handles: [...handles].filter(
            (handle) => handle !== root.view.value.handle && !retained.has(handle),
          ),
        },
      ]);
    };
    releaseRemoved(before);
    root.view.rows = this.rows(root);
    if (branch) {
      root.view.loading = true;
      root.view.issue = "Loading…";
    }
    this.changed();
    if (branch) {
      await this.releasePending;
      if (this.roots.get(key) !== root || !this.valid(root.scope)) {
        return;
      }
      if (!root.view.expanded) {
        root.view.loading = false;
        const handles = root.tree.handles();
        root.tree.reset();
        releaseRemoved(handles);
        root.view.rows = [];
        root.view.issue = "";
        this.changed();
        return;
      }
      const event = await this.request(
        {
          kind: "preview-properties",
          pause: root.scope.pause,
          handle: branch.handle,
          offset: branch.offset,
          limit: branch.limit,
          refresh: branch.refresh,
        },
        root.scope,
      );
      if (this.roots.get(key) !== root || !this.valid(root.scope)) {
        if (
          (event?.kind === "properties" || event?.kind === "property-preview") &&
          this.valid(root.scope)
        ) {
          const handles = event.properties.flatMap((property) =>
            property.value.handle ? [property.value.handle] : [],
          );
          if (handles.length) {
            this.releaseLeases([{ scope: root.scope, handles }]);
          }
        }
        return;
      }
      if (event?.kind === "properties" || event?.kind === "property-preview") {
        root.view.issue = "";
        root.tree.receive(branch.key, event.properties, event.offset, event.next, event.total);
        root.view.rows = this.rows(root);
      } else if (event?.kind === "rejected") {
        root.view.issue = event.message;
      } else {
        root.view.issue = "Inspection unavailable. Collapse and expand to retry.";
      }
      root.view.loading = false;
      if (!root.view.expanded) {
        const handles = root.tree.handles();
        root.tree.reset();
        releaseRemoved(handles);
        root.view.rows = [];
        root.view.issue = "";
      }
      this.changed();
    }
  }
  expire(): void {
    for (const root of this.roots.values()) {
      if (!this.valid(root.scope)) {
        root.view.expired = true;
        root.view.loading = false;
        root.view.expanded = false;
        root.view.rows = [];
        root.tree.reset();
      }
    }
  }
  remove(key: string): void {
    const root = this.roots.get(key);
    this.roots.delete(key);
    if (root && this.valid(root.scope)) {
      const handles = root.tree.handles();
      if (root.view.value.handle) {
        handles.add(root.view.value.handle);
      }
      if (handles.size) {
        this.releaseLeases([{ scope: root.scope, handles: [...handles] }]);
      }
    }
  }
  dispose(): void {
    for (const key of this.roots.keys()) {
      this.remove(key);
    }
    for (const pending of this.pending.values()) {
      window.clearTimeout(pending.timer);
      pending.receive({ kind: "rejected", message: "Debugger closed" });
    }
    this.pending.clear();
  }
}

/** Prompt history preserves a draft when moving back from the newest command. */
export interface ConsoleHistoryState {
  entries: string[];
  index: number;
  draft: string;
}
export class ConsoleHistory {
  entries: string[] = [];
  private index = 0;
  private draft = "";
  snapshot(): ConsoleHistoryState {
    return { entries: [...this.entries], index: this.index, draft: this.draft };
  }
  restore(state: ConsoleHistoryState): void {
    this.entries = [...state.entries];
    this.index = Math.max(0, Math.min(this.entries.length, state.index));
    this.draft = state.draft;
  }
  add(text: string, { preserveNavigation = false } = {}): void {
    const previousIndex = this.index;
    const atEnd = previousIndex === this.entries.length;
    if (this.entries.at(-1) !== text) {
      this.entries.push(text);
    }
    const beforeTrimming = this.entries.length;
    let bytes = 0;
    this.entries = this.entries
      .slice(-100)
      .reverse()
      .filter((entry) => (bytes += entry.length) <= 128 * 1024)
      .reverse();
    this.index =
      preserveNavigation && !atEnd
        ? Math.max(0, previousIndex - (beforeTrimming - this.entries.length))
        : this.entries.length;
    if (!preserveNavigation) {
      this.draft = "";
    }
  }
  move(direction: number, current: string): string {
    if (this.index === this.entries.length) {
      this.draft = current;
    }
    this.index = Math.max(0, Math.min(this.entries.length, this.index + direction));
    return this.entries[this.index] ?? this.draft;
  }
  reset(): void {
    this.index = this.entries.length;
  }
}
