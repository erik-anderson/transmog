import { WebUIElement, observable } from "@microsoft/webui-framework";
import type { ValueView } from "./inspection.js";
import type { PropertyRow } from "./properties.js";

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
  disconnectedCallback(): void {
    this.moreObserver?.disconnect();
    super.disconnectedCallback();
  }
}
DebugValueElement.define("debug-value");
