import { WebUIElement, attr, observable } from '@microsoft/webui-framework';

/** A roomy embedded editor that can become a modal child-window surface. */
export class EditorSurface extends WebUIElement {
  @attr label = 'Editor';
  @observable expanded = false;
  surface!: HTMLDialogElement;
  toggle(): void {
    if (this.expanded) { this.restore(); return; }
    this.surface.close();
    this.surface.showModal();
    this.expanded = true;
    this.$emit('editor-expanded');
  }
  cancel(event: Event): void { event.preventDefault(); this.restore(); }
  closed(): void { if (!this.surface.open) { this.expanded = false; this.surface.show(); } }
  private restore(): void {
    this.surface.close();
    this.surface.show();
    this.expanded = false;
    this.$emit('editor-restored');
  }
}
EditorSurface.define('editor-surface');
