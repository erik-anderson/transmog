import { WebUIElement } from "@microsoft/webui-framework";
import type { CertificateInspector } from "../certificate-inspector/certificate-inspector.js";

export class CertificateViewer extends WebUIElement {
  certificateDialog!: HTMLDialogElement;
  certificateInspector!: CertificateInspector;
  private returnFocus: HTMLElement | null = null;
  async show(id: string, trigger: HTMLElement): Promise<void> {
    this.returnFocus = trigger;
    if (!this.certificateDialog.open) {
      this.certificateDialog.showModal();
    }
    await this.certificateInspector.inspect(id);
  }
  close(): void {
    this.certificateDialog.close();
  }
  closed(): void {
    if (this.certificateDialog.open) {
      return;
    }
    this.certificateInspector.cancelInspection();
    this.returnFocus?.focus();
    this.returnFocus = null;
  }
}
CertificateViewer.define("certificate-viewer");
