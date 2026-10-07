import { WebUIElement, attr } from '@microsoft/webui-framework';

export class PaneDivider extends WebUIElement {
  @attr orientation = 'horizontal';
  @attr label = 'Resize panels';
  @attr value = '45';
  @attr initial = '45';
  private pointer: number | null = null;
  private previous = 45;

  startResize(event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    this.pointer = event.pointerId;
    this.previous = Number(this.value);
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }
  resize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId || !(this.parentElement instanceof HTMLElement)) return;
    const bounds = this.parentElement.getBoundingClientRect();
    const position = this.orientation === 'horizontal' ? (event.clientY-bounds.top)/bounds.height : (event.clientX-bounds.left)/bounds.width;
    this.change(position*100, false);
  }
  finishResize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) return;
    this.pointer = null;
    this.change(Number(this.value), true);
  }
  cancelResize(event: PointerEvent): void {
    if (this.pointer !== event.pointerId) return;
    this.pointer = null;
    this.change(this.previous, true);
  }
  resizeWithKeyboard(event: KeyboardEvent): void {
    if (event.key === 'Escape' && this.pointer === null) return;
    const decrease = this.orientation === 'horizontal' ? 'ArrowUp' : 'ArrowLeft';
    const increase = this.orientation === 'horizontal' ? 'ArrowDown' : 'ArrowRight';
    if (![decrease,increase,'Home','Escape'].includes(event.key)) return;
    event.preventDefault();
    if (event.key === 'Escape') { this.pointer = null; this.change(this.previous,true); return; }
    this.change(event.key === 'Home' ? Number(this.initial) : Number(this.value)+(event.key === increase ? 1 : -1)*(event.shiftKey ? 10 : 2),true);
  }
  reset(): void { this.change(Number(this.initial),true); }
  private change(value: number, committed: boolean): void {
    this.value = String(Math.max(15,Math.min(85,Math.round(value))));
    this.$emit('pane-resize',{value:Number(this.value),committed});
  }
}
PaneDivider.define('pane-divider');
