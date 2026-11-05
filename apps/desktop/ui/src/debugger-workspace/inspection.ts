import type { DebugAction, DebugEvent, DebugValue } from "./models.js";
import { PropertyTree, type PropertyRow } from "./properties.js";

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
}
interface Root {
  view: ValueView;
  tree: PropertyTree;
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
    };
    this.roots.set(key, { view, tree: new PropertyTree(), scope });
    return view;
  }
  get(key: string): ValueView | undefined {
    return this.roots.get(key)?.view;
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
    root.view.rows = root.tree.rows();
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
          kind: "properties",
          pause: root.scope.pause,
          handle: branch.handle,
          offset: branch.offset,
        },
        root.scope,
      );
      if (this.roots.get(key) !== root || !this.valid(root.scope)) {
        if (event?.kind === "properties" && this.valid(root.scope)) {
          const handles = event.properties.flatMap((property) =>
            property.value.handle ? [property.value.handle] : [],
          );
          if (handles.length) {
            this.releaseLeases([{ scope: root.scope, handles }]);
          }
        }
        return;
      }
      if (event?.kind === "properties") {
        root.view.issue = "";
        root.tree.receive(branch.key, event.properties, event.offset, event.next);
        root.view.rows = root.tree.rows();
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
export class ConsoleHistory {
  entries: string[] = [];
  private index = 0;
  private draft = "";
  add(text: string): void {
    if (this.entries.at(-1) !== text) {
      this.entries.push(text);
    }
    let bytes = 0;
    this.entries = this.entries
      .slice(-100)
      .reverse()
      .filter((entry) => (bytes += entry.length) <= 128 * 1024)
      .reverse();
    this.index = this.entries.length;
    this.draft = "";
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
