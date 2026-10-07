import { WebUIElement } from '@microsoft/webui-framework';
import { invoke } from '@tauri-apps/api/core';
import type { NoticeAction, ViewName } from './models.js';
import { describeError } from './utilities.js';

/** Typed events keep workspace components independent of their containing shell. */
export abstract class WorkspaceElement extends WebUIElement {
  protected set diagnostic(message: string) { this.$emit('diagnostic', message); }
  protected showNotice(title: string, message: string, actionLabel: string | null, action: NoticeAction): void {
    this.$emit('notice', { title, message, actionLabel, action });
  }
  protected activateView(view: ViewName): void { this.$emit('navigate', view); }
  protected async reportFrontendIssue(code: string, error: unknown): Promise<void> {
    await invoke<void>('record_frontend_diagnostic', { code, message: describeError(error) }).catch(() => undefined);
  }
}
