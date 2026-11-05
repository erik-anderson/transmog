import { WebUIElement, observable } from "@microsoft/webui-framework";
import type { ValueView } from "./inspection.js";
import type { PropertyRow } from "./properties.js";

export class DebugValueElement extends WebUIElement {
  @observable value: ValueView | undefined;
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
}
DebugValueElement.define("debug-value");
