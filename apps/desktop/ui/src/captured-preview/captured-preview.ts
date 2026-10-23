import { WebUIElement, observable } from "@microsoft/webui-framework";
import type { TrafficWorkspace } from "../traffic-workspace/traffic-workspace.js";

/** Shared window-level surface over the existing frozen-preview controller. */
export class CapturedPreview extends WebUIElement {
  @observable controller: TrafficWorkspace | null = null;
  previewPageDialog!: HTMLDialogElement;
  previewPageScripts!: HTMLInputElement;
  previewReportDialog!: HTMLDialogElement;
  protected hydratedCallback(): void {
    this.controllerChanged();
  }
  controllerChanged(): void {
    this.$flushUpdates();
    if (!this.controller || !this.previewPageDialog) {
      return;
    }
    this.controller.previewPageDialog = this.previewPageDialog;
    this.controller.previewPageScripts = this.previewPageScripts;
    this.controller.previewReportDialog = this.previewReportDialog;
  }
  previewDocumentChanged(event: Event): void {
    this.controller?.previewDocumentChanged(event);
  }
  setPreviewScope(event: Event): void {
    this.controller?.setPreviewScope(event);
  }
  setPreviewMode(event: Event): void {
    this.controller?.setPreviewMode(event);
  }
  setPreviewRule(id: string, event: Event): void {
    this.controller?.setPreviewRule(id, event);
  }
  excludePreviewDraft(event: Event): void {
    this.controller?.excludePreviewDraft(event);
  }
  openCapturedPage(): void {
    void this.controller?.openCapturedPage();
  }
  closeCapturedPageWarning(): void {
    this.controller?.closeCapturedPageWarning();
  }
  showPreviewRule(id: string): void {
    this.controller?.showPreviewRule(id);
  }
  clearPreviewResponseBaseline(): void {
    this.controller?.clearPreviewResponseBaseline();
  }
  comparePreviewResponse(id: string, source: string, rule: string): void {
    this.controller?.comparePreviewResponse(id, source, rule);
  }
  setPreviewResponseBaseline(id: string): void {
    this.controller?.setPreviewResponseBaseline(id);
  }
  showPreviewSource(id: string): void {
    void this.controller?.showPreviewSource(id);
  }
  showRelatedPreview(mode: string): void {
    this.controller?.showRelatedPreview(mode);
  }
  refreshPreviewReport(): void {
    void this.controller?.refreshPreviewReport();
  }
  closePreviewReport(event?: Event): void {
    this.controller?.closePreviewReport(event);
  }
}
CapturedPreview.define("captured-preview");
