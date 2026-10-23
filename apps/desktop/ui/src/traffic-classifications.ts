import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ExtensionTrafficColumn, TrafficClassification } from "./models.js";

export type ClassificationCell = { key: string; text: string; title: string };
/** Optional viewport work. It never runs in the traffic query or row-render transaction. */
export class TrafficClassifications {
  choices: ExtensionTrafficColumn[] = [];
  private catalogError = "";
  private rowError = "";
  private observerError = "";
  get error(): string {
    return this.catalogError || this.observerError || this.rowError;
  }
  private index = new Map<string, ExtensionTrafficColumn>();
  private selectedKeys: string[] | undefined;
  private resolved: ExtensionTrafficColumn[] = [];
  private placeholders: ClassificationCell[] = [];
  private generation = 0;
  private catalogGeneration = 0;
  private connected = false;
  private lifetime = 0;
  private loading = false;
  private idle: number | undefined;
  private latest: string[] = [];
  private unlisten: UnlistenFn | undefined;
  private cache = new Map<string, ClassificationCell[]>();
  private stale = new Set<string>();
  private pending = new Set<string>();
  constructor(
    private selected: () => string[],
    private columnsChanged: () => void,
    private rowsChanged: () => void,
  ) {}
  get columns(): ExtensionTrafficColumn[] {
    const keys = this.selected();
    if (keys !== this.selectedKeys) {
      this.selectedKeys = keys;
      this.resolved = keys.flatMap((key) => {
        const column = this.index.get(key);
        return column ? [column] : [];
      });
      this.placeholders = this.resolved.map((column) => ({
        key: column.key,
        text: "…",
        title: column.name + " · " + column.extensionName + " · Loading header classifications",
      }));
    }
    return this.resolved;
  }
  private setChoices(choices: ExtensionTrafficColumn[]): void {
    this.choices = choices;
    this.index = new Map(choices.map((column) => [column.key, column]));
    this.selectedKeys = undefined;
  }
  async connect(): Promise<void> {
    if (this.connected) {
      return;
    }
    this.connected = true;
    const lifetime = ++this.lifetime;
    this.observerError = "";
    try {
      const unlisten = await listen("extensions-changed", () => {
        void this.refresh();
      });
      if (this.connected && lifetime === this.lifetime) {
        this.unlisten = unlisten;
      } else {
        unlisten();
      }
    } catch {
      if (this.connected && lifetime === this.lifetime) {
        this.observerError = "Extension changes could not be observed. Retry to reconnect.";
      }
    }
    if (this.connected && lifetime === this.lifetime) {
      await this.refresh();
    }
  }
  private cancelIdle(): void {
    if (this.idle !== undefined) {
      cancelIdleCallback(this.idle);
    }
    this.idle = undefined;
  }
  disconnect(): void {
    this.connected = false;
    this.lifetime++;
    this.generation++;
    this.catalogGeneration++;
    this.unlisten?.();
    this.unlisten = undefined;
    this.cancelIdle();
    this.latest = [];
    this.cache.clear();
    this.stale.clear();
    this.pending.clear();
  }
  invalidate(ids?: string[]): void {
    this.generation++;
    this.cancelIdle();
    this.pending.clear();
    if (ids) {
      for (const id of ids) {
        if (this.cache.has(id)) {
          this.stale.add(id);
        }
      }
    } else {
      this.cache.clear();
      this.stale.clear();
    }
  }
  async refresh(): Promise<void> {
    const generation = ++this.catalogGeneration;
    try {
      const choices = await invoke<ExtensionTrafficColumn[]>("extension_traffic_columns");
      if (!this.connected || generation !== this.catalogGeneration) {
        return;
      }
      // Traffic hints invalidate row labels, never metadata discovery. Keep the
      // last catalog visible until its replacement is ready.
      this.invalidate();
      this.setChoices(choices);
      this.catalogError = "";
      this.rowError = "";
      this.columnsChanged();
    } catch {
      if (this.connected && generation === this.catalogGeneration) {
        this.catalogError = "Extension columns could not be loaded. Retry to check for changes.";
        this.columnsChanged();
      }
    }
  }
  cells(id: string): ClassificationCell[] {
    return this.cache.get(id) ?? this.placeholders;
  }
  schedule(ids: string[]): void {
    this.latest = ids;
    if (
      !this.connected ||
      !this.resolved.length ||
      this.loading ||
      this.idle !== undefined ||
      !ids.some((id) => !this.cache.has(id) || this.stale.has(id))
    ) {
      return;
    }
    this.idle = requestIdleCallback(
      () => {
        this.idle = undefined;
        void this.load(this.latest);
      },
      { timeout: 250 },
    );
  }
  private present(row: TrafficClassification): ClassificationCell[] {
    const cells = this.resolved.map((column) => {
      const text = row.limited ? "Limited" : (row.values[column.key] ?? []).join(" · ") || "—";
      return {
        key: column.key,
        text,
        title: row.limited
          ? "Header classification exceeded its metadata limit."
          : column.name +
            " · " +
            column.extensionName +
            " · " +
            text +
            " · Fixed labels matched from retained headers",
      };
    });
    const previous = this.cache.get(row.id);
    return previous?.length === cells.length &&
      previous.every(
        (cell, index) =>
          cell.key === cells[index]!.key &&
          cell.text === cells[index]!.text &&
          cell.title === cells[index]!.title,
      )
      ? previous
      : cells;
  }
  async load(ids: string[]): Promise<void> {
    if (!this.connected || !this.resolved.length || this.loading) {
      return;
    }
    const missing = ids
      .filter((id) => (!this.cache.has(id) || this.stale.has(id)) && !this.pending.has(id))
      .slice(0, 100);
    if (!missing.length) {
      return;
    }
    const generation = this.generation,
      keys = this.resolved.map((column) => column.key);
    this.loading = true;
    let changed = false;
    for (const id of missing) {
      this.pending.add(id);
    }
    try {
      const rows = await invoke<TrafficClassification[]>("classify_traffic_rows", {
        ids: missing,
        columns: keys,
      });
      if (!this.connected || generation !== this.generation) {
        return;
      }
      const returned = new Map(rows.map((row) => [row.id, row]));
      for (const id of missing) {
        const cells = this.present(returned.get(id) ?? { id, values: {}, limited: false });
        changed ||= cells !== this.cache.get(id);
        this.cache.delete(id);
        this.cache.set(id, cells);
        this.stale.delete(id);
      }
      while (this.cache.size > 1024) {
        const first = this.cache.keys().next().value!;
        this.cache.delete(first);
        this.stale.delete(first);
      }
      changed ||= !!this.rowError;
      this.rowError = "";
    } catch {
      if (this.connected && generation === this.generation) {
        this.rowError =
          "Header classifications could not be loaded. Retry to check the retained headers.";
        for (const id of missing) {
          this.cache.set(id, this.present({ id, values: {}, limited: true }));
          this.stale.delete(id);
        }
        changed = true;
      }
    } finally {
      this.loading = false;
      while (this.cache.size > 1024) {
        const first = this.cache.keys().next().value!;
        this.cache.delete(first);
        this.stale.delete(first);
      }
      if (generation === this.generation) {
        for (const id of missing) {
          this.pending.delete(id);
        }
      }
      if (this.connected) {
        if (changed) {
          this.rowsChanged();
        }
        this.schedule(this.latest);
      }
    }
  }
}
