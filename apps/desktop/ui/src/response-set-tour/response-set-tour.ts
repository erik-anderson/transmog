import { observable, WebUIElement } from "@microsoft/webui-framework";

/** One template-owned, nonmodal callout next to the highlighted overview region. */
export class ResponseSetTour extends WebUIElement {
  @observable stepNumber = 0;
  @observable stepTitle = "";
  @observable stepCopy = "";
  @observable stepLast = false;
  @observable tourContext = "";
  nextButton!: HTMLButtonElement;
  focusNext(): void {
    this.nextButton.focus({ preventScroll: true });
  }
  next(): void {
    this.$emit("tour-next");
  }
  back(): void {
    this.$emit("tour-back");
  }
  end(): void {
    this.$emit("tour-end");
  }
  keyboard(event: KeyboardEvent): void {
    // Help owns shortcuts so native Undo cannot reactivate a field behind it.
    event.stopPropagation();
    if (event.key === "Escape") {
      event.preventDefault();
      this.end();
    } else if (
      event.key === "Delete" ||
      ((event.ctrlKey || event.metaKey) && ["a", "z", "y", "s"].includes(event.key.toLowerCase()))
    ) {
      event.preventDefault();
    }
  }
}
ResponseSetTour.define("response-set-tour");
