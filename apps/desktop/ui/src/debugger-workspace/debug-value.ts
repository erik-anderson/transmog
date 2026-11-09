import { WebUIElement, observable } from "@microsoft/webui-framework";
import type { ValueView } from "./inspection.js";
import type { PropertyRow } from "./properties.js";
import { copyGesture, copyScalar, requestCopy } from "../inspection-copy.js";
import { selectedConverterText } from "../converter-selection.js";

export class DebugValueElement extends WebUIElement {
  @observable value: ValueView | undefined;
  private moreObserver: IntersectionObserver | undefined;
  valueChanged(): void {
    queueMicrotask(() => {
      if (!this.isConnected) {
        return;
      }
      this.$flushUpdates();
      this.moreObserver ??= new IntersectionObserver((entries) => {
        if (!this.value || this.value.loading || this.value.issue) {
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
    });
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
    const rows = row ? view.rows.slice(view.rows.indexOf(row) + 1) : view.rows;
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
      source,
      current: () => this.value === view && !view.expired && view.rows === displayedRows,
      actions: [
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
  disconnectedCallback(): void {
    this.moreObserver?.disconnect();
    super.disconnectedCallback();
  }
}
DebugValueElement.define("debug-value");
