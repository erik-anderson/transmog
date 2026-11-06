import { WebUIElement, attr } from "@microsoft/webui-framework";

export class PaneDivider extends WebUIElement {
  @attr orientation = "horizontal";
  @attr label = "Resize panels";
  @attr value = "45";
  @attr initial = "45";
  @attr basis = "";
  private pointer: number | null = null;
  private previous = 45;
  private origin = 0;

  startResize(event: PointerEvent): void {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    this.pointer = event.pointerId;
    this.previous = this.currentValue();
    this.origin = this.orientation === "horizontal" ? event.clientY : event.clientX;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  resize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId || !(this.parentElement instanceof HTMLElement)) {
      return;
    }
    const bounds = this.parentElement.getBoundingClientRect();
    const delta =
      this.orientation === "horizontal"
        ? (event.clientY - this.origin) / bounds.height
        : (event.clientX - this.origin) / bounds.width;
    this.change(this.previous + delta * 100, false);
  }
  finishResize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) {
      return;
    }
    this.pointer = null;
    this.change(this.currentValue(), true);
  }
  cancelResize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) {
      return;
    }
    this.pointer = null;
    this.change(this.previous, true);
  }
  resizeWithKeyboard(event: KeyboardEvent): void {
    if (event.key === "Escape" && this.pointer === null) {
      return;
    }
    const decrease = this.orientation === "horizontal" ? "ArrowUp" : "ArrowLeft";
    const increase = this.orientation === "horizontal" ? "ArrowDown" : "ArrowRight";
    if (![decrease, increase, "Home", "Escape"].includes(event.key)) {
      return;
    }
    event.preventDefault();
    if (event.key === "Escape") {
      this.pointer = null;
      this.change(this.previous, true);
      return;
    }
    this.change(
      event.key === "Home"
        ? Number(this.initial)
        : this.currentValue() + (event.key === increase ? 1 : -1) * (event.shiftKey ? 10 : 2),
      true,
    );
  }
  reset(): void {
    this.change(Number(this.initial), true);
  }
  private currentValue(): number {
    const pane = this.previousElementSibling;
    const parent = this.parentElement;
    if (this.basis !== "previous" || !(pane instanceof HTMLElement) || !parent) {
      return Number(this.value);
    }
    const bounds = parent.getBoundingClientRect();
    const previous = pane.getBoundingClientRect();
    const extent = this.orientation === "horizontal" ? bounds.height : bounds.width;
    const size = this.orientation === "horizontal" ? previous.height : previous.width;
    return extent > 0 ? (size / extent) * 100 : Number(this.value);
  }
  private change(value: number, committed: boolean): void {
    this.value = String(Math.max(15, Math.min(85, Math.round(value))));
    this.$emit("pane-resize", { value: Number(this.value), committed });
  }
}
PaneDivider.define("pane-divider");
