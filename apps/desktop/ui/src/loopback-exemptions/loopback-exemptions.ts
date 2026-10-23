import { WebUIElement, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import initialState from "../initial-state.json";
import { describeError } from "../utilities.js";

interface AppContainer {
  displayName: string;
  description: string;
  package: string;
  name: string;
  sid: string;
  users: string[];
  exempt: boolean;
}
type SortKey = "displayName" | "description" | "package" | "name" | "sid" | "users" | "exempt";
type Row = AppContainer & {
  usersText: string;
  exemptionText: string;
  selectionLabel: string;
  selected: boolean;
  selectionState: string;
  tabIndex: string;
};

export class LoopbackExemptions extends WebUIElement {
  @observable loopbackRows: Row[] = [];
  @observable loopbackBusy = initialState.loopbackBusy;
  @observable loopbackCurrent = initialState.loopbackCurrent;
  @observable loopbackStatus = initialState.loopbackStatus;
  @observable loopbackError = initialState.loopbackError;
  @observable loopbackQuery = initialState.loopbackQuery;
  @observable loopbackExemptOnly = initialState.loopbackExemptOnly;
  @observable loopbackSelected = initialState.loopbackSelected;
  @observable loopbackAddCount = initialState.loopbackAddCount;
  @observable loopbackRemoveCount = initialState.loopbackRemoveCount;
  @observable loopbackActionText = initialState.loopbackActionText;
  @observable loopbackSelectText = initialState.loopbackSelectText;
  @observable loopbackSelectionText = initialState.loopbackSelectionText;
  @observable loopbackCountText = initialState.loopbackCountText;
  @observable loopbackSort: SortKey = initialState.loopbackSort as SortKey;
  @observable loopbackDirection = initialState.loopbackDirection;
  @observable loopbackSortStates: Record<SortKey, string> = initialState.loopbackSortStates;
  exemptionDialog!: HTMLDialogElement;
  containerTable!: HTMLTableElement;
  private containers: AppContainer[] = [];
  private selected = new Set<string>();
  private anchor = "";
  private focused = "";
  private opener: HTMLElement | null = null;
  private generation = 0;
  private readonly collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

  async open(event?: Event): Promise<void> {
    this.opener = event?.currentTarget instanceof HTMLElement ? event.currentTarget : null;
    if (!this.exemptionDialog.open) {
      this.exemptionDialog.showModal();
    }
    if (!this.loopbackBusy) {
      await this.refresh();
    }
  }
  close(): void {
    this.exemptionDialog.close();
  }
  closed(): void {
    if (this.opener?.isConnected && this.opener.getClientRects().length) {
      this.opener.focus({ preventScroll: true });
    }
  }
  private accept(containers: AppContainer[]): void {
    this.containers = containers;
    const available = new Set(containers.map((row) => row.sid));
    this.selected = new Set([...this.selected].filter((sid) => available.has(sid)));
    this.loopbackCurrent = true;
    this.render();
  }
  async refresh(): Promise<void> {
    if (this.loopbackBusy) {
      return;
    }
    const generation = ++this.generation;
    this.loopbackBusy = true;
    this.loopbackError = "";
    this.loopbackStatus = "Reading Windows AppContainers and exemptions…";
    try {
      const containers = await invoke<AppContainer[]>("app_containers");
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.accept(containers);
      this.loopbackStatus = "";
    } catch (error: unknown) {
      if (generation === this.generation) {
        this.loopbackCurrent = false;
        this.loopbackError = describeError(error);
        this.loopbackStatus = "Refresh to read the current Windows state.";
      }
    } finally {
      if (generation === this.generation) {
        this.loopbackBusy = false;
      }
    }
  }
  async apply(enabled: boolean): Promise<void> {
    if (this.loopbackBusy || !this.loopbackCurrent || !this.selected.size) {
      return;
    }
    const sids = this.containers
      .filter((row) => this.selected.has(row.sid) && row.exempt !== enabled)
      .map((row) => row.sid);
    if (!sids.length) {
      return;
    }
    const generation = ++this.generation;
    this.loopbackBusy = true;
    this.loopbackError = "";
    this.loopbackStatus =
      "Waiting for Windows approval and applying the selection… You can close this dialog while it finishes.";
    try {
      const containers = await invoke<AppContainer[]>("set_loopback_exemptions", {
        change: { sids, enabled },
      });
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.accept(containers);
      this.loopbackStatus =
        (enabled ? "Enabled loopback access for " : "Removed loopback exemptions from ") +
        sids.length +
        " AppContainer" +
        (sids.length === 1 ? "" : "s") +
        ".";
    } catch (error: unknown) {
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.loopbackError = describeError(error);
      this.loopbackCurrent = false;
      // A write can succeed before verification fails. Reconcile with Windows,
      // keeping the error visible and without reopening or focusing the dialog.
      try {
        const containers = await invoke<AppContainer[]>("app_containers");
        if (generation !== this.generation || !this.isConnected) {
          return;
        }
        this.accept(containers);
      } catch {
        this.loopbackStatus = "Refresh to read the current Windows state.";
      }
      if (this.loopbackCurrent) {
        this.loopbackStatus = "Current Windows state refreshed.";
      }
    } finally {
      if (generation === this.generation) {
        this.loopbackBusy = false;
      }
    }
  }
  search(event: Event): void {
    this.loopbackQuery = (event.currentTarget as HTMLInputElement).value;
    this.render();
  }
  filterExempt(event: Event): void {
    this.loopbackExemptOnly = (event.currentTarget as HTMLInputElement).checked;
    this.render();
  }
  sortBy(key: SortKey): void {
    this.loopbackDirection =
      this.loopbackSort === key && this.loopbackDirection === "ascending"
        ? "descending"
        : "ascending";
    this.loopbackSort = key;
    this.render();
  }
  private value(row: AppContainer, key: SortKey): string {
    if (key === "users") {
      return row.users.join(" ");
    }
    if (key === "exempt") {
      return row.exempt ? "Enabled" : "Not exempt";
    }
    return row[key];
  }
  private render(): void {
    const query = this.loopbackQuery.trim().toLocaleLowerCase();
    const visible = this.containers.filter(
      (row) =>
        (!this.loopbackExemptOnly || row.exempt) &&
        (!query ||
          [
            row.displayName,
            row.description,
            row.package,
            row.name,
            row.sid,
            row.users.join(" "),
            this.value(row, "exempt"),
          ]
            .join(" ")
            .toLocaleLowerCase()
            .includes(query)),
    );
    visible.sort(
      (left, right) =>
        (this.loopbackDirection === "ascending" ? 1 : -1) *
          this.collator.compare(
            this.value(left, this.loopbackSort),
            this.value(right, this.loopbackSort),
          ) || this.collator.compare(left.sid, right.sid),
    );
    if (!visible.some((row) => row.sid === this.focused)) {
      this.focused = visible[0]?.sid ?? "";
    }
    this.loopbackSortStates = Object.fromEntries(
      Object.keys(this.loopbackSortStates).map((key) => [
        key,
        key === this.loopbackSort ? this.loopbackDirection : "none",
      ]),
    ) as Record<SortKey, string>;
    this.loopbackRows = visible.map((row) => ({
      ...row,
      usersText: row.users.join("\n") || "Unavailable",
      exemptionText: row.exempt ? "Enabled" : "Not exempt",
      selectionLabel: "Select " + row.displayName,
      selected: this.selected.has(row.sid),
      selectionState: String(this.selected.has(row.sid)),
      tabIndex: row.sid === this.focused ? "0" : "-1",
    }));
    this.loopbackSelected = this.selected.size;
    const selected = this.containers.filter((row) => this.selected.has(row.sid));
    const exempt = selected.filter((row) => row.exempt);
    this.loopbackAddCount = selected.length - exempt.length;
    this.loopbackRemoveCount = exempt.length;
    this.loopbackSelectText = query || this.loopbackExemptOnly ? "Select filtered" : "Select all";
    const targets =
      exempt.length <= 3
        ? new Intl.ListFormat(undefined, { style: "long", type: "conjunction" }).format(
            exempt.map((row) => row.displayName),
          )
        : exempt.length + " selected AppContainers";
    this.loopbackActionText = !selected.length
      ? "Select apps to add or remove exemptions."
      : !exempt.length
        ? "Selected apps have no exemptions to remove."
        : (this.loopbackAddCount ? "" : "Selected apps already have exemptions. ") +
          "Remove exemptions for " +
          targets +
          ".";
    const visibleSids = new Set(visible.map((row) => row.sid));
    const hidden = [...this.selected].filter((sid) => !visibleSids.has(sid)).length;
    this.loopbackSelectionText =
      this.selected.size + " selected" + (hidden ? " · " + hidden + " hidden by the filter" : "");
    this.loopbackCountText = visible.length + " of " + this.containers.length + " AppContainers";
  }
  selectMatching(): void {
    if (this.loopbackBusy) {
      return;
    }
    this.selected = new Set(this.loopbackRows.map((row) => row.sid));
    this.render();
  }
  clearSelection(): void {
    if (this.loopbackBusy) {
      return;
    }
    this.selected.clear();
    this.anchor = "";
    this.render();
  }
  toggle(event: Event, sid: string): void {
    if (this.loopbackBusy) {
      return;
    }
    if ((event.currentTarget as HTMLInputElement).checked) {
      this.selected.add(sid);
    } else {
      this.selected.delete(sid);
    }
    this.anchor = sid;
    this.focused = sid;
    this.render();
  }
  private select(sid: string, extend: boolean, toggle: boolean): void {
    const index = this.loopbackRows.findIndex((row) => row.sid === sid),
      anchor = this.loopbackRows.findIndex((row) => row.sid === this.anchor);
    if (extend && anchor >= 0) {
      if (!toggle) {
        this.selected.clear();
      }
      for (const row of this.loopbackRows.slice(
        Math.min(index, anchor),
        Math.max(index, anchor) + 1,
      )) {
        this.selected.add(row.sid);
      }
    } else {
      if (!toggle) {
        this.selected.clear();
      }
      if (toggle && this.selected.has(sid)) {
        this.selected.delete(sid);
      } else {
        this.selected.add(sid);
      }
      this.anchor = sid;
    }
    this.focused = sid;
    this.render();
  }
  rowClick(event: MouseEvent, sid: string): void {
    if (this.loopbackBusy || (event.target as Element).closest("input")) {
      return;
    }
    this.select(sid, event.shiftKey, event.ctrlKey || event.metaKey);
    (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  }
  rowKey(event: KeyboardEvent, sid: string): void {
    if (this.loopbackBusy || (event.target as Element).closest("input")) {
      return;
    }
    const control = event.ctrlKey || event.metaKey;
    if (control && event.key.toLowerCase() === "a") {
      event.preventDefault();
      this.selectMatching();
      return;
    }
    if (event.key === " ") {
      event.preventDefault();
      this.select(sid, event.shiftKey, true);
      return;
    }
    const index = this.loopbackRows.findIndex((row) => row.sid === sid);
    const next =
      event.key === "ArrowDown"
        ? Math.min(index + 1, this.loopbackRows.length - 1)
        : event.key === "ArrowUp"
          ? Math.max(0, index - 1)
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? this.loopbackRows.length - 1
              : -1;
    if (next < 0) {
      return;
    }
    event.preventDefault();
    const target = this.loopbackRows[next]?.sid;
    if (!target) {
      return;
    }
    if (!control || event.shiftKey) {
      this.select(target, event.shiftKey, control);
    } else {
      this.focused = target;
      this.render();
    }
    this.$flushUpdates();
    const row = this.containerTable.querySelector<HTMLElement>(
      'tr[data-appcontainer-sid="' + CSS.escape(target) + '"]',
    );
    row?.focus({ preventScroll: true });
    row?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
  disconnectedCallback(): void {
    this.generation++;
    super.disconnectedCallback();
  }
}
LoopbackExemptions.define("loopback-exemptions");
