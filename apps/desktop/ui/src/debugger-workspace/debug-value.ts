import { WebUIElement, observable } from "@microsoft/webui-framework";
import { captureFocus, captureRenderScope, renderPresentation } from "../rendering.js";
import type {
  ValueView,
  ValueEditRequest,
  ValueWatchRequest,
  ValueEditDraft,
} from "./inspection.js";
import { isCompletePropertyPath, valuePreview, type PropertyRow } from "./properties.js";
import type { DebugValue } from "./models.js";
import { copyGesture, copyScalar, requestCopy } from "../inspection-copy.js";
import { selectedConverterText } from "../converter-selection.js";

export class DebugValueElement extends WebUIElement {
  @observable value: ValueView | undefined;
  @observable draft: ValueEditDraft | null = null;
  @observable rows: Array<
    PropertyRow & {
      level: number;
      tabIndex: number;
      editing: boolean;
      ariaExpanded: string | undefined;
    }
  > = [];
  @observable rootTabIndex = 0;
  @observable rootExpanded: string | undefined;
  @observable editKey = "";
  @observable editDraft = "";
  @observable editIssue = "";
  @observable editBusy = false;
  @observable editHint = "";
  @observable detailName = "";
  @observable detailText = "";
  @observable detailShortened = false;
  @observable detailReadOnly = false;
  @observable detailKind = "";
  detailDialog!: HTMLDialogElement;
  private editEpoch = "";
  private editRevision = 0;
  private displayedValue: DebugValue | undefined;
  private focusPath = "";
  private focusKey = "";
  private detailReturn: HTMLElement | null = null;
  private focusRestore: string | null = null;
  private moreObserver: IntersectionObserver | undefined;
  private ready = false;
  private valueRenderRevision = 0;
  private detailRevision = 0;
  protected hydratedCallback(): void {
    this.ready = true;
    void this.syncRenderedValue();
  }
  valueChanged(): void {
    const replaced = this.displayedValue !== this.value?.value;
    this.displayedValue = this.value?.value;
    if (replaced) {
      this.detailRevision++;
      this.focusKey = "";
    }
    const active = this.shadowRoot?.activeElement;
    if (active && !this.detailDialog?.open) {
      this.focusRestore = this.focusPath;
    }
    if (this.editKey && this.value?.epoch !== this.editEpoch) {
      this.editRevision++;
      this.editKey = "";
      this.editEpoch = "";
      this.editBusy = false;
      this.editIssue = "";
      this.$emit("inspect-edit-draft", null);
    }
    if (replaced && this.editKey) {
      this.editKey = "";
      if (this.draft && !isCompletePropertyPath(this.draft.path)) {
        this.editRevision++;
        this.editEpoch = "";
        this.editBusy = false;
        this.$emit("inspect-edit-draft", null);
      }
    }
    this.restoreDraft();
    this.updateRows();
    if (this.ready) {
      void this.syncRenderedValue();
    }
  }
  private async syncRenderedValue(): Promise<void> {
    const revision = ++this.valueRenderRevision;
    if (!this.isConnected) {
      return;
    }
    await this.$whenRendered();
    if (!this.isConnected || revision !== this.valueRenderRevision) {
      return;
    }
    if (this.focusRestore !== null) {
      const path = this.focusRestore;
      const row = this.uniqueRow(path);
      if (row || path === "") {
        const parent = this.getRootNode();
        if (
          (parent instanceof ShadowRoot && parent.activeElement === this) ||
          document.activeElement === document.body
        ) {
          this.focusPath = path;
          this.focusKey = row?.key ?? "";
          this.updateRows();
          const focus = captureFocus(this);
          await this.$whenRendered();
          if (!this.isConnected || revision !== this.valueRenderRevision || !focus()) {
            return;
          }
          const target =
            this.editKey && row?.key === this.editKey
              ? this.shadowRoot?.querySelector<HTMLTextAreaElement>(".value-edit textarea")
              : this.rowElement(row);
          target?.focus({ preventScroll: true });
        }
        this.focusRestore = null;
      } else if (!this.value?.loading && !this.value?.restoring) {
        this.focusRestore = null;
      }
    }
    this.moreObserver ??= new IntersectionObserver((entries) => {
      if (!this.value || this.value.loading || this.value.issue || this.value.restoring) {
        return;
      }
      const target = entries.find((entry) => entry.isIntersecting)?.target;
      const row = this.value.rows.find((row) => row.key === target?.getAttribute("data-row-key"));
      if (row) {
        this.toggle(row);
      }
    });
    this.moreObserver.disconnect();
    for (const element of this.shadowRoot!.querySelectorAll("button[data-more]")) {
      this.moreObserver.observe(element);
    }
  }
  draftChanged(): void {
    if (this.editKey && (!this.draft || !this.matchesDraft())) {
      this.editRevision++;
      this.editKey = "";
      this.editEpoch = "";
      this.editBusy = false;
      this.updateRows();
    }
    this.restoreDraft();
  }
  private matchesDraft(): boolean {
    return (
      !!this.draft &&
      !!this.value &&
      this.value.epoch === this.draft.epoch &&
      this.value.label === this.draft.label &&
      this.value.key.startsWith("scope-") === this.draft.key.startsWith("scope-") &&
      (!this.draft.key.startsWith("scope-") || this.value.key === this.draft.key)
    );
  }
  private restoreDraft(): void {
    const draft = this.draft;
    if (
      !draft ||
      this.editKey ||
      !this.matchesDraft() ||
      !isCompletePropertyPath(draft.path) ||
      !this.value?.editable ||
      this.value.expired
    ) {
      return;
    }
    const row = this.uniqueRow(JSON.stringify(draft.path));
    if (!row?.writable) {
      return;
    }
    this.editKey = row.key;
    this.editEpoch = this.value.epoch;
    this.focusPath = JSON.stringify(row.path);
    this.focusKey = row.key;
    this.editDraft = draft.text;
    this.editIssue = draft.issue ?? "";
    this.editHint = this.replacementHint(row);
    this.updateRows();
  }
  private replacementHint(row: PropertyRow): string {
    return row.value.truncated
      ? "The current value is shortened; enter a complete replacement."
      : !this.literalValue(row.value)
        ? "Enter an expression for the replacement value. Side effects are blocked."
        : "JavaScript value · side effects blocked.";
  }
  private literalValue(value: DebugValue): boolean {
    return ["string", "number", "bigint", "boolean", "null", "undefined"].includes(value.kind);
  }
  private uniqueRow(path: string): PropertyRow | undefined {
    const matches = this.value?.rows.filter((row) => JSON.stringify(row.path) === path) ?? [];
    return matches.length === 1 ? matches[0] : undefined;
  }
  private rememberDraft(row?: PropertyRow): void {
    const current = row ?? this.value?.rows.find((row) => row.key === this.editKey);
    this.$emit(
      "inspect-edit-draft",
      this.editKey && current && this.value
        ? ({
            key: this.value.key,
            label: this.value.label,
            epoch: this.editEpoch,
            path: current.path,
            text: this.editDraft,
            issue: this.editIssue,
          } satisfies ValueEditDraft)
        : null,
    );
  }
  private updateRows(): void {
    const selected =
      this.value?.rows.find((row) => row.key === this.focusKey) ?? this.uniqueRow(this.focusPath);
    this.rootTabIndex = selected ? -1 : 0;
    this.rootExpanded = this.value?.value.handle ? String(this.value.expanded) : undefined;
    this.rows = (this.value?.rows ?? []).map((row) => ({
      ...row,
      level: row.depth + 2,
      tabIndex: row === selected ? 0 : -1,
      editing: row.key === this.editKey,
      ariaExpanded: row.expandable ? String(row.expanded) : undefined,
    }));
  }
  focused(row?: PropertyRow): void {
    this.focusPath = row ? JSON.stringify(row.path) : "";
    this.focusKey = row?.key ?? "";
    this.updateRows();
  }
  private rowElement(row?: PropertyRow): HTMLElement | undefined {
    return row
      ? [...this.shadowRoot!.querySelectorAll<HTMLElement>("tr[data-row-key]")].find(
          (element) => element.dataset.rowKey === row.key,
        )
      : (this.shadowRoot?.querySelector<HTMLElement>(".heading") ?? undefined);
  }
  private async focusRow(row?: PropertyRow): Promise<void> {
    const focus = captureFocus(this);
    this.focused(row);
    const current = captureRenderScope(this, () => [
      this.value?.epoch,
      this.focusKey,
      this.focusPath,
    ]);
    await this.$whenRendered();
    if (current() && focus()) {
      this.rowElement(row)?.focus();
    }
  }
  async key(event: KeyboardEvent, row?: PropertyRow): Promise<void> {
    const eventTarget = event?.currentTarget;
    if (
      event
        .composedPath()
        .some((node) => node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement)
    ) {
      return;
    }
    if (event.key === "F2" && row) {
      event.preventDefault();
      event.stopPropagation();
      await this.beginEdit(row);
      return;
    }
    const rows = this.value?.rows ?? [];
    const index = row ? rows.findIndex((item) => item.key === row.key) : -1;
    const expanded = row ? row.expanded : this.value?.expanded;
    if (["ArrowDown", "ArrowUp", "Home", "End", "ArrowLeft", "ArrowRight"].includes(event.key)) {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Home") {
        await this.focusRow();
      } else if (event.key === "End") {
        await this.focusRow(rows.at(-1));
      } else if (event.key === "ArrowDown") {
        await this.focusRow(rows[Math.min(index + 1, rows.length - 1)]);
      } else if (event.key === "ArrowUp") {
        await this.focusRow(rows[index - 1]);
      } else if (event.key === "ArrowRight") {
        if (!expanded && (!row || row.expandable || row.offset !== undefined)) {
          this.toggle(row);
        } else if (expanded) {
          const child = rows[index + 1];
          if (child && (!row || child.depth > row.depth)) {
            await this.focusRow(child);
          }
        }
      } else if (expanded) {
        this.toggle(row);
      } else {
        await this.focusRow(
          row
            ? rows
                .slice(0, index)
                .reverse()
                .find((item) => item.depth < row.depth)
            : undefined,
        );
      }
      return;
    }
    if (event.key === "Enter" && event.target === eventTarget) {
      event.preventDefault();
      event.stopPropagation();
      if (!row || row.expandable || row.offset !== undefined) {
        this.toggle(row);
      } else {
        await this.showValue(row);
      }
      return;
    }
    this.menu(event, row);
  }
  async beginEdit(row: PropertyRow): Promise<void> {
    if (!this.value?.editable || this.value.expired || this.value.loading || this.editBusy) {
      return;
    }
    if (!row.writable) {
      await this.showValue(row);
      return;
    }
    const focus = captureFocus(this);
    this.editKey = row.key;
    this.editRevision++;
    this.editEpoch = this.value.epoch;
    this.editDraft =
      row.value.truncated || !this.literalValue(row.value) ? "" : valuePreview(row.value);
    this.editIssue = "";
    this.editHint = this.replacementHint(row);
    this.rememberDraft(row);
    this.updateRows();
    const current = captureRenderScope(this, () => [
      this.editRevision,
      this.editKey,
      this.editDraft,
      this.value?.epoch,
    ]);
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    const input = this.shadowRoot?.querySelector<HTMLTextAreaElement>(".value-edit textarea");
    input?.focus();
    input?.select();
  }
  editInput(event: Event): void {
    this.editDraft = (event.target as HTMLTextAreaElement).value;
    this.rememberDraft();
  }
  async editKeydown(event: KeyboardEvent, row: PropertyRow): Promise<void> {
    event.stopPropagation();
    if (event.isComposing || event.repeat) {
      return;
    }
    if (event.key === "Escape" && !this.editBusy) {
      event.preventDefault();
      await this.cancelEdit(row);
    } else if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      this.saveEdit(event, row);
    }
  }
  async cancelEdit(row: PropertyRow): Promise<void> {
    this.editRevision++;
    this.editKey = "";
    this.rememberDraft();
    this.editIssue = "";
    this.updateRows();
    await this.focusRow(row);
  }
  saveEdit(event: Event, row: PropertyRow): void {
    event.preventDefault();
    event.stopPropagation();
    if (!this.value || !this.editDraft.trim() || this.editBusy) {
      return;
    }
    const epoch = this.editEpoch;
    const value = this.value.value;
    const revision = this.editRevision;
    this.editBusy = true;
    this.editIssue = "";
    const request: ValueEditRequest = {
      key: this.value.key,
      row,
      expression: this.editDraft.trim(),
      reply: async (issue: string) => {
        if (
          !this.isConnected ||
          this.editRevision !== revision ||
          this.editEpoch !== epoch ||
          this.value?.epoch !== epoch
        ) {
          return;
        }
        this.editBusy = false;
        if (issue) {
          this.editIssue = issue;
          this.rememberDraft();
        } else {
          const focused = !!this.shadowRoot?.activeElement && !this.value.expired;
          this.editKey = "";
          this.rememberDraft();
          this.updateRows();
          if (focused) {
            await this.focusRow(
              this.value.value === value
                ? this.value.rows.find((item) => item.key === row.key)
                : this.uniqueRow(JSON.stringify(row.path)),
            );
          }
        }
      },
    };
    this.$emit("inspect-edit", request);
  }
  async showValue(row?: PropertyRow): Promise<void> {
    const value = row?.value ?? this.value?.value;
    if (!value) {
      return;
    }
    this.detailName = row?.name ?? this.value!.label;
    this.detailText = value.display;
    this.detailKind = value.kind;
    this.detailShortened = value.truncated;
    this.detailReadOnly = !!row && this.value?.editable === true && !row.writable;
    this.detailReturn = this.rowElement(row) ?? null;
    this.detailRevision++;
    const current = await renderPresentation(this, () => [this.detailRevision, this.value?.epoch]);
    if (current()) {
      this.detailDialog.showModal();
    }
  }
  closeDetail(): void {
    this.detailRevision++;
    this.detailDialog.close();
  }
  detailClosed(): void {
    if (this.detailDialog.open) {
      return;
    }
    if (this.detailReturn?.isConnected) {
      this.detailReturn.focus();
    }
  }
  toggle(row?: PropertyRow): void {
    if (this.value) {
      this.dispatchEvent(
        new CustomEvent("inspect-toggle", {
          bubbles: true,
          composed: true,
          detail: { key: this.value.key, row },
        }),
      );
    }
  }
  menu(event: MouseEvent | KeyboardEvent, row?: PropertyRow): void {
    if (!this.value || !copyGesture(event) || selectedConverterText(event)) {
      return;
    }
    const view = this.value;
    const displayedRows = view.rows;
    const source = event.currentTarget as HTMLElement;
    const name = row?.name ?? view.label;
    const value = row?.value ?? view.value;
    const addressable =
      row &&
      row.propertyIndex !== undefined &&
      view.editable &&
      (view.key === "scope-0" || view.key.startsWith("watch-")) &&
      (!view.key.startsWith("scope-") || /^[A-Za-z_$][\w$]*$/u.test(row.path[0] ?? "")) &&
      isCompletePropertyPath(row.path) &&
      view.rows.filter((item) => JSON.stringify(item.path) === JSON.stringify(row.path)).length ===
        1;
    const rows = row
      ? view.rows.slice(view.rows.findIndex((item) => item.key === row.key) + 1)
      : view.rows;
    const children: PropertyRow[] = [];
    for (const child of rows) {
      if (row && child.depth <= row.depth) {
        break;
      }
      children.push(child);
    }
    const text = [
      (copyScalar(name) || '""') +
        ": " +
        copyScalar(value.display) +
        (value.truncated ? " [Shortened]" : ""),
      ...children.map(
        (child) =>
          "  ".repeat(child.depth - (row?.depth ?? -1)) +
          (copyScalar(child.name) || '""') +
          ": " +
          copyScalar(child.value.display) +
          (child.value.truncated ? " [Shortened]" : ""),
      ),
    ].join("\n");
    requestCopy(event, {
      label: "Debugger value actions",
      source,
      current: () =>
        this.value?.epoch === view.epoch &&
        !this.value.expired &&
        this.value.rows === displayedRows,
      actions: [
        { id: "view", label: "View value", run: async () => await this.showValue(row) },
        ...(row && view.editable && row.writable
          ? [{ id: "edit", label: "Edit value", run: async () => await this.beginEdit(row) }]
          : []),
        ...(addressable
          ? [
              {
                id: "watch",
                label: "Add to Watch",
                run: () => {
                  this.$emit("inspect-watch", { key: view.key, row } satisfies ValueWatchRequest);
                },
              },
            ]
          : []),
        ...(addressable
          ? [{ id: "path", label: "Copy property path", read: () => this.expression(row) }]
          : []),
        { id: "name", label: "Copy name", read: () => copyScalar(name) },
        value.handle
          ? { id: "subtree", label: "Copy displayed subtree", read: () => text }
          : {
              id: "value",
              label: "Copy value",
              read: () => copyScalar(value.display) + (value.truncated ? " [Shortened]" : ""),
            },
      ],
    });
  }
  expression(row: PropertyRow): string {
    const scope = this.value!.key.startsWith("scope-");
    return (
      (scope ? "" : "(" + this.value!.label + ")") +
      row.path
        .map((part, index) =>
          /^[A-Za-z_$][\w$]*$/u.test(part)
            ? (index || !scope ? "." : "") + part
            : "[" + JSON.stringify(part) + "]",
        )
        .join("")
    );
  }
  disconnectedCallback(): void {
    this.moreObserver?.disconnect();
    super.disconnectedCallback();
  }
}
DebugValueElement.define("debug-value");
