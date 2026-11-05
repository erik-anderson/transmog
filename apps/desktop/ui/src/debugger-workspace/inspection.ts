import type { DebugAction, DebugEvent, DebugValue } from "./models.js";
import { PropertyTree, type PropertyRow } from "./properties.js";

export interface InspectionScope {
  session: string;
  pause: number;
  frame: number | null;
}
export interface ValueView {
  key: string;
  label: string;
  value: DebugValue;
  expanded: boolean;
  rows: PropertyRow[];
  expired: boolean;
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
    const view: ValueView = { key, label, value, expanded: false, rows: [], expired: false };
    this.roots.set(key, { view, tree: new PropertyTree(), scope });
    return view;
  }
  get(key: string): ValueView | undefined {
    return this.roots.get(key)?.view;
  }
  async toggle(key: string, row?: PropertyRow): Promise<void> {
    const root = this.roots.get(key);
    if (!root || root.view.expired || !root.view.value.handle || !this.valid(root.scope)) {
      return;
    }
    let branch;
    if (row) {
      branch = root.tree.toggle(row);
    } else {
      root.view.expanded = !root.view.expanded;
      if (root.view.expanded && !root.view.rows.length) {
        branch = root.tree.select(root.view.value.handle);
      }
    }
    root.view.rows = root.tree.rows();
    this.changed();
    if (branch) {
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
            void this.send(
              { kind: "release", pause: root.scope.pause, handles },
              root.scope.session,
            );
          }
        }
        return;
      }
      if (event?.kind === "properties") {
        root.tree.receive(branch.key, event.properties, event.offset, event.next);
        root.view.rows = root.tree.rows();
      } else if (event?.kind === "rejected") {
        root.view.expired = true;
      }
      this.changed();
    }
  }
  expire(): void {
    for (const root of this.roots.values()) {
      if (!this.valid(root.scope)) {
        root.view.expired = true;
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
        void this.send(
          { kind: "release", pause: root.scope.pause, handles: [...handles].slice(0, 64) },
          root.scope.session,
        );
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
